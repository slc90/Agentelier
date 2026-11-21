use super::super::child_runtime::Execution;
use super::{ChildHandle, ChildTurn, Children, SpawnEnvironment, apply_execution};
use agentelier_conversation::{
    AgentId, AgentIdentity, AgentMessage, AgentResidency, AgentState, Conversation,
    ConversationFailure, MailboxEvent, MessageClaim, SessionId,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::runtime) enum WakeOutcome {
    Running,
    Queued,
    Idle,
}

impl Children {
    pub(super) async fn launch_turn(
        &mut self,
        conversation: &mut Conversation,
        identity: AgentIdentity,
        turn: ChildTurn,
        messages: &[AgentMessage],
        parent_stop: CancellationToken,
        environment: SpawnEnvironment<'_>,
    ) -> Result<(), ConversationFailure> {
        let session = identity.session;
        let id = identity.id;
        let turn_id = turn.turn_id()?;
        let assignment = messages
            .iter()
            .find(|message| message.mode == agentelier_conversation::DeliveryMode::TriggerTurn)
            .cloned()
            .ok_or_else(|| ConversationFailure::new("子 Agent 启动缺少任务来源"))?;
        let resident = match conversation
            .agent_registry()
            .get(session, id)
            .map_err(failure)?
            .residency
        {
            AgentResidency::Resident(instance) => Some(instance),
            AgentResidency::Unloaded => {
                self.reserve_residency(conversation, environment.store, session)
                    .await?;
                None
            }
        };
        let mailbox = self
            .mailbox
            .subscribe(id, conversation.mailboxes().revision());
        let mut claims = Vec::with_capacity(messages.len());
        for message in messages {
            let claim = MessageClaim {
                receiver: id,
                message: message.id,
                turn: turn_id,
                wake: true,
            };
            self.mailbox
                .commit(
                    conversation,
                    environment.store,
                    session,
                    MailboxEvent::Claimed { claim },
                )
                .await?;
            claims.push(claim);
        }
        let instance = if let Some(instance) = resident {
            instance
        } else {
            conversation
                .agent_registry_mut()
                .attach(session, id)
                .map_err(failure)?
        };
        if let Err(error) = apply_execution(
            environment.store,
            conversation,
            &identity,
            AgentState::Running,
            None,
        )
        .await
        {
            if resident.is_none() {
                conversation
                    .agent_registry_mut()
                    .detach(session, id, instance)
                    .map_err(failure)?;
            }
            return Err(error);
        }
        let stop = self.stop.child_token();
        let (commands, incoming) = mpsc::channel(super::super::COMMAND_CAPACITY);
        let thread = identity.thread.session_id();
        self.active.insert(
            id,
            ChildHandle {
                thread,
                turn: turn_id,
                commands,
                stop: stop.clone(),
                cleanup_unconfirmed: false,
            },
        );
        tracing::info!(session_id = %session, agent_id = %id, agent_path = %identity.path, runtime_instance = %instance, turn_id = turn_id.0, model = %turn.view().settings.slug, effort = ?turn.view().settings.effort, role = %identity.role, "子 Agent 已启动独立执行轮次");
        self.jobs.spawn(
            Execution {
                identity,
                instance,
                turn,
                store: environment.store.clone(),
                credentials: environment.credentials.clone(),
                skills_root: environment.skills_root.into(),
                stop,
                parent_stop,
                commands: incoming,
                mailbox,
                claims,
                assignment,
                mailbox_commands: environment.mailbox_commands.clone(),
            }
            .run(),
        );
        Ok(())
    }

    pub(in crate::runtime) async fn wake_pending(
        &mut self,
        conversation: &mut Conversation,
        session: SessionId,
        id: AgentId,
        environment: SpawnEnvironment<'_>,
    ) -> Result<WakeOutcome, ConversationFailure> {
        if self.stop.is_cancelled() || !conversation.is_current(session) {
            return Err(ConversationFailure::new("所属会话正在关闭或已切换"));
        }
        if conversation
            .current()
            .is_some_and(|view| view.execution_blocked)
        {
            return Err(ConversationFailure::new("所属会话存在未确认的存档"));
        }
        if self.active.contains_key(&id) {
            return Ok(WakeOutcome::Queued);
        }
        let messages = conversation.mailboxes().wake_inputs(id)?;
        if messages.is_empty() {
            return Ok(WakeOutcome::Idle);
        }
        self.check_execution_capacity().map_err(failure)?;
        let node = conversation
            .agent_registry()
            .get(session, id)
            .map_err(failure)?;
        let identity = node.identity.clone();
        let residency = node.residency;
        if identity.parent.is_none() {
            return Err(ConversationFailure::new("Follow-up 只唤醒子 Agent"));
        }
        let mut ancestor = Some(id);
        while let Some(member) = ancestor {
            let node = conversation
                .agent_registry()
                .get(session, member)
                .map_err(failure)?;
            if matches!(node.state, AgentState::Closed | AgentState::Stopping)
                || conversation
                    .agent_registry()
                    .subtree_stopping(session, member)
            {
                return Err(ConversationFailure::new(
                    "目标 Agent 或其祖先正在停止或已经关闭",
                ));
            }
            ancestor = node.identity.parent;
        }
        let operation = environment.operations.allocate().map_err(failure)?;
        let task = messages
            .iter()
            .map(AgentMessage::render)
            .collect::<Vec<_>>()
            .join("\n\n");
        let thread = identity.thread.session_id();
        let turn = if let Some(parked) = self.parked.get_mut(&id) {
            if residency != AgentResidency::Resident(parked.instance) {
                return Err(ConversationFailure::new("子 Agent 驻留实例已变化"));
            }
            let view = ChildTurn::start(&mut parked.history, thread, task, operation)?;
            let parked = self
                .parked
                .remove(&id)
                .ok_or_else(|| ConversationFailure::new("唤醒时缺少已准备的 Agent 历史"))?;
            ChildTurn::from_started(*parked.history, view, operation)
        } else {
            if residency != AgentResidency::Unloaded {
                return Err(ConversationFailure::new(
                    "驻留实例未提供可确认的子 Agent 历史",
                ));
            }
            let mut history = restore_child(conversation, &identity, environment.store).await?;
            let view = ChildTurn::start(&mut history, thread, task, operation)?;
            ChildTurn::from_started(history, view, operation)
        };
        self.launch_turn(
            conversation,
            identity,
            turn,
            &messages,
            self.stop.child_token(),
            environment,
        )
        .await?;
        Ok(WakeOutcome::Running)
    }
}

async fn restore_child(
    conversation: &mut Conversation,
    identity: &AgentIdentity,
    store: &agentelier_session_store::SessionStoreClient,
) -> Result<Conversation, ConversationFailure> {
    let session = identity.session;
    let id = identity.id;
    let mut restored = store.load_child(session, id).await.map_err(failure)?;
    if restored.identity != *identity {
        return Err(ConversationFailure::new(
            "冷恢复身份与当前 Agent Tree 不一致",
        ));
    }
    if restored.settle_unknown {
        apply_execution(store, conversation, identity, AgentState::Unknown, None).await?;
    }
    if let Some(event) = restored.recovery_event() {
        let thread = identity.thread.session_id();
        let result = store.for_agent(identity).append(event).await;
        let sequence = match result {
            Ok(commit) if commit.session == thread => commit.sequence,
            result => {
                let message = result.err().map_or_else(
                    || "子 Agent 恢复回执身份不符".into(),
                    |error| error.to_string(),
                );
                conversation.fail_storage(message.clone());
                return Err(ConversationFailure::new(message));
            }
        };
        restored.confirm_recovery(sequence);
    }
    conversation.install_child(restored).inspect_err(|error| {
        tracing::error!(session_id = %session, agent_id = %id, agent_path = %identity.path, %error, "子 Agent 冷恢复绑定失败");
    })
}

fn failure(error: impl std::fmt::Display) -> ConversationFailure {
    ConversationFailure::new(error.to_string())
}
