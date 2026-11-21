use super::mailbox;
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(super) struct Request {
    pub(super) origin: ProcessOrigin,
    pub(super) target: String,
    pub(super) cancel: CancellationToken,
    pub(super) reply: oneshot::Sender<ToolResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    target: String,
}

pub(super) fn registration(commands: mpsc::Sender<mailbox::Command>) -> ToolRegistration {
    const NAME: &str = "interrupt_agent";
    let definition = ToolDefinition {
        name: NAME.into(),
        description: "Request cancellation of a descendant by stable Agent ID or exact path. A stopping receipt means cleanup is pending. Active descendants of the target are cancelled with it. An inactive target with no active descendants requires no action.".into(),
        parameters: json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"],"additionalProperties":false}),
        source: ToolSource::Extension { provider: "agentelier.collaboration".into(), tool: NAME.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::with_origin(definition, move |_, raw, cancel, origin| {
        let commands = commands.clone();
        Box::pin(async move {
            let arguments: Arguments = match serde_json::from_str(raw) {
                Ok(arguments) => arguments,
                Err(error) => return ToolResult::not_executed("invalid_interrupt", &error.to_string()),
            };
            let (reply, receipt) = oneshot::channel();
            let request = mailbox::Command::Interrupt(Request {
                origin, target: arguments.target, cancel: cancel.clone(), reply,
            });
            tokio::select! { biased;
                _ = cancel.cancelled() => return cancelled(),
                result = commands.send(request) => if result.is_err() {
                    return ToolResult::not_executed("interrupt_closed", "Agent Control 已停止接纳");
                },
            }
            tokio::select! { biased;
                result = receipt => result.unwrap_or_else(|_| unconfirmed("Agent Control 未返回中断回执")),
                _ = cancel.cancelled() => unconfirmed("调用已取消，需核对中断请求回执"),
            }
        })
    }).text_result()
}

pub(super) fn cancelled() -> ToolResult {
    ToolResult::not_executed("interrupt_cancelled", "中断请求在接纳前取消")
}

pub(super) fn unconfirmed(message: &str) -> ToolResult {
    ToolResult::error(ToolStatus::Unconfirmed, "interrupt_unconfirmed", message)
}
