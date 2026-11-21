use super::{Children, limits::Capacity};
use agentelier_conversation::{
    AgentId, AgentIdentity, AgentNode, AgentRegistry, AgentResidency, AgentRuntimeId, AgentState,
    Conversation, ConversationFailure, RecordEvent, SessionId,
};
use agentelier_session_store::SessionStoreClient;

pub(super) struct ResidentChild {
    pub(super) instance: AgentRuntimeId,
    pub(super) history: Box<Conversation>,
}

impl Children {
    fn available_slot<'a>(
        &self,
        nodes: impl Iterator<Item = &'a AgentNode>,
    ) -> Result<Option<(AgentIdentity, AgentRuntimeId)>, Capacity> {
        let limit = self.limits.resident;
        if limit == 0 {
            return Err(Capacity::Resident(limit));
        }
        let nodes = nodes
            .skip(1)
            .filter(|node| node.residency != AgentResidency::Unloaded);
        let mut count = 0;
        let mut candidate = None;
        for node in nodes {
            count += 1;
            if candidate.is_none()
                && matches!(
                    node.state,
                    AgentState::Waiting
                        | AgentState::Completed
                        | AgentState::Interrupted
                        | AgentState::Failed(_)
                )
                && let Some(parked) = self.parked.get(&node.identity.id)
                && node.residency == AgentResidency::Resident(parked.instance)
            {
                candidate = Some((node.identity.clone(), parked.instance));
            }
        }
        if count < limit {
            return Ok(None);
        }
        candidate.map(Some).ok_or(Capacity::Resident(limit))
    }

    pub(super) fn check_residency_capacity(
        &self,
        registry: &AgentRegistry,
        session: SessionId,
    ) -> Result<(), agentelier_tools::ToolResult> {
        let nodes = registry
            .subtree(session, AgentId::for_root(session))
            .map_err(super::rejection)?;
        self.available_slot(nodes)
            .map(drop)
            .map_err(Capacity::rejection)
    }

    pub(super) async fn reserve_residency(
        &mut self,
        conversation: &mut Conversation,
        store: &SessionStoreClient,
        session: SessionId,
    ) -> Result<(), ConversationFailure> {
        if conversation
            .current()
            .is_some_and(|view| view.execution_blocked)
        {
            return Err(ConversationFailure::new(
                "会话有未确认的存档，不接纳驻留变化",
            ));
        }
        let nodes = conversation
            .agent_registry()
            .subtree(session, AgentId::for_root(session))
            .map_err(|error| ConversationFailure::new(error.to_string()))?;
        let Some((identity, instance)) = self
            .available_slot(nodes)
            .map_err(|error| ConversationFailure::new(error.to_string()))?
        else {
            return Ok(());
        };
        let agent = identity.id;
        let commit = store
            .append(RecordEvent::AgentEviction {
                session,
                agent,
                instance,
            })
            .await;
        let sequence = match commit {
            Ok(commit) if commit.session == session => commit.sequence,
            result => {
                let message = result.err().map_or_else(
                    || "子 Agent 卸载回执会话不匹配".into(),
                    |failure| failure.to_string(),
                );
                conversation.fail_storage(message.clone());
                return Err(ConversationFailure::new(message));
            }
        };
        conversation
            .agent_registry_mut()
            .detach(session, agent, instance)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.parked.remove(&agent);
        self.mailbox.forget(agent);
        tracing::info!(session_id = %session, agent_id = %agent, agent_path = %identity.path, runtime_instance = %instance, sequence, "空闲子 Agent 已卸载");
        Ok(())
    }
}
