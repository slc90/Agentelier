use crate::control;
use agentelier_account::CredentialClient;
use agentelier_conversation::RecordEvent;
use agentelier_conversation::{
    ActiveTurn, ConversationFailure, InputDecision, RunnerEvent, SteerInput, TurnStatus,
};
use agentelier_llm_model::{GenerationResult, ModelFailure, ModelFailureKind};
use agentelier_tools::ToolResult;

async fn record(
    store: &agentelier_session_store::SessionStoreClient,
    event: RecordEvent,
) -> Result<(), ConversationFailure> {
    store
        .append(event)
        .await
        .map(|_| ())
        .map_err(|failure| ConversationFailure::new(format!("会话记录保存失败：{failure}")))
}
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL_PROGRESS_CAPACITY: usize = 32;
const MAX_MODEL_REQUESTS: usize = 32;
const MAX_TOOL_CALLS: usize = 64;

async fn publish(
    active: &ActiveTurn,
    progress: &mpsc::Sender<RunnerEvent>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    tokio::select! { biased;
        _ = cancel.cancelled() => Ok(()),
        result = progress.send(RunnerEvent::Progress(Box::new(active.view()))) => result.map_err(|_| ConversationFailure::new("任务进度接收端已关闭")),
    }
}

async fn request_once(
    active: &mut ActiveTurn,
    credentials: CredentialClient,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
    steers: &mut mpsc::Receiver<SteerInput>,
    store: &agentelier_session_store::SessionStoreClient,
) -> Result<Option<Result<GenerationResult, ModelFailure>>, ConversationFailure> {
    let (sender, mut events) = mpsc::channel(MODEL_PROGRESS_CAPACITY);
    let applied = active.apply_steers()?;
    let request = active.request()?;
    if !applied.is_empty() {
        record(
            store,
            RecordEvent::SteerApplied {
                state: active.stored_state(),
                history: active.archive(),
            },
        )
        .await?;
    }
    for id in applied {
        control::receipt(progress, id, InputDecision::Applied).await?;
    }
    publish(active, progress, cancel).await?;
    let outcome = {
        let generation = agentelier_llm_model::generate(credentials, request, sender);
        tokio::pin!(generation);
        loop {
            tokio::select! { biased;
                result = &mut generation => break Some(result),
                _ = cancel.cancelled() => break None,
                input = steers.recv(), if !steers.is_closed() => {
                    if let Some(input) = input {control::accept(active,input,progress,store).await?;publish(active,progress,cancel).await?;}
                }
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
    progress: mpsc::Sender<RunnerEvent>,
    mut steers: mpsc::Receiver<SteerInput>,
    store: agentelier_session_store::SessionStoreClient,
) -> Result<ActiveTurn, ConversationFailure> {
    let mut context =
        agentelier_tools::ExecutionContext::new(active.view().settings.working_directory);
    let admitted = record(
        &store,
        RecordEvent::Accepted {
            state: active.stored_state(),
        },
    )
    .await;
    let outcome = if admitted.is_ok() {
        run_task(
            &mut active,
            &mut context,
            credentials,
            &cancel,
            &progress,
            &mut steers,
            &store,
        )
        .await
    } else {
        admitted.map(|()| TurnStatus::Cancelled)
    };
    let phase = active.reaping();
    let projection = if phase.is_ok() {
        publish(&active, &progress, &cancel).await
    } else {
        phase
    };
    let reason = match &outcome {
        Ok(TurnStatus::Completed) => "task_completed",
        Ok(TurnStatus::Cancelled) => "task_stopped",
        Ok(TurnStatus::Limited) => "task_budget_exhausted",
        _ => "task_failed",
    };
    let report = context.cleanup(reason).await;
    let cleanup_failed = report.failed;
    active.process_cleanup(report)?;
    control::close(&mut steers, &progress).await?;
    let status = match outcome.and_then(|status| projection.map(|()| status)) {
        Ok(_) if cleanup_failed => TurnStatus::Failed("任务进程清理失败，执行已暂停".into()),
        Ok(status) => status,
        Err(failure) => {
            active.block_execution();
            TurnStatus::Failed(failure.to_string())
        }
    };
    active.finish(status)?;
    if let Err(failure) = record(
        &store,
        RecordEvent::Finalized {
            state: active.stored_state(),
            history: active.archive(),
        },
    )
    .await
    {
        active.block_execution();
        active.finish(TurnStatus::Failed(failure.to_string()))?;
    }
    Ok(active)
}

async fn run_task(
    active: &mut ActiveTurn,
    context: &mut agentelier_tools::ExecutionContext,
    credentials: CredentialClient,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
    steers: &mut mpsc::Receiver<SteerInput>,
    store: &agentelier_session_store::SessionStoreClient,
) -> Result<TurnStatus, ConversationFailure> {
    let catalog = active.catalog();
    let mut cleanup_failed = false;
    let mut tools_used = 0;
    for request_index in 0..MAX_MODEL_REQUESTS {
        control::drain(active, steers, progress, store).await?;
        if cancel.is_cancelled() {
            return Ok(TurnStatus::Cancelled);
        }
        let result =
            match request_once(active, credentials.clone(), cancel, progress, steers, store).await?
            {
                Some(Ok(result)) => result,
                Some(Err(failure)) => return Ok(TurnStatus::Failed(failure.to_string())),
                None => return Ok(TurnStatus::Cancelled),
            };
        if let Err(failure) = active.accept_response(&result) {
            return Ok(TurnStatus::Failed(failure.to_string()));
        }
        record(
            store,
            RecordEvent::Response {
                state: active.stored_state(),
                output: result.output.archive(),
            },
        )
        .await?;
        publish(active, progress, cancel).await?;
        control::drain(active, steers, progress, store).await?;
        if result.calls.is_empty() {
            active.seal(result)?;
            record(
                store,
                RecordEvent::Sealed {
                    state: active.stored_state(),
                    history: active.archive(),
                },
            )
            .await?;
            if active.has_pending_steer() {
                continue;
            }
            return Ok(TurnStatus::Completed);
        }
        let limited = result.calls.len() > MAX_TOOL_CALLS.saturating_sub(tools_used);
        tools_used += result.calls.len();
        for call in &result.calls {
            control::drain(active, steers, progress, store).await?;
            let outcome = if cleanup_failed {
                ToolResult::not_executed("cleanup_failed", "工具资源清理未确认，后续调用未执行")
            } else if limited {
                ToolResult::not_executed("tool_budget", "本批调用超过任务限额，整批未执行")
            } else if cancel.is_cancelled() {
                ToolResult::not_executed("stopped", "用户已停止，工具未执行")
            } else if active.has_pending_steer() {
                ToolResult::not_executed("steered", "补充输入已接纳，旧调用未执行")
            } else {
                active.tool_started(&call.call_id)?;
                record(
                    store,
                    RecordEvent::ToolStarted {
                        state: active.stored_state(),
                        call_id: call.call_id.clone(),
                    },
                )
                .await?;
                publish(active, progress, cancel).await?;
                let started = std::time::Instant::now();
                tracing::debug!(
                    request_index = request_index + 1,
                    call_id = call.call_id,
                    tool = call.name,
                    "工具执行开始"
                );
                let tool_cancel = cancel.child_token();
                let mut delivery_failure = None;
                let outcome = {
                    let execution = agentelier_tools::execute(
                        &catalog,
                        context,
                        &call.name,
                        &call.arguments,
                        &tool_cancel,
                    );
                    tokio::pin!(execution);
                    loop {
                        tokio::select! {biased;
                            result = &mut execution => break result,
                            input = steers.recv(), if !steers.is_closed() && delivery_failure.is_none() => {
                                if let Some(input) = input {
                                    let delivered = match control::accept(active,input,progress,store).await {
                                        Ok(()) => publish(active,progress,&tool_cancel).await,
                                        Err(failure) => Err(failure),
                                    };
                                    if let Err(failure) = delivered { delivery_failure = Some(failure); tool_cancel.cancel(); }
                                }
                            }
                        }
                    }
                };
                if let Some(failure) = delivery_failure {
                    return Err(failure);
                }
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
            let (outcome, storage_failure) = persist_image(active.view().id, outcome, store).await;
            cleanup_failed |= outcome.cleanup_failed;
            let (status, output) = active.tool_result(&call.call_id, outcome)?;
            if let Some(failure) = storage_failure {
                return Err(ConversationFailure::new(format!(
                    "图片已读取，附件保存失败，停止后续执行：{failure}"
                )));
            }
            record(
                store,
                RecordEvent::ToolReturned {
                    state: active.stored_state(),
                    call_id: call.call_id.clone(),
                    status,
                    output: agentelier_llm_model::ArchivedPayload::capture(&output),
                },
            )
            .await?;
            publish(active, progress, cancel).await?;
        }
        active.seal(result)?;
        record(
            store,
            RecordEvent::Sealed {
                state: active.stored_state(),
                history: active.archive(),
            },
        )
        .await?;
        if cleanup_failed {
            active.block_execution();
            return Ok(TurnStatus::Failed(
                "工具资源清理未确认，请检查运行环境后新建会话".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Ok(TurnStatus::Cancelled);
        }
        if limited {
            return Ok(TurnStatus::Limited);
        }
    }
    Ok(TurnStatus::Limited)
}

async fn persist_image(
    session: agentelier_conversation::SessionId,
    mut result: ToolResult,
    store: &agentelier_session_store::SessionStoreClient,
) -> (ToolResult, Option<String>) {
    let agentelier_tools::ToolPayload::Image { summary, image } = &result.output else {
        return (result, None);
    };
    let stored = match store
        .save_image(session, summary.clone(), image.clone())
        .await
    {
        Ok(reference) => store.read_image(reference).await,
        Err(failure) => Err(failure),
    };
    match stored {
        Ok(image) => {
            result.output = agentelier_tools::ToolPayload::Image {
                summary: summary.clone(),
                image,
            }
        }
        Err(failure) => {
            return (result, Some(failure.to_string()));
        }
    }
    (result, None)
}
