use super::{ModelTask, RuntimeFailure};
use agentelier_account::{CredentialClient, ExecutionIdentity};
use agentelier_conversation::{
    AgentCreationRecord, AgentId, AgentIdentity, AgentNode, AgentRuntimeId, AgentState,
    ChildPreparation, ChildRegistration, Conversation, ConversationFailure, ForkMode,
    PreparedChild, RecordEvent, TurnId,
};
use agentelier_llm_model::{GenerationConnection, GenerationRequest, ModelInfo, ProtocolArchive};
use agentelier_prompts::{Library, RoleSnapshot};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const TOOL: &str = "spawn_agent";
const MAX_TASK_CHARS: usize = 16_384;
const MAX_TITLE_CHARS: usize = 128;

#[derive(Deserialize)]
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
    prepared: Result<SpawnSpec, ToolResult>,
    cancel: CancellationToken,
    reply: oneshot::Sender<ToolResult>,
}

struct SpawnSpec {
    arguments: Arguments,
    role: RoleSnapshot,
    identity: ExecutionIdentity,
    models: Vec<ModelInfo>,
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
            "task":{"type":"string","maxLength":MAX_TASK_CHARS},
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
            || arguments.task.chars().count() > MAX_TASK_CHARS
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
        Ok(SpawnSpec {
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
}

pub(super) struct FinishedChild {
    identity: AgentIdentity,
    instance: AgentRuntimeId,
    state: AgentState,
    output: Option<ProtocolArchive>,
}

impl Children {
    pub(super) async fn spawn(
        &mut self,
        invocation: Invocation,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        store: &SessionStoreClient,
        credentials: &CredentialClient,
    ) {
        let result = self
            .admit(&invocation, conversation, root_task, store, credentials)
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
        store: &SessionStoreClient,
        credentials: &CredentialClient,
    ) -> Result<ToolResult, ToolResult> {
        let origin = &invocation.origin;
        let (session, turn) = (origin.session, TurnId(origin.turn));
        let parent = AgentId::for_root(session);
        let root_task = root_task
            .filter(|task| task.control.matches_turn(session, origin.turn))
            .ok_or_else(|| rejection("Spawn 的父任务已结束或不属于当前会话"))?;
        root_task.check_instance(conversation).map_err(rejection)?;
        if let Some(node) =
            conversation
                .agent_registry()
                .spawned_by(session, parent, turn, &origin.call_id)
        {
            return Ok(success(node));
        }
        if conversation
            .current()
            .is_some_and(|view| view.execution_blocked)
        {
            return Err(unknown("当前会话的存档提交未确认，请重新打开核对"));
        }
        let spec = invocation.prepared.as_ref().map_err(Clone::clone)?;
        if invocation.cancel.is_cancelled()
            || spec.identity.is_invalidated()
            || self.stop.is_cancelled()
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
        let prepared = conversation.active_turn(root_task.id).and_then(|active| {
            active.prepare_child(
                ChildPreparation {
                    request: origin.request,
                    call_id: &origin.call_id,
                    fork: request.fork.unwrap_or_default(),
                    model: request.model.as_deref(),
                    effort: request.reasoning_effort.as_deref(),
                    role: &spec.role,
                },
                &spec.models,
                spec.identity.identity(),
            )
        });
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
        let starting = if invocation.cancel.is_cancelled() || spec.identity.is_invalidated() {
            Err(AgentState::Interrupted)
        } else {
            initial_generation(prepared, &request.task)
                .map_err(|failure| AgentState::Failed(failure.to_string()))
        };
        match starting {
            Err(state) => apply_execution(store, conversation, &identity, state, None, None)
                .await
                .map_err(unknown)?,
            Ok((initial, model, effort)) => {
                let instance = conversation
                    .agent_registry_mut()
                    .attach(session, id)
                    .map_err(unknown)?;
                if let Err(failure) = apply_execution(
                    store,
                    conversation,
                    &identity,
                    AgentState::Running,
                    Some(initial.projection().clone()),
                    None,
                )
                .await
                {
                    conversation
                        .agent_registry_mut()
                        .detach(session, id, instance)
                        .map_err(unknown)?;
                    return Err(unknown(failure));
                }
                let credentials = credentials.clone();
                let authorization = spec.identity.clone();
                let stop = self.stop.child_token();
                tracing::info!(session_id = %session, agent_id = %id, agent_path = %identity.path, model, effort, role = %identity.role, "子 Agent 已启动独立模型请求");
                self.jobs.spawn(run_child(
                    identity,
                    instance,
                    initial,
                    credentials,
                    authorization,
                    stop,
                ));
            }
        }
        conversation
            .agent_registry()
            .get(session, id)
            .map(success)
            .map_err(unknown)
    }

    pub(super) async fn finish(
        finished: Result<FinishedChild, tokio::task::JoinError>,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
    ) -> Result<(), RuntimeFailure> {
        let finished = finished.map_err(|failure| {
            ConversationFailure::new(format!("子 Agent 执行体无法回收：{failure}"))
        })?;
        let identity = &finished.identity;
        conversation
            .agent_registry_mut()
            .detach(identity.session, identity.id, finished.instance)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        apply_execution(
            store,
            conversation,
            identity,
            finished.state,
            None,
            finished.output,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn stop(
        &mut self,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
    ) -> Result<(), RuntimeFailure> {
        self.stop.cancel();
        let mut first_failure = None;
        while let Some(result) = self.jobs.join_next().await {
            if let Err(failure) = Self::finish(result, store, conversation).await {
                first_failure.get_or_insert(failure);
            }
        }
        first_failure.map_or(Ok(()), Err)
    }
}

fn initial_generation(
    prepared: PreparedChild,
    task: &str,
) -> Result<
    (
        agentelier_llm_model::PreparedGeneration,
        String,
        Option<String>,
    ),
    ConversationFailure,
> {
    let settings = prepared.config.settings;
    let history_position = prepared.history.position();
    let compact_threshold = settings
        .context_limits
        .compact_threshold()
        .ok_or_else(|| ConversationFailure::new("子 Agent 模型缺少压缩阈值"))?;
    let skill_catalog = prepared
        .config
        .skills
        .render()
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    let generation = agentelier_llm_model::prepare(GenerationRequest {
        memory: None,
        transcript: None,
        selected_skills: Vec::new(),
        skill_catalog: Some(skill_catalog),
        slug: settings.slug.clone(),
        compact_threshold,
        skip_compaction: false,
        reserved_result_bytes: 0,
        effort: settings.effort.clone(),
        context: prepared.history.with_user(task),
        history_position,
        instructions: Arc::new(settings.instructions),
        tools: prepared.tools,
    })
    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    Ok((generation, settings.slug, settings.effort))
}

async fn run_child(
    identity: AgentIdentity,
    instance: AgentRuntimeId,
    generation: agentelier_llm_model::PreparedGeneration,
    credentials: CredentialClient,
    authorization: ExecutionIdentity,
    cancel: CancellationToken,
) -> FinishedChild {
    let (progress, mut incoming) = mpsc::channel(super::model_task::PROGRESS_CAPACITY);
    let future = agentelier_llm_model::generate(
        GenerationConnection::default(),
        credentials,
        generation,
        &authorization,
        progress,
    );
    tokio::pin!(future);
    let (state, output) = loop {
        tokio::select! { biased;
            _ = cancel.cancelled() => break (AgentState::Interrupted, None),
            result = &mut future => break match result {
                Ok((result, _)) if result.calls.is_empty() => (AgentState::Completed, Some(result.output.archive())),
                Ok(_) => (AgentState::Failed("初始子 Agent 执行入口收到需要独立工具循环的模型调用".into()), None),
                Err(failure) => (AgentState::Failed(failure.to_string()), None),
            },
            Some(_) = incoming.recv() => {}
        }
    };
    FinishedChild {
        identity,
        instance,
        state,
        output,
    }
}

async fn apply_execution(
    store: &SessionStoreClient,
    conversation: &mut Conversation,
    identity: &AgentIdentity,
    state: AgentState,
    request: Option<agentelier_llm_model::RequestProjection>,
    output: Option<ProtocolArchive>,
) -> Result<(), ConversationFailure> {
    let result = store
        .append(RecordEvent::AgentExecution {
            session: identity.session,
            agent: identity.id,
            state: state.clone(),
            request: request.map(Box::new),
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
    let state = match node.state {
        AgentState::Starting => "starting",
        AgentState::Running => "running",
        AgentState::Waiting => "waiting",
        AgentState::Completed => "completed",
        AgentState::Interrupted => "interrupted",
        AgentState::Failed(_) => "failed",
        AgentState::Closed => "closed",
    };
    let mut output = json!({"agent_id":node.identity.id, "path":node.identity.path, "state":state, "nickname":node.identity.nickname});
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
