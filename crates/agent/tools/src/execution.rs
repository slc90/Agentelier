use crate::ToolCatalog;
use serde_json::json;
use std::{
    error::Error,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const MAX_TOOL_BODY_BYTES: usize = 64 * 1024;
const ERROR_MESSAGE_BYTES: usize = 8192;

#[derive(Clone)]
pub struct ToolUserTurn {
    pub memory_epoch: Option<i64>,
    pub memory_view: Option<Arc<dyn std::any::Any + Send + Sync>>,
    pub memory_management: bool,
    pub background: bool,
    pub session: uuid::Uuid,
    pub turn: u64,
    pub input: u64,
    pub statements: Vec<String>,
}

pub struct ExecutionContext {
    pub(crate) directory: PathBuf,
    image_input: Option<bool>,
    user_turn: Arc<ToolUserTurn>,
    request: u32,
    root: bool,
    scene_source: Option<SceneSource>,
}

type SceneLookup =
    Pin<Box<dyn Future<Output = Result<Option<crate::ecs_access::SceneStored>, String>> + Send>>;
type SceneSource = Arc<dyn Fn(crate::ecs_access::SceneTarget, u64) -> SceneLookup + Send + Sync>;

impl ExecutionContext {
    pub fn new(
        directory: PathBuf,
        image_input: Option<bool>,
        user_turn: Arc<ToolUserTurn>,
        request: u32,
        root: bool,
    ) -> Self {
        Self {
            directory,
            image_input,
            user_turn,
            request,
            root,
            scene_source: None,
        }
    }

    pub fn user_turn(&self) -> &ToolUserTurn {
        &self.user_turn
    }

    pub fn image_input(&self) -> Option<bool> {
        self.image_input
    }

    pub fn request(&self) -> u32 {
        self.request
    }

    pub fn working_directory(&self) -> &Path {
        &self.directory
    }

    pub fn with_scene_source<F, Fut>(mut self, source: F) -> Self
    where
        F: Fn(crate::ecs_access::SceneTarget, u64) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<crate::ecs_access::SceneStored>, String>>
            + Send
            + 'static,
    {
        self.scene_source = Some(Arc::new(move |target, revision| {
            Box::pin(source(target, revision))
        }));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolStatus {
    Unconfirmed,
    ExternalUnconfirmed,
    Receiving,
    Pending,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    NotExecuted,
}

#[derive(Clone, Debug)]
pub struct ToolFailure {
    pub code: &'static str,
    message: String,
}
impl ToolFailure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for ToolFailure {}

#[derive(Clone)]
pub struct ToolResult {
    pub status: ToolStatus,
    pub output: crate::ToolPayload,
    pub cleanup_failed: bool,
}
impl ToolResult {
    pub fn reject_image_budget(self) -> Self {
        let message = format!(
            "图片已读取，但加入会话将超过 16 MiB 图片预算，未保留图片载荷。读取元数据：{}",
            self.output.summary()
        );
        Self::error(ToolStatus::Failed, "session_image_limit", &message)
    }
    pub fn not_executed(code: &str, message: &str) -> Self {
        Self::error(ToolStatus::NotExecuted, code, message)
    }
    pub fn error(status: ToolStatus, code: &str, message: &str) -> Self {
        let mut end = message.len().min(ERROR_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = &message[..end];
        Self {
            status,
            cleanup_failed: false,
            output: json!({"ok":false,"error":{"code":code,"message":message}})
                .to_string()
                .into(),
        }
    }
}

pub async fn execute(
    catalog: &ToolCatalog,
    context: &ExecutionContext,
    processes: &crate::ProcessSessions,
    scripts: &crate::ScriptSession,
    name: &str,
    arguments: &str,
    cancel: &CancellationToken,
    permit: crate::ToolPermit,
    origin: crate::ProcessOrigin,
) -> ToolResult {
    if cancel.is_cancelled() {
        return ToolResult::not_executed("stopped", "任务已停止，工具未执行");
    }
    if arguments.len() > MAX_TOOL_BODY_BYTES {
        return ToolResult::error(
            ToolStatus::Failed,
            "arguments_too_large",
            "工具参数超过大小限制",
        );
    }
    let Some(tool) = catalog.find(name) else {
        return ToolResult::error(ToolStatus::Failed, "unknown_tool", "未登记的工具名称");
    };
    tool.execute(
        context, processes, scripts, arguments, cancel, permit, origin,
    )
    .await
}

pub(crate) async fn image(
    context: &ExecutionContext,
    arguments: &str,
    cancel: &CancellationToken,
    supported: Option<bool>,
) -> ToolResult {
    if supported == Some(false) {
        return ToolResult::error(
            ToolStatus::Failed,
            "image_capability",
            "模型目录明确不支持图片输入，未读取图片",
        );
    }
    crate::image::run(arguments, context.directory.clone(), cancel).await
}

pub(crate) fn publish_audio(
    context: &ExecutionContext,
    arguments: &str,
    cancel: &CancellationToken,
) -> ToolResult {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Arguments {
        path: String,
    }
    if cancel.is_cancelled() {
        return ToolResult::not_executed("stopped", "任务已停止，音频未发布");
    }
    let args: Arguments = match serde_json::from_str(arguments) {
        Ok(args) => args,
        Err(failure) => {
            return ToolResult::error(ToolStatus::Failed, "audio_arguments", &failure.to_string());
        }
    };
    if args.path.trim().is_empty() {
        return ToolResult::error(ToolStatus::Failed, "audio_path", "需要一个音频文件路径");
    }
    let path = crate::WorkingDirectory::validate(context.working_directory())
        .and_then(|directory| directory.resolve(Path::new(&args.path)));
    match path {
        Ok(path) => ToolResult {
            status: ToolStatus::Succeeded,
            output: crate::ToolPayload::PublishAudio(path),
            cleanup_failed: false,
        },
        Err(failure) => ToolResult::error(ToolStatus::Failed, "audio_path", &failure.to_string()),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SceneOperation {
    Publish,
    Update,
}

pub(crate) async fn prepare_scene(
    context: &ExecutionContext,
    scripts: &crate::ScriptSession,
    arguments: &str,
    cancel: &CancellationToken,
    operation: SceneOperation,
) -> ToolResult {
    use crate::ToolPayload;
    use crate::ecs_access::{
        MAX_SCENE_DEFINITION_BYTES, SceneDefinition, SceneTarget, SceneUpdate,
    };

    if !context.root || context.user_turn().memory_management || context.user_turn().background {
        return ToolResult::error(
            ToolStatus::NotExecuted,
            "scene_owner",
            "原生 Scene 由用户会话的 Root Agent 操作",
        );
    }
    if cancel.is_cancelled() {
        return ToolResult::not_executed("stopped", "任务已停止，Scene 未发布");
    }
    if arguments.len() > MAX_SCENE_DEFINITION_BYTES {
        return ToolResult::error(
            ToolStatus::Failed,
            "scene_size_limit",
            "Scene 定义超过 32 KiB",
        );
    }
    let parsed = match operation {
        SceneOperation::Publish => {
            serde_json::from_str::<Box<SceneDefinition>>(arguments).map(ToolPayload::PublishScene)
        }
        SceneOperation::Update => {
            serde_json::from_str::<Box<SceneUpdate>>(arguments).map(ToolPayload::UpdateScene)
        }
    };
    let mut payload = match parsed {
        Ok(payload) => payload,
        Err(error) => {
            return ToolResult::error(
                ToolStatus::Failed,
                "invalid_scene_props",
                &error.to_string(),
            );
        }
    };
    let (definition, target) = match &mut payload {
        ToolPayload::PublishScene(definition) => (definition, None),
        ToolPayload::UpdateScene(update) => (
            &mut update.definition,
            Some((
                SceneTarget {
                    session: context.user_turn().session,
                    artifact: update.id,
                },
                update.expected_revision,
            )),
        ),
        _ => {
            return ToolResult::error(ToolStatus::Unconfirmed, "scene_route", "Scene 操作缺少定义");
        }
    };
    if let Err(error) = definition.validate() {
        return ToolResult::error(ToolStatus::Failed, error.code, &error.message);
    }
    let stored = match (target, context.scene_source.as_ref()) {
        (Some((target, revision)), Some(source)) => {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return ToolResult::not_executed("stopped", "任务已停止，Scene 未发布"),
                result = source(target, revision) => match result {
                    Ok(stored) => stored,
                    Err(error) => return ToolResult::error(ToolStatus::Failed, "scene_state_unavailable", &error),
                },
            }
        }
        _ => None,
    };
    match scripts
        .validate_scene(
            definition.as_ref().clone(),
            target.map(|(target, _)| target),
            stored,
            cancel,
        )
        .await
    {
        Ok(checked) if !cancel.is_cancelled() => {
            **definition = checked;
            ToolResult {
                status: ToolStatus::Succeeded,
                output: payload,
                cleanup_failed: false,
            }
        }
        Ok(_) => ToolResult::not_executed("stopped", "任务已停止，Scene 未发布"),
        Err(error) => ToolResult::error(
            if cancel.is_cancelled() {
                ToolStatus::Cancelled
            } else {
                ToolStatus::Failed
            },
            error.code,
            &error.to_string(),
        ),
    }
}
