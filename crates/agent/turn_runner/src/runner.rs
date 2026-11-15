use agentelier_account::CredentialClient;
use agentelier_conversation::{ActiveTurn, ConversationFailure, SessionView, TurnStatus};
use agentelier_llm_model::{GenerationResult, ModelFailure, ModelFailureKind};
use agentelier_tools::ToolResult;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL_PROGRESS_CAPACITY: usize = 32;
const MAX_MODEL_REQUESTS: usize = 2;
const MAX_TOOL_CALLS: usize = 1;

async fn publish(
    active: &ActiveTurn,
    progress: &mpsc::Sender<SessionView>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    tokio::select! { biased;
        _ = cancel.cancelled() => Ok(()),
        result = progress.send(active.view()) => result.map_err(|_| ConversationFailure::new("任务进度接收端已关闭")),
    }
}

async fn request_once(
    active: &mut ActiveTurn,
    credentials: CredentialClient,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<SessionView>,
) -> Result<Option<Result<GenerationResult, ModelFailure>>, ConversationFailure> {
    let (sender, mut events) = mpsc::channel(MODEL_PROGRESS_CAPACITY);
    let request = active.request()?;
    publish(active, progress, cancel).await?;
    let outcome = {
        let generation = agentelier_llm_model::generate(credentials, request, sender);
        tokio::pin!(generation);
        loop {
            tokio::select! { biased;
                result = &mut generation => break Some(result),
                _ = cancel.cancelled() => break None,
                event = events.recv() => {
                    if let Some(event) = event {
                        if let Err(failure) = active.update(event) {
                            break Some(Err(protocol_failure(failure)));
                        }
                        publish(active, progress, cancel).await?;
                    }
                }
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        if let Err(failure) = active.update(event) {
            return Ok(Some(Err(protocol_failure(failure))));
        }
    }
    Ok(outcome)
}
fn protocol_failure(failure: ConversationFailure) -> ModelFailure {
    ModelFailure {
        kind: ModelFailureKind::Protocol,
        message: failure.to_string(),
        status: None,
        code: None,
        request_id: None,
    }
}

pub async fn run(
    mut active: ActiveTurn,
    credentials: CredentialClient,
    cancel: CancellationToken,
    progress: mpsc::Sender<SessionView>,
) -> Result<ActiveTurn, ConversationFailure> {
    let catalog = active.catalog();
    let mut tools_used = 0;
    for request_index in 0..MAX_MODEL_REQUESTS {
        if cancel.is_cancelled() {
            active.finish(TurnStatus::Cancelled)?;
            return Ok(active);
        }
        let result =
            match request_once(&mut active, credentials.clone(), &cancel, &progress).await? {
                Some(Ok(result)) => result,
                Some(Err(failure)) => {
                    active.finish(TurnStatus::Failed(failure.to_string()))?;
                    return Ok(active);
                }
                None => {
                    active.finish(TurnStatus::Cancelled)?;
                    return Ok(active);
                }
            };
        if let Err(failure) = active.accept_response(&result) {
            active.finish(TurnStatus::Failed(failure.to_string()))?;
            return Ok(active);
        }
        publish(&active, &progress, &cancel).await?;
        if result.calls.is_empty() {
            active.seal(result)?;
            active.finish(TurnStatus::Completed)?;
            return Ok(active);
        }
        let limited = result.calls.len() > MAX_TOOL_CALLS.saturating_sub(tools_used);
        tools_used += result.calls.len();
        for call in &result.calls {
            let outcome = if limited {
                ToolResult::not_executed("tool_budget", "本批调用超过任务限额，整批未执行")
            } else if cancel.is_cancelled() {
                ToolResult::not_executed("stopped", "用户已停止，工具未执行")
            } else {
                active.tool_started(&call.call_id)?;
                publish(&active, &progress, &cancel).await?;
                let started = std::time::Instant::now();
                tracing::debug!(
                    request_index = request_index + 1,
                    call_id = call.call_id,
                    tool = call.name,
                    "工具执行开始"
                );
                let outcome =
                    agentelier_tools::execute(&catalog, &call.name, &call.arguments, &cancel).await;
                tracing::debug!(
                    request_index = request_index + 1,
                    call_id = call.call_id,
                    tool = call.name,
                    elapsed_ms = started.elapsed().as_millis(),
                    outcome = match outcome.status {
                        agentelier_tools::ToolStatus::Succeeded => "succeeded",
                        agentelier_tools::ToolStatus::TimedOut => "timed_out",
                        agentelier_tools::ToolStatus::Cancelled => "cancelled",
                        agentelier_tools::ToolStatus::NotExecuted => "not_executed",
                        _ => "failed",
                    },
                    "工具执行已结束"
                );
                outcome
            };
            active.tool_result(&call.call_id, outcome)?;
            publish(&active, &progress, &cancel).await?;
        }
        active.seal(result)?;
        if cancel.is_cancelled() {
            active.finish(TurnStatus::Cancelled)?;
            return Ok(active);
        }
        if limited {
            active.finish(TurnStatus::Limited)?;
            return Ok(active);
        }
    }
    active.finish(TurnStatus::Limited)?;
    Ok(active)
}
