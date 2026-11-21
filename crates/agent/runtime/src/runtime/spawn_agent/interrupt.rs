use super::{Children, ModelTask, apply_execution};
use crate::runtime::interrupt_agent::{Request, cancelled, unconfirmed};
use agentelier_conversation::{
    AgentRegistryError, AgentResidency, AgentState, AgentStop, AgentStopSource, Conversation,
    RecordEvent, SessionId, TurnId,
};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{ToolResult, ToolStatus};
use serde_json::json;

impl Children {
    pub(super) async fn interrupt(
        &mut self,
        request: &Request,
        conversation: &mut Conversation,
        root_task: Option<&ModelTask>,
        store: &SessionStoreClient,
    ) -> ToolResult {
        let origin = &request.origin;
        let turn = TurnId(origin.turn);
        let (session, caller) =
            match self.mailbox_caller(conversation, root_task, origin.session, turn) {
                Ok(caller) => caller,
                Err(error) => return rejected(error),
            };
        let registry = conversation.agent_registry();
        let target = match registry.resolve(session, caller, &request.target) {
            Ok(target) => &target.identity,
            Err(error) => {
                tracing::debug!(session_id = %session, agent_id = %caller, turn_id = turn.0, call_id = %origin.call_id, %error, "中断目标解析失败");
                return match error {
                    AgentRegistryError::ForeignId { .. } => rejected("Agent 目标不属于当前会话"),
                    error => rejected(error),
                };
            }
        };
        let caller_node = match registry.get(session, caller) {
            Ok(node) => node,
            Err(error) => return rejected(error),
        };
        if target.id == caller || !caller_node.identity.path.contains(&target.path) {
            return rejected("中断目标必须是调用者的后代");
        }
        let source = AgentStopSource {
            caller,
            turn,
            call_id: origin.call_id.clone(),
        };
        if let Some(recorded) = registry.stop_request(session, &source) {
            return if target.id == recorded.target {
                self.interrupt_receipt(conversation, session, recorded)
            } else {
                rejected("中断来源已用于不同目标")
            };
        }
        if request.cancel.is_cancelled() {
            return cancelled();
        }
        if conversation
            .current()
            .is_some_and(|view| view.execution_blocked)
        {
            return unconfirmed("当前会话的存档提交未确认，请重新打开核对");
        }
        let target = target.clone();
        let subtree = match registry.subtree(session, target.id) {
            Ok(subtree) => subtree,
            Err(error) => return rejected(error),
        };
        let mut affected = Vec::new();
        let mut existing = None;
        for node in subtree {
            let id = node.identity.id;
            match node.state {
                AgentState::Stopping => {
                    existing = existing.or_else(|| registry.latest_stop(id).cloned())
                }
                AgentState::Running if self.active.contains_key(&id) => affected.push(id),
                AgentState::Waiting => affected.push(id),
                _ => {}
            }
        }
        let operation = if affected.is_empty() {
            existing
        } else {
            Some(source.clone())
        };
        let stop = AgentStop {
            source,
            target: target.id,
            operation,
            affected,
        };
        let result = store
            .append(RecordEvent::AgentStopRequested {
                session,
                stop: stop.clone(),
            })
            .await;
        if !result
            .as_ref()
            .is_ok_and(|commit| commit.session == session)
        {
            let error = result.err().map_or_else(
                || "中断提交回执会话不匹配".into(),
                |error| error.to_string(),
            );
            tracing::error!(session_id = %session, agent_id = %target.id, agent_path = %target.path, operation = %stop.source, %error, "Agent 中断请求提交未确认");
            conversation.fail_storage(error);
            return unconfirmed("中断请求提交未确认，请重新打开核对");
        }
        if let Err(error) = conversation
            .agent_registry_mut()
            .apply_stop(session, stop.clone())
        {
            conversation.fail_storage(error.to_string());
            return unconfirmed("中断请求已提交，Registry 状态需核对");
        }
        // Traversal builds the order from ancestor to descendant. No new admissions can
        // interleave with this actor command. Finished parents commit after their children.
        for id in &stop.affected {
            if let Some(child) = self.active.get(id) {
                child.stop.cancel();
                continue;
            }
            let node = match conversation.agent_registry().get(session, *id) {
                Ok(node) => node,
                Err(error) => return unconfirmed(&error.to_string()),
            };
            let identity = node.identity.clone();
            if let Err(error) = apply_execution(
                store,
                conversation,
                &identity,
                AgentState::Interrupted,
                None,
            )
            .await
            {
                return unconfirmed(&error.to_string());
            }
        }
        tracing::info!(session_id = %session, agent_id = %target.id, agent_path = %target.path, operation = %stop.source, affected = stop.affected.len(), "Agent 中断请求已处理");
        self.interrupt_receipt(conversation, session, &stop)
    }

    fn interrupt_receipt(
        &self,
        conversation: &Conversation,
        session: SessionId,
        stop: &AgentStop,
    ) -> ToolResult {
        let registry = conversation.agent_registry();
        let node = match registry.get(session, stop.target) {
            Ok(node) => node,
            Err(error) => return rejected(error),
        };
        let recorded = stop
            .operation
            .as_ref()
            .and_then(|source| registry.stop_request(session, source));
        let status = if let Some(recorded) = recorded {
            let mut status = "settled";
            for &id in &recorded.affected {
                if registry.latest_stop(id) != Some(&recorded.source) {
                    continue;
                }
                if let Ok(member) = registry.get(session, id)
                    && member.state == AgentState::Stopping
                {
                    if member.residency == AgentResidency::Unloaded
                        || self
                            .active
                            .get(&id)
                            .is_none_or(|child| child.cleanup_unconfirmed)
                    {
                        status = "unknown";
                        break;
                    }
                    status = "stopping";
                }
            }
            status
        } else {
            "inactive"
        };
        ToolResult {
            status: ToolStatus::Succeeded,
            cleanup_failed: false,
            output: json!({
                "agent_id": stop.target, "path": node.identity.path, "state": node.state.name(),
                "status": status, "operation_id": stop.operation.as_ref().map(ToString::to_string)
            })
            .to_string()
            .into(),
        }
    }
}

fn rejected(error: impl std::fmt::Display) -> ToolResult {
    ToolResult::not_executed("interrupt_rejected", &error.to_string())
}
