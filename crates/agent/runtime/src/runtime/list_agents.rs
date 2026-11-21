use super::mailbox;
use agentelier_conversation::{
    AgentId, AgentPath, AgentRegistry, AgentResidency, ConversationFailure, SessionId,
};
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

const DEFAULT_LIMIT: usize = 16;
const MAX_LIMIT: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path_prefix: Option<AgentPath>,
    after: Option<AgentPath>,
    limit: Option<usize>,
}

pub(super) struct Request {
    pub(super) origin: ProcessOrigin,
    arguments: Arguments,
    reply: oneshot::Sender<ToolResult>,
}

#[derive(Serialize)]
pub(super) struct Page<'a> {
    agents: Vec<Entry<'a>>,
    next_after: Option<&'a AgentPath>,
}

#[derive(Serialize)]
struct Entry<'a> {
    agent_id: AgentId,
    path: &'a AgentPath,
    parent_id: Option<AgentId>,
    role: &'a str,
    state: &'static str,
    residency: &'static str,
    task_name: Option<&'a str>,
}

impl Request {
    pub(super) fn snapshot<'a>(
        &self,
        registry: &'a AgentRegistry,
        session: SessionId,
        caller: AgentId,
    ) -> Result<Page<'a>, ConversationFailure> {
        let failure = |error: agentelier_conversation::AgentRegistryError| {
            ConversationFailure::new(error.to_string())
        };
        let caller_path = &registry
            .get(session, caller)
            .map_err(failure)?
            .identity
            .path;
        let prefix = self.arguments.path_prefix.as_ref().unwrap_or(caller_path);
        if !caller_path.contains(prefix)
            || self
                .arguments
                .after
                .as_ref()
                .is_some_and(|after| !prefix.contains(after))
        {
            return Err(ConversationFailure::new("列表路径或游标超出调用者可见范围"));
        }
        let limit = self.arguments.limit.unwrap_or(DEFAULT_LIMIT);
        let mut nodes = registry
            .subtree(session, caller)
            .map_err(failure)?
            .filter(|node| prefix.contains(&node.identity.path))
            .filter(|node| {
                self.arguments
                    .after
                    .as_ref()
                    .is_none_or(|after| &node.identity.path > after)
            });
        let mut agents = Vec::with_capacity(limit);
        for node in nodes.by_ref().take(limit) {
            let identity = &node.identity;
            let task_name = (identity.parent.is_some()
                && identity
                    .nickname
                    .chars()
                    .all(|ch| !ch.is_control() && !matches!(ch, '/' | '\\' | ':')))
            .then_some(identity.nickname.as_str());
            agents.push(Entry {
                agent_id: identity.id,
                path: &identity.path,
                parent_id: (identity.id != caller).then_some(identity.parent).flatten(),
                role: &identity.role,
                state: node.state.name(),
                residency: match node.residency {
                    AgentResidency::Unloaded => "unloaded",
                    AgentResidency::Resident(_) => "resident",
                },
                task_name,
            });
        }
        let next_after = nodes
            .next()
            .and_then(|_| agents.last().map(|agent| agent.path));
        Ok(Page { agents, next_after })
    }

    pub(super) fn respond(self, result: Result<Page<'_>, ConversationFailure>) {
        let result = match result {
            Ok(page) => ToolResult {
                status: ToolStatus::Succeeded,
                cleanup_failed: false,
                output: json!(page).to_string().into(),
            },
            Err(error) => {
                tracing::debug!(thread_id = %self.origin.session, turn_id = self.origin.turn, call_id = %self.origin.call_id, %error, "读取 Agent 树失败");
                ToolResult::not_executed("list_rejected", &error.to_string())
            }
        };
        drop(self.reply.send(result));
    }
}

pub(super) fn registration(commands: mpsc::Sender<mailbox::Command>) -> ToolRegistration {
    const NAME: &str = "list_agents";
    let definition = ToolDefinition {
        name: NAME.into(),
        description: "Read your Agent and its descendants in the current session. Optionally filter by a full path prefix and continue with after set to the returned next_after path. Null uses the default for each parameter.".into(),
        parameters: json!({"type":"object","properties":{
            "path_prefix":{"type":["string","null"]},
            "after":{"type":["string","null"]},
            "limit":{"type":["integer","null"],"minimum":1,"maximum":MAX_LIMIT}
        },"required":["path_prefix","after","limit"],"additionalProperties":false}),
        source: ToolSource::Extension { provider: "agentelier.collaboration".into(), tool: NAME.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::with_origin(definition, move |_, raw, cancel, origin| {
        let commands = commands.clone();
        Box::pin(async move {
            let arguments = match serde_json::from_str::<Arguments>(raw) {
                Ok(args) if (1..=MAX_LIMIT).contains(&args.limit.unwrap_or(DEFAULT_LIMIT)) => args,
                Ok(_) => return ToolResult::not_executed("invalid_list", "每页数量超出允许范围"),
                Err(error) => return ToolResult::not_executed("invalid_list", &error.to_string()),
            };
            let (reply, receipt) = oneshot::channel();
            let command = mailbox::Command::List(Request { origin, arguments, reply });
            let cancelled = || ToolResult::not_executed("list_cancelled", "列表读取已取消");
            tokio::select! { biased;
                _ = cancel.cancelled() => return cancelled(),
                result = commands.send(command) => if result.is_err() {
                    return ToolResult::not_executed("list_closed", "Agent Control 已关闭");
                },
            }
            tokio::select! { biased;
                result = receipt => result.unwrap_or_else(|_| ToolResult::not_executed("list_closed", "Agent Control 已关闭")),
                _ = cancel.cancelled() => cancelled(),
            }
        })
    }).text_result()
}
