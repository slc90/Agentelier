mod computer;
mod ecs;
mod tool;
pub mod widget;

pub(crate) use tool::{definition as tool_definition, ecs_definition, execute as execute_tool};

use crate::{
    ImagePayload, ToolFailure,
    desktop::{CaptureMapping, uia},
    ecs_access::EcsAccess,
};
use agentelier_skills::UserSkills;
use rhai::{AST, Dynamic, Engine, Scope};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024;
const MAX_FUNCTIONS: usize = 128;
const MAX_OPERATIONS: u64 = 2_000_000;
const MAX_CALL_LEVELS: usize = 32;
const MAX_EXPRESSION_DEPTH: usize = 64;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION_SIZE: usize = 8192;

static DESKTOP_CLEANUP_CONFIRMED: AsyncMutex<bool> = AsyncMutex::const_new(true);

pub struct ScriptOutput {
    pub text: String,
    pub text_truncated: bool,
    pub image: Option<Arc<ImagePayload>>,
}

#[derive(Default)]
struct OutputBuffer {
    text: String,
    truncated: bool,
}

#[derive(Clone)]
pub struct ScriptSession {
    state: Arc<AsyncMutex<ScriptState>>,
    ecs_access: EcsAccess,
    skills: UserSkills,
}

pub(crate) enum ScriptTarget {
    Desktop,
    Ecs,
}

struct ScriptState {
    engine: Engine,
    scope: Scope<'static>,
    functions: AST,
    observed: Option<CaptureMapping>,
    uia: Arc<uia::Runtime>,
}

impl Default for ScriptState {
    fn default() -> Self {
        let mut engine = engine();
        computer::register(&mut engine);
        Self {
            engine,
            scope: Scope::new(),
            functions: AST::empty(),
            observed: None,
            uia: Arc::default(),
        }
    }
}

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_expr_depths(MAX_EXPRESSION_DEPTH, MAX_EXPRESSION_DEPTH)
        .set_max_functions(MAX_FUNCTIONS)
        .set_max_string_size(MAX_STRING_BYTES)
        .set_max_array_size(MAX_COLLECTION_SIZE)
        .set_max_map_size(MAX_COLLECTION_SIZE);
    ecs::register(&mut engine);
    engine
}

impl ScriptSession {
    pub fn new(ecs_access: EcsAccess, skills: UserSkills) -> Self {
        Self {
            state: Arc::default(),
            ecs_access,
            skills,
        }
    }

    pub async fn close_uia(&self) -> Result<(), ToolFailure> {
        self.state.lock().await.uia.close().await
    }

    pub(crate) async fn validate_scene(
        &self,
        mut definition: crate::ecs_access::SceneDefinition,
        target: Option<(crate::ecs_access::SceneTarget, u64)>,
        stored: Option<crate::ecs_access::SceneStored>,
        cancel: &CancellationToken,
    ) -> Result<crate::ecs_access::SceneDefinition, ToolFailure> {
        let access = self.ecs_access.clone();
        let candidate = if definition.template == widget::TEMPLATE {
            let captured = if let Some((target, revision)) = target {
                let access = access.clone();
                let control = Control::new(cancel);
                let _cancel_on_drop = control.cancel.clone().drop_guard();
                let state = tokio::task::spawn_blocking(move || {
                    access
                        .call(
                            crate::ecs_access::EcsOperation::WidgetState(target, revision),
                            &control.cancel,
                            control.deadline,
                        )
                        .map_err(|error| ToolFailure::new(error.code, error.message))
                })
                .await
                .map_err(worker_failure)??;
                serde_json::from_value::<Option<widget::State>>(state)
                    .map_err(|error| ToolFailure::new("invalid_widget_state", error.to_string()))?
            } else {
                None
            };
            let state = match &captured {
                Some(state) => Some(state.clone()),
                None => widget::prior_state(stored.as_ref())?,
            };
            let program = widget::prepare(
                &mut definition,
                stored.as_ref(),
                state,
                access.clone(),
                self.skills.clone(),
                cancel,
            )
            .await?;
            target.map(|(_, expected_revision)| widget::Candidate {
                expected_revision,
                captured,
                program,
            })
        } else {
            None
        };
        let control = Control::new(cancel);
        let _cancel_on_drop = control.cancel.clone().drop_guard();
        tokio::task::spawn_blocking(move || {
            access
                .call(
                    crate::ecs_access::EcsOperation::SceneValidate {
                        definition,
                        target: target.map(|(target, _)| target),
                        stored,
                        widget: candidate,
                    },
                    &control.cancel,
                    control.deadline,
                )
                .map_err(|error| ToolFailure::new(error.code, error.message))
                .and_then(|checked| {
                    serde_json::from_value(checked).map_err(|error| {
                        ToolFailure::new("scene_validation_failed", error.to_string())
                    })
                })
        })
        .await
        .map_err(worker_failure)?
    }

    pub(crate) async fn run(
        &self,
        target: ScriptTarget,
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
        if matches!(target, ScriptTarget::Ecs) {
            let control = Control::new(cancel);
            let _cancel_on_drop = control.cancel.clone().drop_guard();
            let run = ecs::Run {
                access: self.ecs_access.clone(),
                control,
            };
            let script = script.to_owned();
            return tokio::task::spawn_blocking(move || {
                state.run(&script, run.control.clone(), Some(run))
            })
            .await
            .map_err(worker_failure)?;
        }
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
        let control = Control::new(cancel);
        let _cancel_on_drop = control.cancel.clone().drop_guard();
        let script = script.to_owned();
        tokio::task::spawn_blocking(move || {
            // Keep desktop ownership on this worker, including on unwind or dropped awaiters.
            *desktop = false;
            let result = state.run(&script, control, None);
            *desktop = result
                .as_ref()
                .err()
                .is_none_or(|error| error.code != "desktop_input_cleanup");
            result
        })
        .await
        .map_err(worker_failure)?
    }
}

impl ScriptState {
    fn run(
        &mut self,
        script: &str,
        control: Control,
        ecs: Option<ecs::Run>,
    ) -> Result<ScriptOutput, ToolFailure> {
        control.check()?;
        let ast = self
            .engine
            .compile_with_scope(&self.scope, script)
            .map_err(|error| ToolFailure::new("script_parse_failed", error.to_string()))?;
        let program = self.functions.merge(&ast);

        let output = Arc::new(Mutex::new(OutputBuffer::default()));
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

        let (result, mapping, image) = if let Some(run) = ecs {
            (self.evaluate(&program, Dynamic::from(run)), None, None)
        } else {
            let desktop = Arc::new(Mutex::new(computer::Run::new(
                control.clone(),
                self.observed.clone(),
                self.uia.clone(),
            )));
            let result = self.evaluate(&program, Dynamic::from(desktop.clone()));
            let (mapping, image) = desktop
                .lock()
                .map_err(|error| ToolFailure::new("desktop_input_cleanup", error.to_string()))?
                .finish()?;
            (result, Some(mapping), image)
        };
        let result = result.map_err(|error| computer::script_failure(&error));
        control
            .check()
            .map_err(|failure| match result.as_ref().err() {
                Some(error)
                    if matches!(
                        (failure.code, error.code),
                        ("cancelled", "uia_cancelled") | ("timeout", "uia_timeout")
                    ) =>
                {
                    error.clone()
                }
                _ => failure,
            })?;
        let value = result?;
        self.functions = program.clone_functions_only();
        if let Some(mapping) = mapping {
            self.observed = mapping;
        }

        let mut output = output
            .lock()
            .map_err(|error| ToolFailure::new("script_output_failed", error.to_string()))?;
        if !value.is_unit() {
            output.append(&value.to_string());
        }
        Ok(ScriptOutput {
            text: std::mem::take(&mut output.text),
            text_truncated: output.truncated,
            image,
        })
    }

    fn evaluate(
        &mut self,
        program: &AST,
        tag: Dynamic,
    ) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        self.engine.set_default_tag(tag);
        let tagged = TaggedEngine(&mut self.engine);
        tagged
            .0
            .eval_ast_with_scope::<Dynamic>(&mut self.scope, program)
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
    fn new(cancel: &CancellationToken) -> Self {
        Self {
            cancel: cancel.child_token(),
            deadline: Instant::now() + SCRIPT_TIMEOUT,
        }
    }

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

fn worker_failure(error: tokio::task::JoinError) -> ToolFailure {
    ToolFailure::new("script_worker_failed", error.to_string())
}

fn cancelled() -> ToolFailure {
    ToolFailure::new("cancelled", "Rhai 脚本已取消")
}

fn print_line(output: &Mutex<OutputBuffer>, line: &str) {
    if let Ok(mut output) = output.lock() {
        output.append(line);
        output.append("\n");
    }
}

impl OutputBuffer {
    fn append(&mut self, text: &str) {
        if !self.truncated {
            let limit = MAX_OUTPUT_BYTES.saturating_sub(self.text.len());
            let end = text.floor_char_boundary(text.len().min(limit));
            self.text.push_str(&text[..end]);
            self.truncated = end < text.len();
        }
    }
}
