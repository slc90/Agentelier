use super::model_task::{self, ModelTask, TaskRequest};
use super::spawn_agent::Preparation;
use super::{AgentEventKind, ModelControl};
use agentelier_account::CredentialClient;
use agentelier_conversation::{
    AgentIdentity, AgentMessage, AgentRuntimeId, AgentState, Conversation, ConversationEvent,
    ConversationFailure, ConversationRequest, MessageClaim, PreparedChild, RecordEvent,
    SessionView, TurnId, TurnStatus,
};
use agentelier_llm_model::ProtocolArchive;
use agentelier_session_store::SessionStoreClient;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) struct ChildTurn {
    conversation: Conversation,
    view: SessionView,
    operation: u64,
}

impl ChildTurn {
    pub(super) fn new(
        identity: &AgentIdentity,
        prepared: PreparedChild,
        task: String,
        operation: u64,
    ) -> Result<Self, ConversationFailure> {
        let thread = identity.thread.session_id();
        let mut conversation = Conversation::from_child(thread, prepared)?;
        let view = Self::start(&mut conversation, thread, task, operation)?;
        Ok(Self::from_started(conversation, view, operation))
    }

    pub(super) fn start(
        conversation: &mut Conversation,
        thread: agentelier_conversation::SessionId,
        task: String,
        operation: u64,
    ) -> Result<SessionView, ConversationFailure> {
        conversation.start(
            operation,
            ConversationRequest::Send {
                session: thread,
                input: task,
                audio: None,
                transcript: None,
            },
        )
    }

    pub(super) fn from_started(
        conversation: Conversation,
        view: SessionView,
        operation: u64,
    ) -> Self {
        Self {
            conversation,
            view,
            operation,
        }
    }

    pub(super) fn view(&self) -> &SessionView {
        &self.view
    }

    pub(super) fn turn_id(&self) -> Result<TurnId, ConversationFailure> {
        self.view
            .turn
            .as_ref()
            .map(|turn| turn.id)
            .ok_or_else(|| ConversationFailure::new("子 Agent 接纳任务时缺少 Turn ID"))
    }
}

pub(super) struct Execution {
    pub(super) identity: AgentIdentity,
    pub(super) instance: AgentRuntimeId,
    pub(super) turn: ChildTurn,
    pub(super) store: SessionStoreClient,
    pub(super) credentials: CredentialClient,
    pub(super) ecs_access: crate::ecs_access::EcsAccess,
    pub(super) skills_root: PathBuf,
    pub(super) stop: CancellationToken,
    pub(super) parent_stop: CancellationToken,
    pub(super) commands: mpsc::Receiver<Preparation>,
    pub(super) mailbox: tokio::sync::watch::Receiver<u64>,
    pub(super) claims: Vec<MessageClaim>,
    pub(super) assignment: AgentMessage,
    pub(super) mailbox_commands: mpsc::Sender<super::mailbox::Command>,
}

pub(super) struct FinishedChild {
    pub(super) identity: AgentIdentity,
    pub(super) instance: AgentRuntimeId,
    pub(super) state: AgentState,
    pub(super) output: Option<ProtocolArchive>,
    pub(super) consumed: Vec<MessageClaim>,
    pub(super) residence: Option<Box<Conversation>>,
    pub(super) assignment: Box<AgentMessage>,
    pub(super) result: Option<(TurnId, String)>,
}

impl Execution {
    pub(super) async fn run(self) -> FinishedChild {
        let identity = self.identity.clone();
        let instance = self.instance;
        let claims = self.claims.clone();
        let assignment = Box::new(self.assignment.clone());
        let result = self.execute().await;
        let (state, output, residence, consumed, result) = match result {
            Ok((state, output, conversation, turn, text)) => (
                state,
                output,
                Some(Box::new(conversation)),
                claims,
                Some((turn, text)),
            ),
            Err(failure) => (
                AgentState::Failed(failure.to_string()),
                None,
                None,
                Vec::new(),
                None,
            ),
        };
        FinishedChild {
            identity,
            instance,
            state,
            output,
            consumed,
            residence,
            assignment,
            result,
        }
    }

    async fn execute(
        mut self,
    ) -> Result<
        (
            AgentState,
            Option<ProtocolArchive>,
            Conversation,
            TurnId,
            String,
        ),
        super::RuntimeFailure,
    > {
        let store = self.store.for_agent(&self.identity);
        let ChildTurn {
            mut conversation,
            view,
            operation,
        } = self.turn;
        let identity = &self.identity;
        let tools = super::tool_jobs::SessionTools::for_session(
            &mut None,
            &view,
            &conversation,
            store.clone(),
            &self.ecs_access,
        );
        let (control, steers, task_actions) = ModelControl::new(operation);
        let (progress, incoming) = mpsc::channel(model_task::PROGRESS_CAPACITY);
        let mut task = ModelTask::start(
            operation,
            control,
            self.credentials,
            TaskRequest::Chat(
                progress,
                steers,
                task_actions,
                self.skills_root,
                None,
                tools.clone(),
            ),
        );
        let mut incoming = Some(incoming);
        let internal_only = |_| Ok(());
        let mut first_failure = None;
        let mut stopping = false;
        let result = loop {
            tokio::select! { biased;
                result = &mut task.handle => break result,
                _ = self.stop.cancelled(), if !stopping => {
                    stopping = true;
                    task.control.cancel.cancel();
                }
                _ = self.parent_stop.cancelled(), if !stopping => {
                    stopping = true;
                    self.stop.cancel();
                    task.control.cancel.cancel();
                }
                Some(command) = self.commands.recv() => {
                    let Preparation { origin, spec, reply } = command;
                    let result = if stopping {
                        Err(ConversationFailure::new("父 Agent 已停止接纳 Spawn"))
                    } else {
                        spec.prepare(&mut conversation, operation, &origin)
                    };
                    drop(reply.send(result));
                }
                Ok(()) = self.mailbox.changed() => {
                    let sequence = *self.mailbox.borrow_and_update();
                    tracing::debug!(session_id = %identity.session, agent_id = %identity.id, sequence, "子 Agent 收到 Mailbox 活动通知");
                }
                event = async {
                    match &mut incoming {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                } => match event {
                    Some(agentelier_turn_runner::RunnerEvent::Inbox(request)) => {
                        if let Err(failure) = self.mailbox_commands.try_send(super::mailbox::Command::Inbox(request))
                            && let super::mailbox::Command::Inbox(request) = failure.into_inner() {
                            request.close();
                        }
                    }
                    Some(event) => {
                        if let Err(failure) = model_task::progress(&mut conversation, operation, event, &store, &internal_only) {
                            first_failure.get_or_insert(failure);
                            task.control.cancel.cancel();
                        }
                    }
                    None => incoming = None,
                }
            }
        };
        self.commands.close();
        drop(self.commands);
        if let Err(failure) = model_task::drain_progress(
            &mut conversation,
            operation,
            &mut incoming,
            &store,
            &internal_only,
        ) {
            first_failure.get_or_insert(failure);
        }
        let finished = model_task::complete(&mut conversation, operation, true, result);
        let cleanup = model_task::finish_tools(
            tools,
            "agent_turn_finished",
            &mut conversation,
            &store,
            &internal_only,
        )
        .await;
        if let Some(failure) = first_failure {
            return Err(failure);
        }
        let AgentEventKind::Conversation(ConversationEvent::Finished(view)) = finished? else {
            return Err(ConversationFailure::new("子 Agent Runner 缺少轮次终态").into());
        };
        cleanup?;
        if view.execution_blocked {
            return Err(ConversationFailure::new("子 Agent 会话的执行或结果提交未确认").into());
        }
        let (state, history) = conversation
            .archive()
            .ok_or_else(|| ConversationFailure::new("子 Agent 完成后缺少历史"))?;
        let committed = store
            .append(RecordEvent::Finished {
                state,
                history: history.clone(),
            })
            .await
            .map_err(super::RuntimeFailure::Store)?;
        if committed.session != identity.thread.session_id() {
            return Err(ConversationFailure::new("子 Agent 轮次完成回执身份不一致").into());
        }
        let turn = view
            .turn
            .ok_or_else(|| ConversationFailure::new("子 Agent 缺少已完成轮次"))?;
        tracing::info!(session_id = %identity.session, agent_id = %identity.id, agent_path = %identity.path, thread_id = %identity.thread, turn_id = turn.id.0, operation_id = operation, "子 Agent 轮次已存档并回收");
        let result = match turn.status {
            TurnStatus::Completed => {
                let final_text = turn
                    .messages
                    .iter()
                    .rev()
                    .find(|message| message.phase.as_deref() == Some("final_answer"))
                    .or_else(|| turn.messages.last())
                    .map(|message| {
                        message
                            .content
                            .iter()
                            .map(|content| content.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "Child completed without a text response.".into());
                (AgentState::Completed, history, final_text)
            }
            TurnStatus::Cancelled => (
                AgentState::Interrupted,
                None,
                "Child task was interrupted.".into(),
            ),
            TurnStatus::Failed(reason) => (AgentState::Failed(reason.clone()), None, reason),
            TurnStatus::Limited => (
                AgentState::Failed("子 Agent 达到执行预算".into()),
                None,
                "Child exceeded its execution budget.".into(),
            ),
            TurnStatus::Running => {
                return Err(ConversationFailure::new("子 Agent 收到非终态").into());
            }
        };
        Ok((result.0, result.1, conversation, turn.id, result.2))
    }
}
