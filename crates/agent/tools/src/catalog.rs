use crate::{
    ExecutionContext, ProcessSessions, ToolAccess, ToolFailure, ToolPermit, ToolResult, execution,
};

const SHELL_LIFETIME: &str = "If still running, returns a conversation-scoped session_id for write_stdin. Native asynchronous work and yielded processes can outlive a turn. At most 8 live or unread process results are retained. Use list_async_tasks to discover them.";
const LEGACY_SHELL_LIFETIME: &str = "If still running, returns a task-scoped session_id for write_stdin and new output. No command lifetime limit. At most 8 unreaped sessions. All processes are reaped when this user task ends; old IDs cannot be used in a later task.";
const STDIN_SCOPE: &str = "Continue an existing process in this conversation.";
const LEGACY_STDIN_SCOPE: &str = "Continue a process session from this user task.";
const STDIN_EXPIRATION: &str = "Consumed or restored historical IDs cannot be used.";
const LEGACY_STDIN_EXPIRATION: &str = "Sessions expire at task end.";
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

const MAX_MODEL_NAME_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolContract {
    #[default]
    Flat,
    AdditionalTools,
    CodexWebSocket,
    #[serde(rename = "native_async")]
    ExperimentalNamespace,
}

impl ToolContract {
    pub fn is_flat(&self) -> bool {
        *self == Self::Flat
    }

    pub fn namespace(self) -> Option<&'static str> {
        match self {
            Self::Flat | Self::AdditionalTools | Self::CodexWebSocket => None,
            Self::ExperimentalNamespace => Some("agentelier"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    #[default]
    Synchronous,
    Asynchronous,
}

impl ToolMode {
    fn is_synchronous(&self) -> bool {
        *self == Self::Synchronous
    }
}

#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSource {
    #[default]
    Builtin,
    Extension {
        provider: String,
        tool: String,
    },
    Mcp {
        server: uuid::Uuid,
        target: String,
        tool: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default)]
    pub source: ToolSource,
    #[serde(default = "strict_by_default")]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "ToolMode::is_synchronous")]
    pub mode: ToolMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

impl ToolDefinition {
    pub fn same_declaration(&self, other: &Self) -> bool {
        self.mode == other.mode && self.same_executor_contract(other)
    }

    pub fn can_bind(&self, saved: &Self) -> bool {
        self.same_executor_contract(saved)
            && (saved.mode == ToolMode::Synchronous || saved.mode == self.mode)
    }

    fn same_executor_contract(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.parameters == other.parameters
            && self.source == other.source
            && self.strict == other.strict
            && self.output_schema == other.output_schema
    }

    pub fn catalog_description(&self) -> String {
        match &self.source {
            ToolSource::Mcp { server, tool, .. } => {
                let source = self
                    .display_source
                    .clone()
                    .unwrap_or_else(|| format!("MCP {server} · {tool}"));
                let mut details = format!(
                    "{source} · strict={}\n{}\n{}",
                    self.strict, self.description, self.parameters
                );
                if let Some(schema) = &self.output_schema {
                    details.push_str(&format!("\noutputSchema {schema}"));
                }
                details
            }
            _ => self.description.clone(),
        }
    }
}

fn strict_by_default() -> bool {
    true
}

pub type ToolExecution<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;
type ToolExecutor = dyn for<'a> Fn(
        &'a ExecutionContext,
        &'a str,
        &'a CancellationToken,
        crate::ProcessOrigin,
    ) -> ToolExecution<'a>
    + Send
    + Sync;

#[derive(Clone)]
enum Executor {
    Function(Arc<ToolExecutor>),
    CodexImage,
    Shell,
    Stdin,
    TaskControl(crate::TaskControl),
}

#[derive(Clone)]
pub struct ToolRegistration {
    definition: ToolDefinition,
    executor: Executor,
    access: ToolAccess,
    image_result: bool,
}

impl ToolRegistration {
    fn can_rebind(&self, saved: &ToolDefinition) -> bool {
        if self.definition.can_bind(saved) {
            return true;
        }
        let mut legacy = self.definition.clone();
        legacy.description = match self.executor {
            Executor::Shell => legacy
                .description
                .replace(SHELL_LIFETIME, LEGACY_SHELL_LIFETIME),
            Executor::Stdin => legacy
                .description
                .replace(STDIN_SCOPE, LEGACY_STDIN_SCOPE)
                .replace(STDIN_EXPIRATION, LEGACY_STDIN_EXPIRATION),
            _ => return false,
        };
        legacy.can_bind(saved)
    }
    pub fn new(
        definition: ToolDefinition,
        executor: impl for<'a> Fn(
            &'a ExecutionContext,
            &'a str,
            &'a CancellationToken,
        ) -> ToolExecution<'a>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_origin(definition, move |context, arguments, cancel, _| {
            executor(context, arguments, cancel)
        })
    }

    pub fn with_origin(
        definition: ToolDefinition,
        executor: impl for<'a> Fn(
            &'a ExecutionContext,
            &'a str,
            &'a CancellationToken,
            crate::ProcessOrigin,
        ) -> ToolExecution<'a>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            definition,
            executor: Executor::Function(Arc::new(executor)),
            access: ToolAccess::Exclusive,
            image_result: true,
        }
    }

    pub fn shared_read(mut self) -> Self {
        self.access = ToolAccess::SharedRead;
        self
    }

    pub fn text_result(mut self) -> Self {
        self.image_result = false;
        self
    }

    fn process(definition: ToolDefinition, executor: Executor, access: ToolAccess) -> Self {
        Self {
            definition,
            executor,
            access,
            image_result: false,
        }
    }

    pub(crate) fn execute<'a>(
        &'a self,
        context: &'a ExecutionContext,
        processes: &'a ProcessSessions,
        arguments: &'a str,
        cancel: &'a CancellationToken,
        mut permit: ToolPermit,
        origin: crate::ProcessOrigin,
    ) -> ToolExecution<'a> {
        match &self.executor {
            Executor::Shell => Box::pin(async move {
                let Some(lease) = permit.exclusive.take() else {
                    return ToolResult::error(
                        crate::ToolStatus::Unconfirmed,
                        "process_admission",
                        "命令缺少独占许可",
                    );
                };
                let result = processes
                    .exec(
                        context.working_directory(),
                        arguments,
                        cancel,
                        lease,
                        origin,
                    )
                    .await;
                drop(permit);
                result
            }),
            Executor::Stdin => Box::pin(async move { processes.stdin(arguments, cancel).await }),
            Executor::TaskControl(_) | Executor::CodexImage => Box::pin(async {
                ToolResult::error(
                    crate::ToolStatus::Unconfirmed,
                    "tool_route",
                    "专用工具被错误派至普通执行器",
                )
            }),
            Executor::Function(execute) => {
                let execution = execute(context, arguments, cancel, origin);
                Box::pin(async move {
                    let result = execution.await;
                    if result.status == crate::ToolStatus::ExternalUnconfirmed {
                        permit.mark_external_unconfirmed();
                    }
                    drop(permit);
                    self.limit_result(result)
                })
            }
        }
    }

    fn limit_result(&self, result: ToolResult) -> ToolResult {
        if result.output.summary().len() <= crate::MAX_TOOL_BODY_BYTES
            && (self.image_result || result.output.image().is_none())
        {
            return result;
        }
        let mut failure = ToolResult::error(
            crate::ToolStatus::Failed,
            "result_too_large",
            "工具结果超过大小限制",
        );
        failure.cleanup_failed = result.cleanup_failed;
        failure
    }
}

#[derive(Default)]
pub struct ToolCatalog {
    contract: ToolContract,
    tools: Vec<ToolRegistration>,
}

impl ToolCatalog {
    fn builtins(directory: &std::path::Path, image_input: Option<bool>) -> Vec<ToolRegistration> {
        vec![
            ToolRegistration::new(ToolDefinition {
                name: "view_image".into(),
                mode: ToolMode::Asynchronous,
                source: ToolSource::Builtin,
                strict: true,
                display_source: None,
                output_schema: None,
                description: format!("Read one existing local static PNG or JPEG and return its actual image content to the model, with safe metadata. Required JSON: {{\"path\":\"image.png\"}}. Relative paths resolve in fixed session directory {}; absolute paths use local Windows rules. Detects format from bytes, not extension. Does not capture screenshots, click, edit, generate or OCR images. At most 10 MiB input, 25 million pixels, longest sent edge 2048 px (proportional resize), 4 MiB encoded result, 16 MiB retained images per session and 32 MiB serialized model request. Immutable image bytes remain in history even if the original file changes or is deleted; use a new call to read new content. Model image capability from catalog: {}. Errors preserve existing history, without changing model or account.", directory.display(), match image_input { Some(true)=>"supported", Some(false)=>"unsupported", None=>"unknown; actual request determines support" }),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            }, move |context, arguments, cancel| Box::pin(execution::image(context, arguments, cancel, image_input))).shared_read(),
            ToolRegistration::new(ToolDefinition {
                name: "apply_patch".into(),
                mode: ToolMode::Synchronous,
                source: ToolSource::Builtin,
                strict: true,
                display_source: None,
                output_schema: None,
                description: format!("Modify UTF-8 text using a JSON object with required patch and workdir (string or null). Null workdir uses {}; relative workdir resolves there. Paths may be absolute or relative to workdir. Example: {{\"patch\":\"*** Begin Patch\\n*** Add File: hello.txt\\n+hello\\n*** End Patch\\n\",\"workdir\":null}}. Supports *** Add File:, *** Delete File:, *** Update File:, optional *** Move to: immediately after Update, @@ or @@ exact-anchor, context lines prefixed by space, - removals, + additions, and *** End of File after a chunk. Context must match exactly and uniquely. Read changed files before retrying. At most 64 actions, 8 MiB per file, 32 MiB prepared text. Preserves existing UTF-8 BOM and uniform LF/CRLF; new files use LF without BOM. Validates the whole patch before writing. Commits files separately, with actual action and temporary-file reports; errors or Stop preserve completed changes and never roll back. Does not overwrite existing add/move destinations.", directory.display()),
                parameters: json!({"type":"object","properties":{"patch":{"type":"string"},"workdir":{"type":["string","null"]}},"required":["patch","workdir"],"additionalProperties":false}),
            }, patch).text_result(),
            ToolRegistration::process(ToolDefinition {
                name: "exec_command".into(),
                mode: ToolMode::Asynchronous,
                source: ToolSource::Builtin,
                strict: true,
                display_source: None,
                output_schema: None,
                description: format!("Execute PowerShell 7 using UTF-8 pipes and no profile. workdir null uses fixed session directory {}; relative values resolve there. yield_time_ms null waits 10000 ms; 0..30000 is allowed. {SHELL_LIFETIME} Runs with current-user access, without a sandbox or PTY.", directory.display()),
                parameters: json!({"type":"object","properties":{
                    "cmd":{"type":"string"},"workdir":{"type":["string","null"]},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000}
                },"required":["cmd","workdir","yield_time_ms"],"additionalProperties":false}),
            }, Executor::Shell, ToolAccess::Exclusive),
            ToolRegistration::process(ToolDefinition {
                name: "write_stdin".into(),
                mode: ToolMode::Synchronous,
                source: ToolSource::Builtin,
                strict: true,
                display_source: None,
                output_schema: None,
                description: format!("{STDIN_SCOPE} Empty chars only polls for new output; nonempty chars writes exactly those UTF-8 bytes, without adding a newline. close_stdin closes input after this write (EOF). terminate kills and reaps that process group, and cannot be combined with nonempty chars or close_stdin. yield_time_ms null waits 10000 ms; range 0..30000. Output is consumed, never repeated; no automatic input retry. Nonzero exit codes are preserved. {STDIN_EXPIRATION} Pipes are not window keyboard input or a terminal."),
                parameters: json!({"type":"object","properties":{
                    "session_id":{"type":"string"},"chars":{"type":"string"},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000},
                    "close_stdin":{"type":"boolean"},"terminate":{"type":"boolean"}
                },"required":["session_id","chars","yield_time_ms","close_stdin","terminate"],"additionalProperties":false}),
            }, Executor::Stdin, ToolAccess::ProcessControl),
        ]
    }

    pub fn with_extensions<'a>(
        directory: &std::path::Path,
        image_input: Option<bool>,
        contract: ToolContract,
        codex_images: bool,
        extensions: impl IntoIterator<Item = &'a ToolRegistration>,
    ) -> Result<Self, ToolFailure> {
        let mut tools = Self::builtins(directory, image_input);
        if codex_images {
            let mut registration = ToolRegistration::process(
                ToolDefinition {
                    name: crate::GENERATE_IMAGE_TOOL.into(),
                    description: "Generate one image from a text prompt using GPT Image 2 when the user asks for an image. Saves the original as a conversation attachment and returns its metadata and local path. Editing and reference images are unavailable.".into(),
                    parameters: json!({"type":"object","properties":{"prompt":{"type":"string"}},"required":["prompt"],"additionalProperties":false}),
                    source: ToolSource::Builtin,
                    strict: true,
                    mode: ToolMode::Asynchronous,
                    display_source: Some("GPT Image 2".into()),
                    output_schema: None,
                },
                Executor::CodexImage,
                ToolAccess::SharedRead,
            );
            registration.image_result = image_input != Some(false);
            tools.push(registration);
        }
        if !contract.is_flat() {
            tools.extend(
                [
                    crate::TaskControl::List,
                    crate::TaskControl::Wait,
                    crate::TaskControl::Cancel,
                ]
                .map(|control| {
                    ToolRegistration::process(
                        control.definition(),
                        Executor::TaskControl(control),
                        ToolAccess::ProcessControl,
                    )
                }),
            );
        }
        for tool in extensions {
            if matches!(tool.definition.source, ToolSource::Builtin) {
                return Err(ToolFailure::new(
                    "invalid_source",
                    "外部工具不能声明内置来源",
                ));
            }
            tools.push(tool.clone());
        }
        let mut names = HashSet::new();
        let mut sources = HashSet::new();
        for tool in &mut tools {
            let definition = &mut tool.definition;
            if contract.is_flat() {
                definition.mode = ToolMode::Synchronous;
            }
            validate_definition(definition)?;
            if !names.insert(&definition.name) {
                return Err(ToolFailure::new(
                    "duplicate_name",
                    format!("重复的工具名称：{}", definition.name),
                ));
            }
            if !matches!(definition.source, ToolSource::Builtin)
                && !sources.insert(&definition.source)
            {
                return Err(ToolFailure::new(
                    "duplicate_source",
                    format!("重复的工具来源：{}", definition.name),
                ));
            }
        }
        Ok(Self { contract, tools })
    }

    pub fn contract(&self) -> ToolContract {
        self.contract
    }

    pub fn definition(&self, name: &str) -> Option<&ToolDefinition> {
        Some(&self.find(name)?.definition)
    }

    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &ToolDefinition> {
        self.tools.iter().map(|tool| &tool.definition)
    }

    pub fn inherited(
        &self,
        allowed: &std::collections::BTreeSet<String>,
        model_contract: ToolContract,
        image_input: Option<bool>,
    ) -> Self {
        let contract = if self.contract == model_contract {
            self.contract
        } else {
            ToolContract::Flat
        };
        let tools = self
            .tools
            .iter()
            .filter(|tool| {
                allowed.contains(&tool.definition.name)
                    && !(contract.is_flat() && matches!(&tool.executor, Executor::TaskControl(_)))
                    && !(image_input == Some(false) && tool.definition.name == "view_image")
            })
            .cloned()
            .map(|mut tool| {
                if contract.is_flat() {
                    tool.definition.mode = ToolMode::Synchronous;
                }
                if image_input == Some(false) {
                    tool.image_result = false;
                }
                tool
            })
            .collect();
        Self { contract, tools }
    }

    pub fn rebind(&self, saved: &[ToolDefinition]) -> Result<Self, ToolFailure> {
        let mut seen = HashSet::new();
        let mut tools = Vec::with_capacity(saved.len());
        for definition in saved {
            if !seen.insert(&definition.name) {
                return Err(ToolFailure::new(
                    "duplicate_name",
                    format!("存档工具名称重复：{}", definition.name),
                ));
            }
            let registration = self
                .find(&definition.name)
                .filter(|tool| tool.can_rebind(definition))
                .ok_or_else(|| {
                    ToolFailure::new(
                        "tool_contract",
                        format!("工具来源或声明与存档不匹配：{}", definition.name),
                    )
                })?;
            tools.push(ToolRegistration {
                definition: definition.clone(),
                executor: registration.executor.clone(),
                access: registration.access,
                image_result: registration.image_result,
            });
        }
        if self.tools.iter().any(|tool| {
            matches!(tool.definition.source, ToolSource::Builtin)
                && !matches!(
                    tool.executor,
                    Executor::TaskControl(_) | Executor::CodexImage
                )
                && !seen.contains(&tool.definition.name)
        }) {
            return Err(ToolFailure::new("tool_contract", "存档缺少内置工具契约"));
        }
        Ok(Self {
            contract: self.contract,
            tools,
        })
    }

    pub fn task_control(&self, name: &str) -> Option<crate::TaskControl> {
        match self.find(name)?.executor {
            Executor::TaskControl(control) => Some(control),
            _ => None,
        }
    }

    pub fn is_codex_image(&self, name: &str) -> bool {
        self.find(name)
            .is_some_and(|tool| matches!(tool.executor, Executor::CodexImage))
    }

    pub fn access(&self, name: &str) -> ToolAccess {
        self.find(name)
            .map_or(ToolAccess::Exclusive, |registration| registration.access)
    }

    pub fn may_return_image(&self, name: &str) -> bool {
        self.find(name)
            .is_some_and(|registration| registration.image_result)
    }

    pub fn display_source(&self, name: &str) -> Option<&str> {
        self.find(name)?.definition.display_source.as_deref()
    }

    pub(crate) fn find(&self, name: &str) -> Option<&ToolRegistration> {
        self.tools.iter().find(|tool| tool.definition.name == name)
    }
}

fn validate_definition(definition: &ToolDefinition) -> Result<(), ToolFailure> {
    let name = &definition.name;
    if name.is_empty()
        || name.len() > MAX_MODEL_NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ToolFailure::new(
            "invalid_name",
            format!("工具名称无效：{name}"),
        ));
    }
    if let ToolSource::Extension { provider, tool } = &definition.source
        && (provider.trim().is_empty() || tool.trim().is_empty())
    {
        return Err(ToolFailure::new(
            "invalid_source",
            format!("工具提供者或原始名称为空：{name}"),
        ));
    }
    let schema = &definition.parameters;
    let properties = schema.get("properties").and_then(Value::as_object);
    if schema.get("type").and_then(Value::as_str) != Some("object") || properties.is_none() {
        return Err(ToolFailure::new(
            "invalid_schema",
            format!("工具需要包含 properties 的对象 Schema：{name}"),
        ));
    }
    if definition.strict {
        let required = schema.get("required").and_then(Value::as_array);
        let all_fields_required = properties
            .zip(required)
            .is_some_and(|(properties, required)| {
                properties.len() == required.len()
                    && properties
                        .keys()
                        .all(|key| required.iter().any(|name| name.as_str() == Some(key)))
            });
        if !all_fields_required
            || schema.get("additionalProperties").and_then(Value::as_bool) != Some(false)
        {
            return Err(ToolFailure::new(
                "invalid_schema",
                format!("严格工具 Schema 必须列出所有必填字段并关闭额外字段：{name}"),
            ));
        }
    }
    Ok(())
}

fn patch<'a>(
    context: &'a ExecutionContext,
    arguments: &'a str,
    cancel: &'a CancellationToken,
) -> ToolExecution<'a> {
    Box::pin(crate::patch::run(
        arguments,
        context.directory.clone(),
        cancel,
    ))
}
