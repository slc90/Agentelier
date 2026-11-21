use super::child_runtime::{ChildTurn, FinishedChild};
use super::mailbox::{self, MailboxActivity};
use super::{ModelTask, RuntimeFailure};
use agentelier_account::{CredentialClient, ExecutionIdentity};
use agentelier_conversation::{
    AgentCreationRecord, AgentId, AgentIdentity, AgentMessage, AgentMessageId, AgentNode,
    AgentState, ChildPreparation, ChildRegistration, Conversation, ConversationFailure,
    DeliveryMode, ForkMode, MAX_AGENT_MESSAGE_CHARS, MailboxEvent, PreparedChild, RecordEvent,
    SessionId, TurnId,
};
use agentelier_llm_model::{ModelInfo, ProtocolArchive};
use agentelier_prompts::{Library, RoleSnapshot};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const TOOL: &str = "spawn_agent";
const MAX_TITLE_CHARS: usize = 128;

mod inbox;
mod interrupt;
mod wake;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    task: String,
    task_name: Option<String>,
    role: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    fork: Option<ForkMode>,
}

pub(super) struct Invocation {
    origin: ProcessOrigin,
    prepared: Result<PreparedInvocation, ToolResult>,
    cancel: CancellationToken,
    reply: oneshot::Sender<ToolResult>,
}

#[derive(Clone)]
pub(super) struct PreparedInvocation {
    arguments: Arguments,
    role: RoleSnapshot,
    identity: ExecutionIdentity,
    models: Vec<ModelInfo>,
}

impl PreparedInvocation {
    pub(super) fn prepare(
        &self,
        conversation: &mut Conversation,
        operation: u64,
        origin: &ProcessOrigin,
    ) -> Result<PreparedChild, ConversationFailure> {
        let active = conversation.active_turn(operation)?;
        if active.session_id() != origin.session || active.turn_id()? != origin.turn {
            return Err(ConversationFailure::new("Spawn 来源不属于当前执行轮次"));
        }
        active.prepare_child(
            ChildPreparation {
                request: origin.request,
                call_id: &origin.call_id,
                fork: self.arguments.fork.unwrap_or_default(),
                model: self.arguments.model.as_deref(),
                effort: self.arguments.reasoning_effort.as_deref(),
                role: &self.role,
            },
            &self.models,
            self.identity.identity(),
        )
    }
}

pub(super) struct Preparation {
    pub(super) origin: ProcessOrigin,
    pub(super) spec: PreparedInvocation,
    pub(super) reply: oneshot::Sender<Result<PreparedChild, ConversationFailure>>,
}

pub(super) fn registration(
    requests: mpsc::Sender<Invocation>,
    credentials: CredentialClient,
    library: PathBuf,
) -> ToolRegistration {
    let definition = ToolDefinition {
        name: TOOL.into(),
        description: "Start an independent child Agent with an assigned task. Set unused options to null. Fresh context includes the task and current project instructions. Full history requires a closed parent request history. Success returns the stable Agent ID, path, and starting status.".into(),
        parameters: json!({"type":"object","properties":{
            "task":{"type":"string","maxLength":MAX_AGENT_MESSAGE_CHARS},
            "task_name":{"type":["string","null"],"maxLength":MAX_TITLE_CHARS},
            "role":{"type":["string","null"]},
            "model":{"type":["string","null"]},
            "reasoning_effort":{"type":["string","null"]},
            "fork":{"anyOf":[{"type":"string","enum":["fresh_context","full_history"]},{"type":"null"}]}
        },"required":["task","task_name","role","model","reasoning_effort","fork"],"additionalProperties":false}),
        source: ToolSource::Extension { provider: "agentelier.collaboration".into(), tool: TOOL.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::with_origin(definition, move |_, raw, cancel, origin| {
        let library = library.clone();
        let credentials = credentials.clone();
        let requests = requests.clone();
        Box::pin(invoke(raw, cancel, origin, requests, credentials, library))
    })
    .text_result()
}

async fn invoke(
    raw: &str,
    cancel: &CancellationToken,
    origin: ProcessOrigin,
    requests: mpsc::Sender<Invocation>,
    credentials: CredentialClient,
    library: PathBuf,
) -> ToolResult {
    let prepare = async {
        let mut arguments: Arguments = serde_json::from_str(raw).map_err(rejection)?;
        arguments.task = arguments.task.trim().to_owned();
        if arguments.task.is_empty()
            || arguments.task.chars().count() > MAX_AGENT_MESSAGE_CHARS
            || arguments
                .task_name
                .as_ref()
                .is_some_and(|name| name.chars().count() > MAX_TITLE_CHARS)
        {
            return Err(rejection("子 Agent 的任务或名称超出约定范围"));
        }
        let requested_role = arguments.role.clone();
        let role = tokio::task::spawn_blocking(move || {
            Library::load(&library, &Library::default())?.resolve_role(requested_role.as_deref())
        })
        .await
        .map_err(rejection)?
        .map_err(rejection)?;
        let identity = credentials.execution().await.map_err(rejection)?;
        let models = if arguments.model.is_some()
            || arguments.reasoning_effort.is_some()
            || role.model.is_some()
            || role.reasoning.is_some()
        {
            agentelier_llm_model::task_catalog(&credentials, &identity)
                .await
                .map_err(rejection)?
        } else {
            Vec::new()
        };
        Ok(PreparedInvocation {
            arguments,
            role,
            identity,
            models,
        })
    };
    let prepared = tokio::select! { biased;
        _ = cancel.cancelled() => return ToolResult::not_executed("spawn_cancelled", "Spawn 已在提交前取消"),
        result = prepare => result,
    };
    let (reply, wait) = oneshot::channel();
    let invocation = Invocation {
        origin,
        prepared,
        cancel: cancel.clone(),
        reply,
    };
    let queued = tokio::select! { biased;
        _ = cancel.cancelled() => return ToolResult::not_executed("spawn_cancelled", "Spawn 已在提交前取消"),
        result = requests.send(invocation) => result,
    };
    if queued.is_err() {
        return ToolResult::not_executed("spawn_closed", "Agent Control 已停止接纳");
    }
    let result = tokio::select! { biased;
        result = wait => result,
        _ = cancel.cancelled() => return unknown("Spawn 执行被取消，创建回执待核对"),
    };
    match result {
        Ok(result) => result,
        Err(failure) => unknown(failure),
    }
}

#[derive(Default)]
pub(super) struct Children {
    pub(super) jobs: JoinSet<FinishedChild>,
    stop: CancellationToken,
    active: HashMap<AgentId, ChildHandle>,
    mailbox: MailboxActivity,
    parked: HashMap<AgentId, Box<Conversation>>,
    waiters: HashMap<AgentId, CancellationToken>,
    finishing: HashMap<AgentId, FinishedChild>,
}

struct ChildHandle {
    thread: SessionId,
    turn: TurnId,
    commands: mpsc::Sender<Preparation>,
    stop: CancellationToken,
    cleanup_unconfirmed: bool,
}

#[derive(Clone, Copy)]
pub(super) struct SpawnEnvironment<'a> {
    pub(super) store: &'a SessionStoreClient,
    pub(super) credentials: &'a CredentialClient,
    pub(super) operations: &'a crate::OperationSequence,
    pub(super) skills_root: &'a Path,
    pub(super) mailbox_commands: &'a mpsc::Sender<mailbox::Command>,
}

impl Children {
    pub(super) fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    pub(super) fn steer_wait(&self, agent: AgentId) {
        if let Some(waiter) = self.waiters.get(&agent) {
            waiter.cancel();
        }
    }

    pub(super) fn clear_wait(&mut self, agent: AgentId) {
        self.waiters.remove(&agent);
    }

    pub(super) fn cancel_active(&self) {
        for child in self.active.values() {
            child.stop.cancel();
        }
    }

    pub(super) async fn spawn(
        &mut self,
        invocation: Invocation,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        environment: SpawnEnvironment<'_>,
    ) {
        let result = self
            .admit(&invocation, conversation, root_task, environment)
            .await;
        drop(
            invocation
                .reply
                .send(result.unwrap_or_else(|failure| failure)),
        );
    }

    async fn admit(
        &mut self,
        invocation: &Invocation,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        environment: SpawnEnvironment<'_>,
    ) -> Result<ToolResult, ToolResult> {
        let origin = &invocation.origin;
        let view = conversation
            .current()
            .ok_or_else(|| rejection("Spawn 缺少所属 Session"))?;
        let session = view.id;
        let turn = TurnId(origin.turn);
        let (parent, root_operation, child, parent_stop) = if origin.session == session {
            let task = root_task
                .filter(|task| task.control.matches_turn(session, origin.turn))
                .ok_or_else(|| rejection("Spawn 的 Root 任务已结束或不属于当前会话"))?;
            task.check_instance(conversation).map_err(rejection)?;
            (
                AgentId::for_root(session),
                Some(task.id),
                None,
                task.control.cancel.clone(),
            )
        } else {
            let (&parent, child) = self
                .active
                .iter()
                .find(|(_, child)| child.thread == origin.session && child.turn == turn)
                .ok_or_else(|| rejection("Spawn 来源子 Agent 未执行或已关闭"))?;
            (parent, None, Some(child), child.stop.clone())
        };
        if let Some(node) =
            conversation
                .agent_registry()
                .spawned_by(session, parent, turn, &origin.call_id)
        {
            return Ok(success(node));
        }
        if view.execution_blocked {
            return Err(unknown("当前会话的存档提交未确认，请重新打开核对"));
        }
        let spec = invocation.prepared.as_ref().map_err(Clone::clone)?;
        if invocation.cancel.is_cancelled()
            || spec.identity.is_invalidated()
            || self.stop.is_cancelled()
            || parent_stop.is_cancelled()
        {
            return Err(ToolResult::not_executed(
                "spawn_cancelled",
                "Spawn 已在提交前取消",
            ));
        }
        let id = AgentId::allocate();
        let request = &spec.arguments;
        let reservation = conversation
            .agent_registry_mut()
            .reserve_child(
                session,
                parent,
                ChildRegistration {
                    id,
                    segment: format!("agent-{id}"),
                    nickname: request
                        .task_name
                        .clone()
                        .unwrap_or_else(|| spec.role.id.clone()),
                    role: spec.role.id.clone(),
                    turn,
                    call_id: origin.call_id.clone(),
                },
            )
            .map_err(rejection)?;
        let identity = reservation.identity().clone();
        let prepared = if let Some(operation) = root_operation {
            spec.prepare(conversation, operation, origin)
        } else if let Some(child) = child {
            let (reply, result) = oneshot::channel();
            match child
                .commands
                .send(Preparation {
                    origin: origin.clone(),
                    spec: spec.clone(),
                    reply,
                })
                .await
            {
                Ok(()) => result.await.unwrap_or_else(|_| {
                    Err(ConversationFailure::new("父 Agent 未返回创建准备结果"))
                }),
                Err(_) => Err(ConversationFailure::new("父 Agent 已退出，不能准备 Spawn")),
            }
        } else {
            Err(ConversationFailure::new("Spawn 缺少父 Agent 执行体"))
        };
        let prepared = match prepared {
            Ok(prepared)
                if !invocation.cancel.is_cancelled() && !spec.identity.is_invalidated() =>
            {
                prepared
            }
            result => {
                conversation
                    .agent_registry_mut()
                    .release(reservation)
                    .map_err(rejection)?;
                return Err(rejection(result.err().map_or_else(
                    || "Spawn 已在提交前取消".into(),
                    |failure| failure.to_string(),
                )));
            }
        };
        let creation = AgentCreationRecord {
            identity: identity.clone(),
            task: request.task.clone(),
            config: prepared.config.clone(),
            history: prepared.history.archive(),
        };
        if let Err(failure) = creation.validate(session) {
            conversation
                .agent_registry_mut()
                .release(reservation)
                .map_err(rejection)?;
            return Err(rejection(failure));
        }
        let store = environment.store;
        let pending = match store.try_append(RecordEvent::AgentCreated {
            session,
            creation: Box::new(creation),
        }) {
            Ok(pending) => pending,
            Err(failure) => {
                conversation
                    .agent_registry_mut()
                    .release(reservation)
                    .map_err(rejection)?;
                return Err(rejection(failure));
            }
        };
        let failure = match pending.await {
            Ok(Ok(commit)) if commit.session == session => None,
            Ok(Ok(_)) => Some("Spawn 提交回执属于其他会话".into()),
            Ok(Err(failure)) => Some(failure.to_string()),
            Err(failure) => Some(failure.to_string()),
        };
        if let Some(failure) = failure {
            tracing::error!(session_id = %session, agent_id = %id, agent_path = %identity.path, turn_id = origin.turn, call_id = %origin.call_id, error = %failure, "子 Agent 身份提交未确认");
            let message = format!("Spawn 身份提交未确认，请重新打开会话核对存档：{failure}");
            conversation.fail_storage(message.clone());
            return Err(unknown(message));
        }
        conversation
            .agent_registry_mut()
            .commit(reservation)
            .map_err(unknown)?;
        tracing::info!(session_id = %session, agent_id = %id, agent_path = %identity.path, turn_id = origin.turn, call_id = %origin.call_id, "子 Agent 身份已持久登记");
        let message = AgentMessage {
            id: AgentMessageId::initial(&identity),
            sender: parent,
            receiver: id,
            turn,
            call_id: origin.call_id.clone(),
            mode: DeliveryMode::TriggerTurn,
            text: request.task.clone(),
        };
        self.mailbox
            .commit(
                conversation,
                store,
                session,
                MailboxEvent::Queued {
                    message: message.clone(),
                },
            )
            .await
            .map_err(unknown)?;
        let starting = if invocation.cancel.is_cancelled()
            || spec.identity.is_invalidated()
            || parent_stop.is_cancelled()
        {
            Err(AgentState::Interrupted)
        } else {
            environment
                .operations
                .allocate()
                .map_err(|failure| ConversationFailure::new(failure.to_string()))
                .and_then(|operation| {
                    ChildTurn::new(&identity, prepared, message.text.clone(), operation)
                })
                .map_err(|failure| AgentState::Failed(failure.to_string()))
        };
        match starting {
            Err(state) => apply_execution(store, conversation, &identity, state, None)
                .await
                .map_err(unknown)?,
            Ok(turn) => self
                .launch_turn(
                    conversation,
                    identity,
                    turn,
                    &[message],
                    parent_stop,
                    environment,
                )
                .await
                .map_err(unknown)?,
        }
        conversation
            .agent_registry()
            .get(session, id)
            .map(success)
            .map_err(unknown)
    }

    pub(super) async fn finish(
        &mut self,
        finished: Result<FinishedChild, tokio::task::JoinError>,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
    ) -> Result<Option<AgentId>, RuntimeFailure> {
        let finished = finished.map_err(|failure| {
            ConversationFailure::new(format!("子 Agent 执行体无法回收：{failure}"))
        })?;
        let session = finished.identity.session;
        self.finishing.insert(finished.identity.id, finished);
        let mut wake = None;
        while let Some(id) = self.finishing.keys().copied().find(|&id| {
            conversation
                .agent_registry()
                .get(session, id)
                .is_ok_and(|node| {
                    node.state != AgentState::Stopping
                        || conversation
                            .agent_registry()
                            .subtree(session, id)
                            .is_ok_and(|mut descendants| {
                                !descendants.any(|other| {
                                    other.identity.id != id
                                        && self.active.contains_key(&other.identity.id)
                                })
                            })
                })
        }) {
            let finished = self
                .finishing
                .remove(&id)
                .ok_or_else(|| ConversationFailure::new("待收尾的子 Agent 已丢失"))?;
            let node = conversation
                .agent_registry()
                .get(session, id)
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
            if node.state == AgentState::Stopping && finished.residence.is_none() {
                if let Some(child) = self.active.get_mut(&id) {
                    child.cleanup_unconfirmed = true;
                }
                tracing::error!(session_id = %session, agent_id = %id, agent_path = %node.identity.path, state = ?finished.state, "子 Agent 中断收尾未确认，保留活动占用");
                continue;
            }
            if let Some(agent) = self.commit_finish(finished, store, conversation).await? {
                wake = Some(agent);
            }
        }
        Ok(wake)
    }

    async fn commit_finish(
        &mut self,
        finished: FinishedChild,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
    ) -> Result<Option<AgentId>, RuntimeFailure> {
        let identity = &finished.identity;
        self.clear_wait(identity.id);
        let cancelled = self
            .active
            .get(&identity.id)
            .is_some_and(|child| child.stop.is_cancelled());
        for claim in finished.consumed {
            self.mailbox
                .commit(
                    conversation,
                    store,
                    identity.session,
                    MailboxEvent::Consumed { claim },
                )
                .await?;
        }
        let resume =
            finished.residence.is_some() && !cancelled && finished.state != AgentState::Interrupted;
        apply_execution(
            store,
            conversation,
            identity,
            finished.state,
            finished.output,
        )
        .await?;
        conversation
            .agent_registry_mut()
            .detach(identity.session, identity.id, finished.instance)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.active.remove(&identity.id);
        if let Some(residence) = finished.residence {
            self.parked.insert(identity.id, residence);
        }
        Ok(resume.then_some(identity.id))
    }

    pub(super) async fn stop(
        &mut self,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
    ) -> Result<(), RuntimeFailure> {
        self.stop.cancel();
        conversation.mailboxes_mut().close();
        let mut first_failure = None;
        while let Some(result) = self.jobs.join_next().await {
            if let Err(failure) = self.finish(result, store, conversation).await {
                first_failure.get_or_insert(failure);
            }
        }
        if self.has_active() {
            first_failure.get_or_insert_with(|| {
                ConversationFailure::new("部分子 Agent 尚无可确认的收尾结果").into()
            });
        }
        first_failure.map_or(Ok(()), Err)
    }
}

async fn apply_execution(
    store: &SessionStoreClient,
    conversation: &mut Conversation,
    identity: &AgentIdentity,
    state: AgentState,
    output: Option<ProtocolArchive>,
) -> Result<(), ConversationFailure> {
    let result = store
        .append(RecordEvent::AgentExecution {
            session: identity.session,
            agent: identity.id,
            state: state.clone(),
            request: None,
            output,
        })
        .await;
    if !result
        .as_ref()
        .is_ok_and(|commit| commit.session == identity.session)
    {
        let message = result.err().map_or_else(
            || "子 Agent 提交回执会话不匹配".into(),
            |failure| failure.to_string(),
        );
        conversation.fail_storage(message.clone());
        return Err(ConversationFailure::new(message));
    }
    conversation
        .agent_registry_mut()
        .set_state(identity.session, identity.id, state.clone())
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    tracing::debug!(session_id = %identity.session, agent_id = %identity.id, agent_path = %identity.path, ?state, "子 Agent 执行状态已提交");
    Ok(())
}

fn success(node: &AgentNode) -> ToolResult {
    let mut output = json!({"agent_id":node.identity.id, "path":node.identity.path, "state":node.state.name(), "nickname":node.identity.nickname});
    if let AgentState::Failed(reason) = &node.state {
        output["reason"] = json!(reason);
    }
    ToolResult {
        status: ToolStatus::Succeeded,
        cleanup_failed: false,
        output: output.to_string().into(),
    }
}

fn rejection(failure: impl std::fmt::Display) -> ToolResult {
    ToolResult::not_executed("spawn_rejected", &failure.to_string())
}

fn unknown(failure: impl std::fmt::Display) -> ToolResult {
    ToolResult::error(
        ToolStatus::Unconfirmed,
        "spawn_unconfirmed",
        &failure.to_string(),
    )
}
