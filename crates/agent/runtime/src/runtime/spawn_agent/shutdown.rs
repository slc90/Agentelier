use super::{Children, mailbox};
use crate::runtime::RuntimeFailure;
use agentelier_conversation::{Conversation, ConversationFailure};
use agentelier_session_store::SessionStoreClient;
use std::future::pending;
use tokio::sync::mpsc;

impl Children {
    pub(in crate::runtime) async fn settle_active(
        &mut self,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
        mut requests: Option<&mut mpsc::Receiver<mailbox::Command>>,
    ) -> Result<(), RuntimeFailure> {
        self.cancel_active();
        let mut first_failure = None;
        while !self.jobs.is_empty() {
            tokio::select! {
                Some(result) = self.jobs.join_next() => {
                    if let Err(failure) = self.finish(result, store, conversation).await {
                        first_failure.get_or_insert(failure);
                    }
                }
                command = async {
                    match requests.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        None => pending().await,
                    }
                } => match command {
                    Some(mailbox::Command::Inbox(request)) => self.settle_inbox(request, conversation, None, store).await,
                    Some(command) => command.close(),
                    None => requests = None,
                },
            }
        }
        if self.has_active() {
            first_failure.get_or_insert_with(|| {
                ConversationFailure::new("部分子 Agent 尚无可确认的收尾结果").into()
            });
        }
        first_failure.map_or(Ok(()), Err)
    }

    pub(in crate::runtime) fn release_idle(
        &mut self,
        conversation: &mut Conversation,
    ) -> Result<(), RuntimeFailure> {
        if let Some(view) = conversation.current() {
            for (&agent, child) in &self.parked {
                conversation
                    .agent_registry_mut()
                    .detach(view.id, agent, child.instance)
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
            }
        }
        self.parked.clear();
        self.waiters.clear();
        self.mailbox = mailbox::MailboxActivity::default();
        Ok(())
    }

    pub(in crate::runtime) async fn stop(
        &mut self,
        store: &SessionStoreClient,
        conversation: &mut Conversation,
        requests: &mut mpsc::Receiver<mailbox::Command>,
    ) -> Result<(), RuntimeFailure> {
        self.stop.cancel();
        let stopped = self
            .settle_active(store, conversation, Some(requests))
            .await;
        conversation.mailboxes_mut().close();
        let released = self.release_idle(conversation);
        stopped.and(released)
    }
}
