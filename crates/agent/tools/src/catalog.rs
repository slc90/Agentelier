use crate::{ExecutionContext, ToolFailure, ToolResult, execution};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

const MAX_MODEL_NAME_BYTES: usize = 64;

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
}

impl ToolDefinition {
    pub fn catalog_description(&self) -> String {
        match &self.source {
            ToolSource::Mcp { server, tool, .. } => format!(
                "MCP {server} · {tool} · strict={}\n{}\n{}",
                self.strict, self.description, self.parameters
            ),
            _ => self.description.clone(),
        }
    }
}

fn strict_by_default() -> bool {
    true
}

pub type ToolExecution<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;
type Executor = dyn for<'a> Fn(&'a mut ExecutionContext, &'a str, &'a CancellationToken) -> ToolExecution<'a>
    + Send
    + Sync;

#[derive(Clone)]
pub struct ToolRegistration {
    definition: ToolDefinition,
    executor: Arc<Executor>,
}

impl ToolRegistration {
    pub fn new(
        definition: ToolDefinition,
        executor: impl for<'a> Fn(
            &'a mut ExecutionContext,
            &'a str,
            &'a CancellationToken,
        ) -> ToolExecution<'a>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            definition,
            executor: Arc::new(executor),
        }
    }

    pub(crate) fn execute<'a>(
        &'a self,
        context: &'a mut ExecutionContext,
        arguments: &'a str,
        cancel: &'a CancellationToken,
    ) -> ToolExecution<'a> {
        let execution = (self.executor)(context, arguments, cancel);
        if matches!(self.definition.source, ToolSource::Builtin) {
            return execution;
        }
        Box::pin(async move {
            let result = execution.await;
            if result.output.summary().len() <= crate::MAX_TOOL_BODY_BYTES {
                return result;
            }
            let mut failure = ToolResult::error(
                crate::ToolStatus::Failed,
                "result_too_large",
                "工具结果超过大小限制",
            );
            failure.cleanup_failed = result.cleanup_failed;
            failure
        })
    }
}

#[derive(Default)]
pub struct ToolCatalog(Vec<ToolRegistration>);

impl ToolCatalog {
    fn builtins(directory: &std::path::Path, image_input: Option<bool>) -> Vec<ToolRegistration> {
        vec![
            ToolRegistration::new(ToolDefinition {
                name: "view_image".into(),
                source: ToolSource::Builtin,
                strict: true,
                description: format!("Read one existing local static PNG or JPEG and return its actual image content to the model, with safe metadata. Required JSON: {{\"path\":\"image.png\"}}. Relative paths resolve in fixed session directory {}; absolute paths use local Windows rules. Detects format from bytes, not extension. Does not capture screenshots, click, edit, generate or OCR images. At most 10 MiB input, 25 million pixels, longest sent edge 2048 px (proportional resize), 4 MiB encoded result, 16 MiB retained images per session and 32 MiB serialized model request. Immutable image bytes remain in history even if the original file changes or is deleted; use a new call to read new content. Model image capability from catalog: {}. Errors preserve existing history, without changing model or account.", directory.display(), match image_input { Some(true)=>"supported", Some(false)=>"unsupported", None=>"unknown; actual request determines support" }),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            }, move |context, arguments, cancel| Box::pin(execution::image(context, arguments, cancel, image_input))),
            ToolRegistration::new(ToolDefinition {
                name: "apply_patch".into(),
                source: ToolSource::Builtin,
                strict: true,
                description: format!("Modify UTF-8 text using a JSON object with required patch and workdir (string or null). Null workdir uses {}; relative workdir resolves there. Paths may be absolute or relative to workdir. Example: {{\"patch\":\"*** Begin Patch\\n*** Add File: hello.txt\\n+hello\\n*** End Patch\\n\",\"workdir\":null}}. Supports *** Add File:, *** Delete File:, *** Update File:, optional *** Move to: immediately after Update, @@ or @@ exact-anchor, context lines prefixed by space, - removals, + additions, and *** End of File after a chunk. Context must match exactly and uniquely. Read changed files before retrying. At most 64 actions, 8 MiB per file, 32 MiB prepared text. Preserves existing UTF-8 BOM and uniform LF/CRLF; new files use LF without BOM. Validates the whole patch before writing. Commits files separately, with actual action and temporary-file reports; errors or Stop preserve completed changes and never roll back. Does not overwrite existing add/move destinations.", directory.display()),
                parameters: json!({"type":"object","properties":{"patch":{"type":"string"},"workdir":{"type":["string","null"]}},"required":["patch","workdir"],"additionalProperties":false}),
            }, patch),
            ToolRegistration::new(ToolDefinition {
                name: "exec_command".into(),
                source: ToolSource::Builtin,
                strict: true,
                description: format!("Execute PowerShell 7 using UTF-8 pipes and no profile. workdir null uses fixed session directory {}; relative values resolve there. yield_time_ms null waits 10000 ms; 0..30000 is allowed. If still running, returns a task-scoped session_id for write_stdin and new output. No command lifetime limit. At most 8 unreaped sessions. All processes are reaped when this user task ends; old IDs cannot be used in a later task. Runs with current-user access, without a sandbox or PTY.", directory.display()),
                parameters: json!({"type":"object","properties":{
                    "cmd":{"type":"string"},"workdir":{"type":["string","null"]},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000}
                },"required":["cmd","workdir","yield_time_ms"],"additionalProperties":false}),
            }, exec),
            ToolRegistration::new(ToolDefinition {
                name: "write_stdin".into(),
                source: ToolSource::Builtin,
                strict: true,
                description: "Continue a process session from this user task. Empty chars only polls for new output; nonempty chars writes exactly those UTF-8 bytes, without adding a newline. close_stdin closes input after this write (EOF). terminate kills and reaps that process group, and cannot be combined with nonempty chars or close_stdin. yield_time_ms null waits 10000 ms; range 0..30000. Output is consumed, never repeated; no automatic input retry. Nonzero exit codes are preserved. Sessions expire at task end. Pipes are not window keyboard input or a terminal.".into(),
                parameters: json!({"type":"object","properties":{
                    "session_id":{"type":"string"},"chars":{"type":"string"},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000},
                    "close_stdin":{"type":"boolean"},"terminate":{"type":"boolean"}
                },"required":["session_id","chars","yield_time_ms","close_stdin","terminate"],"additionalProperties":false}),
            }, stdin),
        ]
    }

    pub fn with_extensions<'a>(
        directory: &std::path::Path,
        image_input: Option<bool>,
        extensions: impl IntoIterator<Item = &'a ToolRegistration>,
    ) -> Result<Self, ToolFailure> {
        let mut tools = Self::builtins(directory, image_input);
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
        for tool in &tools {
            let definition = &tool.definition;
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
        Ok(Self(tools))
    }

    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &ToolDefinition> {
        self.0.iter().map(|tool| &tool.definition)
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
                .filter(|tool| tool.definition == *definition)
                .ok_or_else(|| {
                    ToolFailure::new(
                        "tool_contract",
                        format!("工具来源或声明与存档不匹配：{}", definition.name),
                    )
                })?;
            tools.push(registration.clone());
        }
        if self
            .definitions()
            .any(|tool| matches!(tool.source, ToolSource::Builtin) && !seen.contains(&tool.name))
        {
            return Err(ToolFailure::new("tool_contract", "存档缺少内置工具契约"));
        }
        Ok(Self(tools))
    }

    pub(crate) fn find(&self, name: &str) -> Option<&ToolRegistration> {
        self.0.iter().find(|tool| tool.definition.name == name)
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

fn exec<'a>(
    context: &'a mut ExecutionContext,
    arguments: &'a str,
    cancel: &'a CancellationToken,
) -> ToolExecution<'a> {
    Box::pin(context.exec(arguments, cancel))
}

fn stdin<'a>(
    context: &'a mut ExecutionContext,
    arguments: &'a str,
    cancel: &'a CancellationToken,
) -> ToolExecution<'a> {
    Box::pin(context.stdin(arguments, cancel))
}

fn patch<'a>(
    context: &'a mut ExecutionContext,
    arguments: &'a str,
    cancel: &'a CancellationToken,
) -> ToolExecution<'a> {
    Box::pin(crate::patch::run(
        arguments,
        context.directory.clone(),
        cancel,
    ))
}
