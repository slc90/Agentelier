mod computer;
mod tool;

pub(crate) use tool::{definition as tool_definition, execute as execute_tool};

use crate::{ImagePayload, ToolFailure, desktop::CaptureMapping};
use rhai::{AST, Dynamic, Engine, Scope};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024;
const MAX_FUNCTIONS: usize = 128;
const MAX_VARIABLES: usize = 256;
const MAX_OPERATIONS: u64 = 2_000_000;
const MAX_CALL_LEVELS: usize = 32;
const MAX_EXPRESSION_DEPTH: usize = 64;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION_SIZE: usize = 8192;

static DESKTOP_CLEANUP_CONFIRMED: AsyncMutex<bool> = AsyncMutex::const_new(true);

pub struct ScriptOutput {
    pub text: String,
    pub image: Option<Arc<ImagePayload>>,
}

#[derive(Clone, Default)]
pub struct ScriptSession {
    state: Arc<AsyncMutex<ScriptState>>,
}

struct ScriptState {
    engine: Engine,
    scope: Scope<'static>,
    functions: AST,
    observed: Option<CaptureMapping>,
}

impl Default for ScriptState {
    fn default() -> Self {
        let mut engine = Engine::new();
        engine
            .set_max_operations(MAX_OPERATIONS)
            .set_max_call_levels(MAX_CALL_LEVELS)
            .set_max_expr_depths(MAX_EXPRESSION_DEPTH, MAX_EXPRESSION_DEPTH)
            .set_max_functions(MAX_FUNCTIONS)
            .set_max_variables(MAX_VARIABLES)
            .set_max_string_size(MAX_STRING_BYTES)
            .set_max_array_size(MAX_COLLECTION_SIZE)
            .set_max_map_size(MAX_COLLECTION_SIZE);
        computer::register(&mut engine);
        Self {
            engine,
            scope: Scope::new(),
            functions: AST::empty(),
            observed: None,
        }
    }
}

impl ScriptSession {
    pub async fn run(
        &self,
        script: &str,
        cancel: &CancellationToken,
    ) -> Result<ScriptOutput, ToolFailure> {
        if script.trim().is_empty() || script.len() > MAX_SCRIPT_BYTES {
            return Err(ToolFailure::new(
                "invalid_script",
                "Rhai 脚本必须为非空文本且不超过 64 KiB",
            ));
        }
        let mut state = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(cancelled()),
            state = self.state.clone().lock_owned() => state,
        };
        let mut desktop = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(cancelled()),
            desktop = DESKTOP_CLEANUP_CONFIRMED.lock() => desktop,
        };
        if !*desktop {
            return Err(ToolFailure::new(
                "desktop_unconfirmed",
                "桌面输入清理尚未确认，当前无法接受新的桌面操作",
            ));
        }
        let worker_cancel = cancel.child_token();
        let _cancel_on_drop = worker_cancel.clone().drop_guard();
        let control = Control {
            cancel: worker_cancel,
            deadline: Instant::now() + SCRIPT_TIMEOUT,
        };
        let script = script.to_owned();
        tokio::task::spawn_blocking(move || {
            // Keep desktop ownership on this worker, including on unwind or dropped awaiters.
            *desktop = false;
            let result = state.run(&script, control);
            *desktop = result
                .as_ref()
                .err()
                .is_none_or(|error| error.code != "desktop_input_cleanup");
            result
        })
        .await
        .map_err(|error| ToolFailure::new("script_worker_failed", error.to_string()))?
    }
}

impl ScriptState {
    fn run(&mut self, script: &str, control: Control) -> Result<ScriptOutput, ToolFailure> {
        control.check()?;
        let ast = self
            .engine
            .compile_with_scope(&self.scope, script)
            .map_err(|error| ToolFailure::new("script_parse_failed", error.to_string()))?;
        let program = self.functions.merge(&ast);
        if program.iter_functions().take(MAX_FUNCTIONS + 1).count() > MAX_FUNCTIONS {
            return Err(ToolFailure::new(
                "script_function_limit",
                "会话函数数量超过限制",
            ));
        }

        let output = Arc::new(Mutex::new(String::new()));
        let printer = output.clone();
        let debugger = output.clone();
        let progress = control.clone();
        self.engine
            .on_print(move |line| print_line(&printer, line))
            .on_debug(move |line, _, _| print_line(&debugger, line))
            .on_progress(move |_| {
                progress
                    .check()
                    .err()
                    .map(|error| Dynamic::from(error.code))
            });

        let desktop = Arc::new(Mutex::new(computer::Run::new(
            control.clone(),
            self.observed.clone(),
        )));
        let result = {
            self.engine.set_default_tag(Dynamic::from(desktop.clone()));
            let tagged = TaggedEngine(&mut self.engine);
            tagged
                .0
                .eval_ast_with_scope::<Dynamic>(&mut self.scope, &program)
        };
        let (mapping, image) = desktop
            .lock()
            .map_err(|error| ToolFailure::new("desktop_input_cleanup", error.to_string()))?
            .finish()?;
        control.check()?;
        let value = result.map_err(|error| computer::script_failure(&error))?;
        self.functions = program.clone_functions_only();
        self.observed = mapping;

        let mut output = output
            .lock()
            .map_err(|error| ToolFailure::new("script_output_failed", error.to_string()))?;
        if !value.is_unit() {
            append_output(&mut output, &value.to_string());
        }
        Ok(ScriptOutput {
            text: std::mem::take(&mut output),
            image,
        })
    }
}

struct TaggedEngine<'a>(&'a mut Engine);

impl Drop for TaggedEngine<'_> {
    fn drop(&mut self) {
        self.0.set_default_tag(Dynamic::UNIT);
    }
}

#[derive(Clone)]
struct Control {
    cancel: CancellationToken,
    deadline: Instant,
}

impl Control {
    fn check(&self) -> Result<(), ToolFailure> {
        if self.cancel.is_cancelled() {
            Err(cancelled())
        } else if Instant::now() >= self.deadline {
            Err(ToolFailure::new("timeout", "Rhai 脚本超过执行期限"))
        } else {
            Ok(())
        }
    }
}

fn cancelled() -> ToolFailure {
    ToolFailure::new("cancelled", "Rhai 脚本已取消")
}

fn print_line(output: &Mutex<String>, line: &str) {
    if let Ok(mut output) = output.lock() {
        append_output(&mut output, line);
        append_output(&mut output, "\n");
    }
}

fn append_output(output: &mut String, text: &str) {
    let limit = MAX_OUTPUT_BYTES.saturating_sub(output.len());
    output.push_str(&text[..text.floor_char_boundary(text.len().min(limit))]);
}
