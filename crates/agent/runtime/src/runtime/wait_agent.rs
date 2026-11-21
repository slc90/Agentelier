use super::mailbox;
use agentelier_conversation::ConversationFailure;
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MIN_TIMEOUT_MS: u64 = 50;
const MAX_TIMEOUT_MS: u64 = 60_000;

pub(super) struct Request {
    pub(super) origin: ProcessOrigin,
    pub(super) reply: oneshot::Sender<Result<Ticket, ConversationFailure>>,
}

pub(super) struct Ticket {
    pub(super) activity: watch::Receiver<u64>,
    pub(super) ready: bool,
    pub(super) steer: CancellationToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    timeout_ms: Option<u64>,
}

pub(super) fn registration(commands: mpsc::Sender<mailbox::Command>) -> ToolRegistration {
    const NAME: &str = "wait_agent";
    let definition = ToolDefinition {
        name: NAME.into(),
        description: "Wait for activity in your own collaboration mailbox. Returns as soon as activity is available, a steer arrives, or the timeout expires. Use null for 15000ms. The range is 50–60000ms; shorter durations are clamped. This tool leaves other agents running.".into(),
        parameters: json!({"type":"object","properties":{
            "timeout_ms":{"type":["integer","null"]}
        },"required":["timeout_ms"],"additionalProperties":false}),
        source: ToolSource::Extension { provider: "agentelier.collaboration".into(), tool: NAME.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::with_origin(definition, move |_, raw, cancel, origin| {
        let commands = commands.clone();
        Box::pin(wait(raw, cancel, origin, commands))
    })
    .text_result()
}

async fn wait(
    raw: &str,
    cancel: &CancellationToken,
    origin: ProcessOrigin,
    commands: mpsc::Sender<mailbox::Command>,
) -> ToolResult {
    let duration = match serde_json::from_str::<Arguments>(raw) {
        Ok(arguments) => arguments.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
        Err(error) => return ToolResult::not_executed("invalid_wait", &error.to_string()),
    };
    if duration > MAX_TIMEOUT_MS {
        return ToolResult::not_executed(
            "invalid_wait_timeout",
            &format!("等待上限为 {MAX_TIMEOUT_MS} 毫秒"),
        );
    }
    let timeout = duration.max(MIN_TIMEOUT_MS);
    let (reply, ticket) = oneshot::channel();
    let request = mailbox::Command::Wait(Request {
        origin: origin.clone(),
        reply,
    });
    tokio::select! { biased;
        _ = cancel.cancelled() => return cancelled(),
        result = commands.send(request) => if result.is_err() { return closed(); },
    }
    let mut ticket = tokio::select! { biased;
        _ = cancel.cancelled() => return cancelled(),
        result = ticket => match result {
            Ok(Ok(ticket)) => ticket,
            Ok(Err(error)) => return ToolResult::not_executed("wait_rejected", &error.to_string()),
            Err(_) => return closed(),
        },
    };
    let started = std::time::Instant::now();
    tracing::debug!(thread_id = %origin.session, turn_id = origin.turn, call_id = %origin.call_id, timeout, ready = ticket.ready, "已订阅 Agent 邮箱等待");
    let reason = if ticket.steer.is_cancelled() {
        "steered"
    } else if ticket.ready {
        "mailbox_activity"
    } else {
        tokio::select! { biased;
            _ = cancel.cancelled() => return cancelled(),
            _ = ticket.steer.cancelled() => "steered",
            changed = ticket.activity.changed() => if changed.is_ok() { "mailbox_activity" } else { return closed(); },
            _ = tokio::time::sleep(Duration::from_millis(timeout)) => "timeout",
        }
    };
    let elapsed_ms = started.elapsed().as_millis();
    let revision = *ticket.activity.borrow();
    tracing::debug!(thread_id = %origin.session, turn_id = origin.turn, call_id = %origin.call_id, reason, elapsed_ms, revision, "Agent 等待已结束");
    ToolResult {
        status: ToolStatus::Succeeded,
        cleanup_failed: false,
        output: json!({"reason":reason,"timeout_ms":timeout,"clamped":timeout!=duration,"elapsed_ms":elapsed_ms,"revision":revision}).to_string().into(),
    }
}

fn cancelled() -> ToolResult {
    ToolResult::error(ToolStatus::Cancelled, "wait_cancelled", "Agent 等待已取消")
}

fn closed() -> ToolResult {
    ToolResult::not_executed("wait_closed", "Agent 邮箱已结束等待接纳")
}
