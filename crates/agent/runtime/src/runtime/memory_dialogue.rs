use super::creation::Creation;
use super::model_task::{self, ModelJoin, ModelTask, PROGRESS_CAPACITY, TaskRequest};
use super::tool_jobs::SessionTools;
use super::{
    AgentEvent, AgentEventKind, DeliveryFailure, MemoryReply, ModelControl, RuntimeFailure, emit,
};
use agentelier_account::CredentialClient;
use agentelier_conversation::{
    Conversation, ConversationEvent, ConversationFailure, ConversationRequest, RecordEvent,
    SessionCreate, SessionId, SessionKind, SessionLocation, SessionView,
};
use agentelier_llm_model::{CodexIdentity, ModelInfo};
use agentelier_memory::MemoryClient;
use agentelier_prompts::{
    InstructionSlot, InstructionSnapshot, PromptRevision, ResolvedInstruction,
};
use agentelier_session_store::SessionStore;
use agentelier_turn_runner::RunnerEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

const INSTRUCTIONS: &str = "This is the user's global memory management conversation. Use read_memory_summary to check what is currently published when the user asks what you remember. Use search_memory and read_memory only for specific published evidence. A question or ordinary statement is not authorization for a change. If the current user explicitly asks to remember, correct, or forget a specific fact, use revise_memory with scope global and quote the user's exact current instruction, target, and any replacement. Do not rewrite the user's wording or infer consent from history. A saved revision awaits consolidation; only the published summary reflects the latest successful publication. Answer in the user's language.";

pub(super) struct Dialogue {
    credentials: CredentialClient,
    ecs_access: crate::ecs_access::EcsAccess,
    skills: agentelier_skills::UserSkills,
    memory: Option<MemoryClient>,
    conversation: Conversation,
    model: Option<ModelTask>,
    incoming: Option<mpsc::Receiver<RunnerEvent>>,
    tools: Option<(SessionId, SessionTools)>,
}

pub(super) enum Signal {
    Update(Option<RunnerEvent>),
    Finished(ModelJoin),
}

impl Dialogue {
    pub(super) fn new(
        credentials: CredentialClient,
        memory: Option<MemoryClient>,
        ecs_access: crate::ecs_access::EcsAccess,
        skills: agentelier_skills::UserSkills,
    ) -> Self {
        Self {
            credentials,
            ecs_access,
            skills,
            conversation: Conversation::with_extensions(
                agentelier_memory::management_registrations(memory.clone()),
            ),
            memory,
            model: None,
            incoming: None,
            tools: None,
        }
    }

    pub(super) fn current(&self) -> Option<SessionView> {
        self.conversation.current()
    }

    pub(super) fn validate_source(
        &self,
        source: &CodexIdentity,
    ) -> Result<(), ConversationFailure> {
        self.conversation.validate_model_source(source)
    }

    pub(super) async fn open(
        &mut self,
        id: u64,
        source: CodexIdentity,
        models: &[ModelInfo],
        (slug, effort): (String, String),
        data_root: &Path,
        store: &SessionStore,
    ) -> Result<SessionView, ConversationFailure> {
        let mut revision = PromptRevision::custom(
            "记忆管理".into(),
            INSTRUCTIONS.into(),
            "en".into(),
            "agentelier.memory".into(),
        );
        revision.reference.id = "agentelier.memory.management".into();
        revision.readonly = true;
        let directory = data_root.to_owned();
        let create = SessionCreate {
            slug,
            effort,
            location: SessionLocation::Standalone(directory.clone()),
            instructions: Arc::new(InstructionSnapshot {
                base_overridden: false,
                instructions: vec![ResolvedInstruction {
                    slot: InstructionSlot::Supplement,
                    revision,
                }],
            }),
        };
        let mut settings = create.resolve(source, models, directory)?;
        settings.kind = SessionKind::MemoryManagement;
        let prepared = self.conversation.prepare(settings, &[], None)?;
        let (control, _, _) = ModelControl::new(id);
        let mut creation = Creation::start(id, control, prepared, None, store)?;
        let commit = creation.wait().await;
        match creation.complete(commit, &mut self.conversation).0 {
            ConversationEvent::Created(view) => Ok(view),
            ConversationEvent::Rejected(reason) => Err(ConversationFailure::new(reason)),
            _ => Err(ConversationFailure::new("记忆管理会话未返回创建结果")),
        }
    }

    pub(super) fn send(
        &mut self,
        id: u64,
        request: ConversationRequest,
        source: &CodexIdentity,
        store: &SessionStore,
        publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        if self.model.is_some() {
            return Err(ConversationFailure::new("上一条记忆消息仍在处理").into());
        }
        self.conversation.validate_model_source(source)?;
        let view = self.conversation.start(id, request)?;
        let tools = SessionTools::for_session(
            &mut self.tools,
            &view,
            &self.conversation,
            store.client(),
            &self.ecs_access,
            &self.skills,
        );
        let (control, steers, actions) = ModelControl::new(id);
        control.bind_root(&mut self.conversation, &view)?;
        let (sender, receiver) = mpsc::channel(PROGRESS_CAPACITY);
        self.incoming = Some(receiver);
        self.model = Some(ModelTask::start(
            id,
            control,
            self.credentials.clone(),
            TaskRequest::Chat(
                sender,
                steers,
                actions,
                PathBuf::new(),
                self.memory.clone(),
                tools,
            ),
        ));
        emit(publish, id, AgentEventKind::MemoryProgress(Box::new(view)))
    }

    pub(super) async fn next(&mut self) -> Signal {
        let Some(model) = &mut self.model else {
            return std::future::pending().await;
        };
        let incoming = &mut self.incoming;
        tokio::select! { biased;
            event = async {
                match incoming {
                    Some(receive) => receive.recv().await,
                    None => std::future::pending().await,
                }
            } => Signal::Update(event),
            result = &mut model.handle => Signal::Finished(result),
        }
    }

    pub(super) async fn apply(
        &mut self,
        signal: Signal,
        store: &SessionStore,
        publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        let relay = |event| relay_event(event, publish);
        let client = store.client();
        match signal {
            Signal::Update(Some(event)) => {
                let model = self.model.as_ref().ok_or(RuntimeFailure::ModelState)?;
                model.check_instance(&self.conversation)?;
                model_task::progress(
                    &mut self.conversation,
                    model.id,
                    event,
                    &client,
                    &relay,
                    None,
                )
            }
            Signal::Update(None) => {
                self.incoming = None;
                Ok(())
            }
            Signal::Finished(result) => {
                let model = self.model.take().ok_or(RuntimeFailure::ModelState)?;
                model.check_instance(&self.conversation)?;
                model_task::drain_progress(
                    &mut self.conversation,
                    model.id,
                    &mut self.incoming,
                    &client,
                    &relay,
                )?;
                let mut event =
                    model_task::complete(&mut self.conversation, model.id, true, result)?;
                let (state, history) = self
                    .conversation
                    .archive()
                    .ok_or(RuntimeFailure::ModelState)?;
                match client
                    .append(RecordEvent::Finished { state, history })
                    .await
                {
                    Ok(commit) => {
                        self.conversation.settle_tasks(commit.sequence);
                        if let Some(failure) = commit.summary_failure {
                            tracing::error!(operation_id = model.id, %failure, "记忆会话终态已保存，摘要更新失败");
                        }
                    }
                    Err(failure) => {
                        tracing::error!(operation_id = model.id, %failure, "记忆会话终态保存失败");
                        if let Some(view) = self
                            .conversation
                            .fail_storage(format!("记忆会话终态保存失败：{failure}"))
                        {
                            event = AgentEventKind::Conversation(ConversationEvent::Finished(view));
                        }
                    }
                }
                if let AgentEventKind::Conversation(ConversationEvent::Finished(view)) = &event {
                    model_task::settle_root(&mut self.conversation, view, model.id)?;
                }
                model.control.finished.cancel();
                emit(&relay, model.id, event)
            }
        }
    }

    pub(super) async fn stop(
        &mut self,
        reason: &'static str,
        store: &SessionStore,
        publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        let relay = |event| relay_event(event, publish);
        let stopped = model_task::stop_model(
            &mut self.model,
            &mut self.incoming,
            &mut self.conversation,
            store,
            &relay,
        )
        .await;
        let tools = if let Some((_, tools)) = self.tools.take() {
            model_task::finish_tools(
                tools,
                reason,
                &mut self.conversation,
                &store.client(),
                &relay,
            )
            .await
        } else {
            Ok(())
        };
        stopped.and(tools)
    }
}

fn relay_event(
    event: AgentEvent,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), DeliveryFailure> {
    let kind = match event.kind {
        AgentEventKind::Conversation(ConversationEvent::Updated(view)) => {
            AgentEventKind::MemoryProgress(Box::new(view))
        }
        AgentEventKind::Conversation(ConversationEvent::Finished(view)) => {
            AgentEventKind::Memory(Ok(MemoryReply::Chat(Box::new(view))))
        }
        _ => return Ok(()),
    };
    publish(AgentEvent {
        operation_id: event.operation_id,
        kind,
    })
}
