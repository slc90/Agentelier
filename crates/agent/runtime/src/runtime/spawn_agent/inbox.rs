use super::wake::WakeOutcome;
use super::{Children, ModelTask, SpawnEnvironment, mailbox};
use agentelier_conversation::{
    AgentId, AgentMessage, AgentMessageId, AgentRegistryError, ClaimedMessage, Conversation,
    ConversationFailure, DeliveryMode, MailboxEvent, MessageClaim, MessageStatus, SessionId,
    TurnId,
};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{ToolResult, ToolStatus};
use agentelier_turn_runner::{InboxAction, InboxRequest};
use serde_json::json;

impl Children {
    fn mailbox_caller(
        &self,
        conversation: &Conversation,
        root_task: Option<&ModelTask>,
        thread: SessionId,
        turn: TurnId,
    ) -> Result<(SessionId, AgentId), ConversationFailure> {
        let view = conversation
            .current()
            .ok_or_else(|| ConversationFailure::new("协作请求缺少当前会话"))?;
        let session = view.id;
        let id = if session == thread {
            let task = root_task
                .filter(|task| task.control.matches_turn(session, turn.0))
                .ok_or_else(|| ConversationFailure::new("Root 协作请求不属于当前轮次"))?;
            task.check_instance(conversation).map_err(failure)?;
            AgentId::for_root(session)
        } else {
            self.active
                .iter()
                .find_map(|(id, child)| {
                    (child.thread == thread && child.turn == turn).then_some(*id)
                })
                .ok_or_else(|| ConversationFailure::new("子 Agent 协作请求不属于当前轮次"))?
        };
        Ok((session, id))
    }

    pub(in crate::runtime) async fn mailbox_command(
        &mut self,
        command: mailbox::Command,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        environment: SpawnEnvironment<'_>,
    ) {
        match command {
            mailbox::Command::Send(outgoing) => {
                let result = self
                    .send_message(&outgoing, conversation, root_task, environment)
                    .await;
                let result = result.unwrap_or_else(|failure| {
                    if conversation
                        .current()
                        .is_some_and(|view| view.execution_blocked)
                    {
                        ToolResult::error(
                            ToolStatus::Unconfirmed,
                            "message_unconfirmed",
                            &failure.to_string(),
                        )
                    } else {
                        ToolResult::not_executed("message_rejected", &failure.to_string())
                    }
                });
                drop(outgoing.reply.send(result));
            }
            mailbox::Command::Inbox(inbox) => {
                self.inbox(inbox, conversation, root_task, environment.store)
                    .await
            }
        }
    }

    async fn send_message(
        &mut self,
        outgoing: &mailbox::Outgoing,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        environment: SpawnEnvironment<'_>,
    ) -> Result<ToolResult, ConversationFailure> {
        let origin = &outgoing.origin;
        let turn = TurnId(origin.turn);
        let (session, sender) =
            self.mailbox_caller(conversation, root_task, origin.session, turn)?;
        if outgoing.cancel.is_cancelled() {
            return Err(ConversationFailure::new("消息在 Mailbox 接纳前取消"));
        }
        let target = conversation
            .agent_registry()
            .resolve(session, sender, &outgoing.target)
            .map_err(|error| match error {
                AgentRegistryError::ForeignId { .. } => {
                    tracing::debug!(session_id = %session, agent_id = %sender, turn_id = origin.turn, call_id = %origin.call_id, %error, "拒绝跨会话投递 Agent 消息");
                    ConversationFailure::new("Agent 目标不属于当前会话")
                }
                error => failure(error),
            })?;
        let receiver = target.identity.id;
        if receiver == sender {
            return Err(ConversationFailure::new("协作消息不能发送给调用者自身"));
        }
        if outgoing.mode == DeliveryMode::TriggerTurn && target.identity.parent.is_none() {
            return Err(ConversationFailure::new("Follow-up 只唤醒子 Agent"));
        }
        let path = target.identity.path.clone();
        let id = conversation
            .mailboxes()
            .find_source(sender, turn, &origin.call_id)
            .map_or_else(AgentMessageId::allocate, |entry| entry.message.id);
        let message = AgentMessage {
            id,
            sender,
            receiver,
            turn,
            call_id: origin.call_id.clone(),
            mode: outgoing.mode,
            text: outgoing.message.clone(),
        };
        let sequence = self
            .mailbox
            .commit(
                conversation,
                environment.store,
                session,
                MailboxEvent::Queued { message },
            )
            .await?;
        let mut output = json!({"accepted":true,"message_id":id,"target":receiver,"path":path,"sequence":sequence});
        let mut status = ToolStatus::Succeeded;
        if outgoing.mode == DeliveryMode::TriggerTurn {
            let saved = conversation.mailboxes().find(id).map(|entry| entry.status);
            let wake = match saved {
                Some(MessageStatus::Consumed(_)) => Ok(WakeOutcome::Idle),
                Some(MessageStatus::Claimed(_)) if self.active.contains_key(&receiver) => {
                    Ok(WakeOutcome::Running)
                }
                Some(MessageStatus::Claimed(_)) => Err(ConversationFailure::new(
                    "此前领用的 Follow-up 执行尚未确认",
                )),
                Some(MessageStatus::Pending) => {
                    self.wake_pending(conversation, session, receiver, environment)
                        .await
                }
                None => Err(ConversationFailure::new("缺少已接纳的 Follow-up 记录")),
            };
            match wake {
                Ok(WakeOutcome::Running) => output["wake"] = json!("running"),
                Ok(WakeOutcome::Queued) => output["wake"] = json!("queued"),
                Ok(WakeOutcome::Idle) => output["wake"] = json!("already_processed"),
                Err(error) => {
                    status = if matches!(saved, Some(MessageStatus::Claimed(_)))
                        || conversation
                            .current()
                            .is_some_and(|view| view.execution_blocked)
                    {
                        ToolStatus::Unconfirmed
                    } else {
                        ToolStatus::Failed
                    };
                    output["wake"] = json!("pending");
                    output["error"] = json!(error.to_string());
                }
            }
        }
        Ok(ToolResult {
            status,
            cleanup_failed: false,
            output: output.to_string().into(),
        })
    }

    pub(in crate::runtime) async fn inbox(
        &mut self,
        request: InboxRequest,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        store: &SessionStoreClient,
    ) {
        let context = self.mailbox_caller(conversation, root_task, request.thread, request.turn);
        match request.action {
            InboxAction::Take(reply) => {
                let result = match context {
                    Ok((session, receiver)) => {
                        self.take_note(conversation, store, session, receiver, request.turn)
                            .await
                    }
                    Err(failure) => Err(failure),
                };
                drop(reply.send(result));
            }
            InboxAction::Consumed { claim, reply } => {
                let result = match context {
                    Ok((session, receiver))
                        if receiver == claim.receiver && request.turn == claim.turn =>
                    {
                        self.mailbox
                            .commit(
                                conversation,
                                store,
                                session,
                                MailboxEvent::Consumed { claim },
                            )
                            .await
                            .map(drop)
                    }
                    Ok(_) => Err(ConversationFailure::new(
                        "邮箱消费回执不属于当前 Agent 轮次",
                    )),
                    Err(failure) => Err(failure),
                };
                drop(reply.send(result));
            }
        }
    }

    async fn take_note(
        &mut self,
        conversation: &mut Conversation,
        store: &SessionStoreClient,
        session: SessionId,
        receiver: AgentId,
        turn: TurnId,
    ) -> Result<Option<ClaimedMessage>, ConversationFailure> {
        let Some(entry) = conversation
            .mailboxes()
            .next_for_turn(receiver, turn)
            .filter(|entry| {
                entry.status == MessageStatus::Pending
                    && entry.message.mode == DeliveryMode::QueueOnly
            })
        else {
            return Ok(None);
        };
        let message = entry.message.clone();
        let claim = MessageClaim {
            message: message.id,
            receiver,
            turn,
        };
        self.mailbox
            .commit(
                conversation,
                store,
                session,
                MailboxEvent::Claimed { claim },
            )
            .await?;
        Ok(Some(ClaimedMessage { claim, message }))
    }
}

fn failure(error: impl std::fmt::Display) -> ConversationFailure {
    ConversationFailure::new(error.to_string())
}
