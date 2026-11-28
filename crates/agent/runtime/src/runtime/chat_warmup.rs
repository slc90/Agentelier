use super::tool_jobs::SessionTools;
use agentelier_account::CredentialClient;
use agentelier_conversation::SessionId;
use agentelier_llm_model::GenerationConnection;

#[derive(Clone, PartialEq, Eq)]
pub enum ChatWarmupTarget {
    Draft(String),
    Session(SessionId),
}

#[derive(Default)]
pub(super) struct ChatWarmup(Option<(ChatWarmupTarget, GenerationConnection)>);

impl ChatWarmup {
    pub(super) fn prepare(&mut self, target: ChatWarmupTarget, credentials: &CredentialClient) {
        if self
            .0
            .as_ref()
            .is_some_and(|(current, socket)| current == &target && socket.is_pending_or_open())
        {
            return;
        }
        self.0 = Some((
            target,
            GenerationConnection::preconnect(credentials.clone()),
        ));
    }

    pub(super) fn attach(&mut self, target: &ChatWarmupTarget, tools: &SessionTools) {
        if let Ok(mut connection) = tools.model_connection.try_lock()
            && !connection.is_pending_or_open()
            && self
                .0
                .as_ref()
                .is_some_and(|(current, _)| current == target)
            && let Some((_, warm)) = self.0.take()
        {
            *connection = warm;
        }
    }
}
