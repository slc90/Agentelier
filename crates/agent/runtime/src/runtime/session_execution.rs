use super::{
    Command, ModelTask, RuntimeFailure, audio_task, mailbox, recording, spawn_agent, task_control,
    tool_jobs,
};
use agentelier_account::{CredentialClient, DeliveryFailure};
use agentelier_conversation::{Conversation, ConversationRequest, SessionId};
use agentelier_session_store::SessionStore;
use agentelier_turn_runner::RunnerEvent;
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::mpsc;

pub(super) struct Owner {
    pub(super) conversation: Conversation,
    pub(super) model: Option<ModelTask>,
    pub(super) tools: Option<(SessionId, tool_jobs::SessionTools)>,
    pub(super) progress: Option<mpsc::Receiver<RunnerEvent>>,
    pub(super) children: spawn_agent::Children,
    pub(super) mailbox_sender: mpsc::Sender<mailbox::Command>,
    pub(super) mailbox: mpsc::Receiver<mailbox::Command>,
    spawns: mpsc::Receiver<spawn_agent::Invocation>,
    pub(super) stopping_root: Option<u64>,
    pub(super) finalizing: bool,
    pub(super) pending_queue: bool,
    pub(super) pending_model: Option<super::session_model::Change>,
    pub(super) records: recording::Records,
    pub(super) receipts: task_control::Receipts,
}

impl Owner {
    pub(super) fn new(
        credentials: &CredentialClient,
        data_root: &Path,
        memory: Option<agentelier_memory::MemoryClient>,
        skills: &agentelier_skills::UserSkills,
        background: &mpsc::Sender<super::background::Invocation>,
    ) -> Result<Self, RuntimeFailure> {
        let (spawn_sender, spawns) = mpsc::channel(super::COMMAND_CAPACITY);
        let (mailbox_sender, mailbox) = mpsc::channel(super::SESSION_COMMAND_CAPACITY);
        let mut extensions = agentelier_memory::registrations(memory);
        extensions.push(spawn_agent::registration(
            spawn_sender,
            credentials.clone(),
            data_root.join("prompts.json"),
        ));
        for mode in [
            agentelier_conversation::DeliveryMode::QueueOnly,
            agentelier_conversation::DeliveryMode::TriggerTurn,
        ] {
            extensions.push(mailbox::registration(mailbox_sender.clone(), mode));
        }
        extensions.push(super::wait_agent::registration(mailbox_sender.clone()));
        extensions.push(super::interrupt_agent::registration(mailbox_sender.clone()));
        extensions.push(super::list_agents::registration(mailbox_sender.clone()));
        extensions.push(super::skill_resource::registration());
        extensions.extend(super::skill_creation::registrations(skills.clone()));
        extensions.extend(super::background::registrations(background.clone()));
        Ok(Self {
            conversation: Conversation::with_extensions(extensions),
            model: None,
            tools: None,
            progress: None,
            children: spawn_agent::Children::with_configured_limits()?,
            mailbox_sender,
            mailbox,
            spawns,
            stopping_root: None,
            finalizing: false,
            pending_queue: false,
            pending_model: None,
            records: recording::Records::new(),
            receipts: task_control::Receipts::new(),
        })
    }

    async fn next(&mut self) -> Result<Command, RuntimeFailure> {
        let work = self.tools.as_ref().map(|(_, tools)| tools.work.clone());
        let root_cancel = self
            .model
            .as_ref()
            .filter(|task| task.chat && self.stopping_root != Some(task.id))
            .map(|task| task.control.cancel.clone());
        let command = tokio::select! {
            _ = async {
                match root_cancel {
                    Some(cancel) => cancel.cancelled().await,
                    None => std::future::pending().await,
                }
            } => Command::RootStopping,
            Some(result) = self.records.join_next(), if !self.records.is_empty() => Command::Recorded(result),
            Some(receipt) = self.receipts.join_next(), if !self.receipts.is_empty() => {
                let (id, receipt) = receipt.map_err(|_| RuntimeFailure::ModelState)?;
                Command::TaskActionFinished(id, receipt)
            },
            changed = async {
                match self.tools.as_mut() {
                    Some((_, tools)) => tools.process_views.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                changed.map_err(|_| RuntimeFailure::ModelState)?;
                Command::ProcessesChanged
            },
            event = async {
                match work {
                    Some(work) => work.lock().await.next().await,
                    None => std::future::pending().await,
                }
            }, if self.model.is_none() && !self.finalizing && !self.pending_queue => {
                Command::IdleTool(event.ok_or(RuntimeFailure::ModelState)?)
            },
            result = async {
                match &mut self.model {
                    Some(task) => (&mut task.handle).await,
                    None => std::future::pending().await,
                }
            } => Command::ModelFinished(result),
            Some(invocation) = self.spawns.recv() => Command::SpawnAgent(Box::new(invocation)),
            Some(command) = self.mailbox.recv() => Command::Mailbox(command),
            Some(result) = self.children.jobs.join_next(), if !self.children.jobs.is_empty() => Command::ChildFinished(result),
            event = async {
                match &mut self.progress {
                    Some(receiver) => receiver.recv().await,
                    None => std::future::pending().await,
                }
            } => Command::Stream(event),
        };
        Ok(command)
    }

    pub(super) fn cancel(&self) {
        if let Some(task) = &self.model {
            task.control.cancel.cancel();
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.model.is_some() || self.finalizing || self.pending_queue
    }

    pub(super) fn ready_to_remove(&self) -> bool {
        !self.busy()
            && !self.children.has_active()
            && self.records.is_empty()
            && self.receipts.is_empty()
            && self.conversation.check_replacement().is_ok()
            && self.tools.as_ref().is_none_or(|(_, tools)| {
                !tools.busy() && !tools.process_views.borrow().has_active()
            })
    }

    pub(super) async fn stop(
        &mut self,
        reason: &'static str,
        store: &SessionStore,
        publish: &impl Fn(super::AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        self.cancel();
        let children = if reason == "application_shutdown" {
            self.children
                .stop(&store.client(), &mut self.conversation, &mut self.mailbox)
                .await
        } else {
            self.children
                .settle_active(
                    &store.client(),
                    &mut self.conversation,
                    Some(&mut self.mailbox),
                )
                .await
        };
        let model = super::stop_model(
            &mut self.model,
            &mut self.progress,
            &mut self.conversation,
            store,
            publish,
        )
        .await;
        let tools = async {
            if let Some((session, tools)) = self.tools.take() {
                let processes = tools.process_views.clone();
                super::model_task::finish_tools(
                    tools,
                    reason,
                    &mut self.conversation,
                    &store.client(),
                    publish,
                )
                .await?;
                let update = self
                    .conversation
                    .retain_closed_processes(session, processes.borrow().clone())?;
                super::emit(
                    publish,
                    0,
                    super::AgentEventKind::Conversation(
                        agentelier_conversation::ConversationEvent::Processes(update),
                    ),
                )?;
            }
            Ok(())
        }
        .await;
        children.and(model).and(tools)
    }
}

pub(super) async fn next(
    owners: &mut HashMap<Option<SessionId>, Owner>,
) -> (Option<SessionId>, Result<Command, RuntimeFailure>) {
    let events: Vec<_> = owners
        .iter_mut()
        .map(|(id, owner)| Box::pin(async move { (*id, owner.next().await) }))
        .collect();
    if events.is_empty() {
        return std::future::pending().await;
    }
    futures_util::future::select_all(events).await.0
}

pub(super) fn target(command: &Command) -> Option<SessionId> {
    match command {
        Command::AudioDraft(_, session, _)
        | Command::TranscribeAudio(_, session, _)
        | Command::CancelTranscription(_, session)
        | Command::AudioFinished(
            _,
            session,
            audio_task::Reply::Draft(_) | audio_task::Reply::Transcription(_),
        )
        | Command::ImageAttachment(_, session, _, _)
        | Command::TaskAction { session, .. } => Some(*session),
        Command::ChangeSessionModel(change) => Some(change.session),
        Command::Session(command) => Some(command.session()),
        Command::Conversation { request, .. } => match request {
            ConversationRequest::Open { session }
            | ConversationRequest::Delete { session }
            | ConversationRequest::Reset { session }
            | ConversationRequest::SetWorkingDirectory { session, .. }
            | ConversationRequest::Send { session, .. }
            | ConversationRequest::Retry { session, .. }
            | ConversationRequest::ContinueWithoutCompaction { session, .. }
            | ConversationRequest::ProcessResults { session } => Some(*session),
            ConversationRequest::Create(_) | ConversationRequest::CreateAndSend { .. } => None,
        },
        _ => None,
    }
}
