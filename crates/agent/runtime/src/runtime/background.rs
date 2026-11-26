use super::{
    AgentEvent, AgentEventKind, DeliveryFailure, ModelControl, ModelTask, SharedModel, TaskRequest,
    creation, loading, session_execution::Owner, tool_jobs,
};
use agentelier_account::CredentialClient;
use agentelier_conversation::{
    BACKGROUND_RESULT_TOOL, BackgroundBinding, BackgroundOutcome, BackgroundStatus, BackgroundTask,
    ConversationEvent, ConversationFailure, ConversationRequest, SessionId, SessionView, TurnId,
};
use agentelier_llm_model::CodexIdentity;
use agentelier_memory::MemoryClient;
use agentelier_session_store::SessionStore;
use agentelier_tools::{
    ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const MANAGE: &str = "manage_background_task";
const BRIEF_LIMIT: usize = 8 * 1024;
const SUMMARY_LIMIT: usize = 2048;

pub(super) fn hidden_lifecycle(event: &AgentEvent) -> bool {
    match &event.kind {
        AgentEventKind::Conversation(
            ConversationEvent::Created(view)
            | ConversationEvent::CreatedForSend(view)
            | ConversationEvent::Opened(view)
            | ConversationEvent::Accepted(view)
            | ConversationEvent::Started(view)
            | ConversationEvent::Updated(view)
            | ConversationEvent::Finished(view)
            | ConversationEvent::IdleUpdated(view),
        ) => view.settings.background.is_some(),
        _ => false,
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Start,
    List,
    Continue,
    Stop,
    Delete,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    action: Action,
    task: Option<SessionId>,
    title: Option<String>,
    brief: Option<String>,
    authorization: Option<String>,
}

pub(super) struct Invocation {
    pub(super) source: SessionId,
    turn: TurnId,
    args: Arguments,
    cancel: CancellationToken,
    reply: oneshot::Sender<ToolResult>,
}

fn definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
        source: ToolSource::Extension {
            provider: "agentelier.background".into(),
            tool: name.into(),
        },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    }
}

pub(super) fn registrations(commands: mpsc::Sender<Invocation>) -> [ToolRegistration; 2] {
    let manage = definition(
        MANAGE,
        "Manage independent background work from the ordinary Project chat that commissioned it. Start only for a current user's explicit background delegation. List includes completed work. Continue, stop or delete by the returned task UUID and only when the user explicitly requests the operation. Quote the exact current user message in authorization for every change. Use null for unused fields. The response confirms admission or cancellation request, not task completion.",
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["start","list","continue","stop","delete"]},
            "task":{"type":["string","null"]},
            "title":{"type":["string","null"]},
            "brief":{"type":["string","null"]},
            "authorization":{"type":["string","null"]}
        },"required":["action","task","title","brief","authorization"],"additionalProperties":false}),
    );
    let result = definition(
        BACKGROUND_RESULT_TOOL,
        "In a background session, record the verified outcome before ending the turn. Set completed to true only when the whole delegated task is done and results are checked. Otherwise use false and explain what the user must provide, decide or verify. The summary must be grounded in actual results and is shown to the originating chat.",
        json!({"type":"object","properties":{
            "completed":{"type":"boolean"},
            "summary":{"type":"string","maxLength":SUMMARY_LIMIT}
        },"required":["completed","summary"],"additionalProperties":false}),
    );
    [
        ToolRegistration::with_origin(manage, move |context, raw, cancel, origin| {
            let commands = commands.clone();
            Box::pin(async move {
                let args: Arguments = match serde_json::from_str(raw) {
                    Ok(args) => args,
                    Err(failure) => return rejected(&failure.to_string()),
                };
                let user = context.user_turn();
                if origin.session != user.session || origin.turn != user.turn || user.memory_management || user.background {
                    return rejected("后台任务操作需要当前普通会话身份");
                }
                if args.action != Action::List
                    && !args.authorization.as_ref().is_some_and(|authorization| {
                        authorizes(args.action, authorization)
                            && user.statements.iter().any(|statement| statement.trim() == authorization.trim())
                    })
                {
                    return rejected("请引用当前用户明确授权的完整输入");
                }
                let (reply, receipt) = oneshot::channel();
                let invocation = Invocation { source: user.session, turn: TurnId(user.turn), args, reply, cancel: cancel.clone() };
                tokio::select! { biased;
                    _ = cancel.cancelled() => return rejected("后台任务操作在接纳前取消"),
                    result = commands.send(invocation) => if result.is_err() { return rejected("后台任务入口已关闭"); }
                }
                tokio::select! { biased;
                    result = receipt => result.unwrap_or_else(|_| unconfirmed()),
                    _ = cancel.cancelled() => unconfirmed(),
                }
            })
        }).text_result(),
        ToolRegistration::with_origin(result, |context, raw, _, _| {
            Box::pin(async move {
                if !context.user_turn().background {
                    return rejected("只有后台 Session 可以记录委托结果");
                }
                match serde_json::from_str::<BackgroundOutcome>(raw) {
                    Ok(outcome) if !outcome.summary.trim().is_empty() && outcome.summary.len() <= SUMMARY_LIMIT => {
                        success(json!({"recorded":true,"completed":outcome.completed}))
                    }
                    _ => rejected("后台任务结果需要非空且有界的摘要"),
                }
            })
        }).text_result(),
    ]
}

fn rejected(reason: &str) -> ToolResult {
    ToolResult::not_executed("background_rejected", reason)
}

fn authorizes(action: Action, statement: &str) -> bool {
    let text = statement.to_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| text.contains(term));
    match action {
        Action::Start => {
            has(&["后台", "background"])
                && has(&[
                    "请", "帮", "执行", "启动", "继续", "做", "跑", "start", "run", "perform",
                ])
                && !has(&[
                    "不要在后台",
                    "别启动后台",
                    "do not start",
                    "don't start",
                    "do not run in the background",
                ])
        }
        Action::Continue => {
            has(&["继续", "接着", "处理", "续接", "continue", "resume"])
                && !has(&[
                    "不要继续",
                    "别继续",
                    "don't continue",
                    "do not continue",
                    "do not resume",
                ])
        }
        Action::Stop => {
            has(&["停止", "取消", "暂停", "stop", "cancel"])
                && !has(&[
                    "不要停",
                    "不要取消",
                    "别停",
                    "don't stop",
                    "do not stop",
                    "do not cancel",
                ])
        }
        Action::Delete => {
            has(&["删除", "清理", "delete", "remove"])
                && !has(&[
                    "不要删",
                    "别删",
                    "不要清理",
                    "don't delete",
                    "do not delete",
                    "do not remove",
                ])
        }
        Action::List => true,
    }
}
fn unconfirmed() -> ToolResult {
    ToolResult::error(
        ToolStatus::Unconfirmed,
        "background_unconfirmed",
        "未收到后台任务操作回执，请查询已保存状态",
    )
}

fn success(value: Value) -> ToolResult {
    ToolResult {
        status: ToolStatus::Succeeded,
        output: value.to_string().into(),
        cleanup_failed: false,
    }
}

pub(super) struct Environment<'a, P> {
    pub(super) data_root: &'a Path,
    pub(super) store: &'a SessionStore,
    pub(super) credentials: &'a CredentialClient,
    pub(super) shared: &'a SharedModel,
    pub(super) source: Option<&'a CodexIdentity>,
    pub(super) mcp: &'a agentelier_mcp::Connections,
    pub(super) memory: Option<MemoryClient>,
    pub(super) skills: &'a agentelier_skills::UserSkills,
    pub(super) operations: &'a crate::OperationSequence,
    pub(super) ecs: &'a crate::ecs_access::EcsAccess,
    pub(super) commands: &'a mpsc::Sender<Invocation>,
    pub(super) publish: &'a P,
}

impl Invocation {
    pub(super) async fn run<P>(
        self,
        tasks: Vec<BackgroundTask>,
        owners: &mut HashMap<Option<SessionId>, Owner>,
        env: Environment<'_, P>,
    ) -> bool
    where
        P: Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    {
        let removed = self.args.action == Action::Delete;
        let result = self.handle(tasks, owners, &env).await;
        let changed = removed && result.is_ok();
        let result = match result {
            Ok(value) => success(value),
            Err(failure) => rejected(&failure.to_string()),
        };
        drop(self.reply.send(result));
        changed
    }

    async fn handle<P>(
        &self,
        tasks: Vec<BackgroundTask>,
        owners: &mut HashMap<Option<SessionId>, Owner>,
        env: &Environment<'_, P>,
    ) -> Result<Value, ConversationFailure>
    where
        P: Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    {
        if self.cancel.is_cancelled() {
            return Err(ConversationFailure::new("后台任务操作在接纳前取消"));
        }
        let parent = owners
            .get(&Some(self.source))
            .and_then(|owner| owner.conversation.current())
            .filter(|view| view.settings.kind.is_user() && view.settings.background.is_none())
            .filter(|view| view.turn.as_ref().is_some_and(|turn| turn.id == self.turn))
            .ok_or_else(|| ConversationFailure::new("后台任务只接受发起聊天当前轮次的请求"))?;
        if self.args.action == Action::List {
            return Ok(json!({"tasks": tasks}));
        }
        let source = env
            .source
            .ok_or_else(|| ConversationFailure::new("当前账号无法接纳后台任务"))?;
        parent.settings.validate_source(source)?;
        if self.args.action == Action::Start {
            return self.start(parent, owners, env).await;
        }
        let task = self
            .args
            .task
            .and_then(|id| tasks.iter().find(|task| task.session == id))
            .ok_or_else(|| ConversationFailure::new("任务不属于此聊天，或任务 ID 无效"))?;
        match self.args.action {
            Action::Continue => {
                let project = env
                    .store
                    .session_project(self.source)
                    .await
                    .map_err(failure)?;
                if project.is_none()
                    || env
                        .store
                        .session_project(task.session)
                        .await
                        .map_err(failure)?
                        != project
                {
                    return Err(ConversationFailure::new(
                        "后台任务与发起聊天的项目归属不一致",
                    ));
                }
                let brief = checked_brief(self.args.brief.as_deref())?;
                let key = Some(task.session);
                if let std::collections::hash_map::Entry::Vacant(entry) = owners.entry(key) {
                    let mut owner = env.owner()?;
                    let id = env.operations.allocate().map_err(failure)?;
                    let mut pending = loading::Loading::start_background(
                        id,
                        task.session,
                        self.source,
                        env.store,
                    )
                    .map_err(failure)?;
                    let result = pending.wait().await;
                    let event =
                        pending.complete(result, env.operations, &mut owner.conversation, env.mcp);
                    match event {
                        ConversationEvent::Opened(view)
                            if view
                                .settings
                                .background
                                .as_ref()
                                .is_some_and(|binding| binding.source == self.source) =>
                        {
                            entry.insert(owner);
                        }
                        ConversationEvent::ArchiveFailed(reason) => {
                            return Err(ConversationFailure::new(reason));
                        }
                        _ => return Err(ConversationFailure::new("无法恢复关联后台会话")),
                    }
                }
                let owner = owners
                    .get_mut(&key)
                    .ok_or_else(|| ConversationFailure::new("后台会话未恢复"))?;
                let view = owner
                    .conversation
                    .current()
                    .ok_or_else(|| ConversationFailure::new("后台会话不可用"))?;
                view.settings.validate_source(source)?;
                let original = &view
                    .settings
                    .background
                    .as_ref()
                    .ok_or_else(|| ConversationFailure::new("后台任务缺少保存的委托"))?
                    .brief;
                let input = if matches!(task.status, BackgroundStatus::Pending) {
                    format!("Original authorized task: {original}\n\nFollow-up: {brief}")
                } else {
                    format!(
                        "Continue from the saved task state. Verify existing effects before repeating work.\n\nUser-authorized follow-up: {brief}"
                    )
                };
                env.start_turn(owner, task.session, input)?;
                Ok(json!({"task":task.session,"accepted":true}))
            }
            Action::Stop => {
                let owner = owners
                    .get(&Some(task.session))
                    .ok_or_else(|| ConversationFailure::new("该任务当前没有可停止的执行"))?;
                if let Some(model) = &owner.model {
                    model.control.cancel.cancel();
                } else if let Some((_, tools)) =
                    owner.tools.as_ref().filter(|(_, tools)| tools.busy())
                {
                    tools.work.lock().await.stop_all();
                } else {
                    return Err(ConversationFailure::new("任务没有活动执行"));
                }
                Ok(json!({"task":task.session,"stop_requested":true}))
            }
            Action::Delete => {
                if task.status == BackgroundStatus::Running
                    || owners
                        .get(&Some(task.session))
                        .is_some_and(|owner| !owner.ready_to_remove())
                {
                    return Err(ConversationFailure::new("请先停止任务，等待执行和保存收尾"));
                }
                let commit = env
                    .store
                    .client()
                    .delete(task.session)
                    .map_err(failure)?
                    .await
                    .map_err(failure)?
                    .map_err(failure)?;
                owners.remove(&Some(task.session));
                let cleanup_error = commit.cleanup_error.map(|failure| failure.to_string());
                super::emit(
                    env.publish,
                    0,
                    AgentEventKind::Conversation(ConversationEvent::Deleted {
                        session: task.session,
                        cleanup_error: cleanup_error.clone(),
                        was_open: false,
                    }),
                )
                .map_err(failure)?;
                Ok(json!({"task":task.session,"deleted":true,"cleanup_error":cleanup_error}))
            }
            Action::List | Action::Start => Err(ConversationFailure::new("后台任务操作状态异常")),
        }
    }

    async fn start<P>(
        &self,
        parent: SessionView,
        owners: &mut HashMap<Option<SessionId>, Owner>,
        env: &Environment<'_, P>,
    ) -> Result<Value, ConversationFailure>
    where
        P: Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    {
        let project = env
            .store
            .session_project(parent.id)
            .await
            .map_err(failure)?
            .ok_or_else(|| ConversationFailure::new("请在 Project 内的普通聊天委托后台任务"))?;
        let title =
            agentelier_conversation::session_title(self.args.title.as_deref().unwrap_or(""))
                .map_err(failure)?;
        let brief = checked_brief(self.args.brief.as_deref())?;
        let authorization = checked_brief(self.args.authorization.as_deref())?;
        let brief = format!(
            "Original user authorization and scope:\n{authorization}\n\nTask brief within that scope:\n{brief}"
        );
        let definitions = owners
            .get(&Some(parent.id))
            .ok_or_else(|| ConversationFailure::new("发起会话已卸载"))?
            .conversation
            .tool_definitions()
            .cloned()
            .collect::<Vec<_>>();
        let mut settings = *parent.settings;
        settings.background = Some(BackgroundBinding {
            source: self.source,
            source_turn: self.turn,
            title,
            brief: brief.clone(),
        });
        let settings = creation::freeze_origin(settings, env.data_root).await?;
        let mut owner = env.owner()?;
        let available = env.mcp.restore_registrations(&definitions);
        let input = format!(
            "Carry out this explicitly delegated background task. Record the verified result with record_background_result before ending.\n\n{brief}"
        );
        let prepared = owner.conversation.prepare(settings, &available, None)?;
        prepared.check_first_input(&input)?;
        let id = env.operations.allocate().map_err(failure)?;
        let (control, _, _) = ModelControl::new(id);
        let mut pending =
            creation::Creation::start(id, control, prepared, Some(project), env.store)?;
        let reply = pending.wait().await;
        let (event, _) = pending.complete(reply, &mut owner.conversation);
        let session = match &event {
            ConversationEvent::Created(view) => view.id,
            ConversationEvent::Rejected(reason) => {
                return Err(ConversationFailure::new(reason.clone()));
            }
            _ => return Err(ConversationFailure::new("后台会话创建未返回确认结果")),
        };
        super::emit(env.publish, 0, AgentEventKind::Conversation(event)).map_err(failure)?;
        let started = env.start_turn(&mut owner, session, input);
        owners.insert(Some(session), owner);
        if let Err(reason) = started {
            return Ok(
                json!({"task":session,"saved":true,"accepted":false,"reason":reason.to_string()}),
            );
        }
        Ok(json!({"task":session,"saved":true,"accepted":true}))
    }
}

impl<P: Fn(AgentEvent) -> Result<(), DeliveryFailure>> Environment<'_, P> {
    fn owner(&self) -> Result<Owner, ConversationFailure> {
        Owner::new(
            self.credentials,
            self.data_root,
            self.memory.clone(),
            self.skills,
            self.commands,
        )
        .map_err(failure)
    }

    fn start_turn(
        &self,
        owner: &mut Owner,
        session: SessionId,
        input: String,
    ) -> Result<(), ConversationFailure> {
        if owner.busy() {
            return Err(ConversationFailure::new("后台会话已有活动轮次"));
        }
        let id = self.operations.allocate().map_err(failure)?;
        let (control, steers, task_actions) = ModelControl::new(id);
        let view = owner.conversation.start(
            id,
            ConversationRequest::Send {
                session,
                input,
                audio: None,
                transcript: None,
            },
        )?;
        let tools = tool_jobs::SessionTools::for_session(
            &mut owner.tools,
            &view,
            &owner.conversation,
            self.store.client(),
            self.ecs,
        );
        control
            .bind_root(&mut owner.conversation, &view)
            .map_err(failure)?;
        self.shared
            .lock()
            .map_err(failure)?
            .insert(id, control.clone());
        super::emit(
            self.publish,
            id,
            AgentEventKind::Conversation(ConversationEvent::Accepted(view)),
        )
        .map_err(failure)?;
        let (progress, receiver) = mpsc::channel(super::PROGRESS_CAPACITY);
        owner.progress = Some(receiver);
        owner.model = Some(ModelTask::start(
            id,
            control,
            self.credentials.clone(),
            TaskRequest::Chat(
                progress,
                steers,
                task_actions,
                self.data_root.join("skills"),
                self.memory.clone(),
                tools,
            ),
        ));
        Ok(())
    }
}

fn checked_brief(brief: Option<&str>) -> Result<&str, ConversationFailure> {
    brief
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= BRIEF_LIMIT)
        .ok_or_else(|| ConversationFailure::new("后台任务需要非空且有界的委托说明"))
}

fn failure(error: impl std::fmt::Display) -> ConversationFailure {
    ConversationFailure::new(error.to_string())
}
