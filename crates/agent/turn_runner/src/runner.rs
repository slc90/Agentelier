use crate::dispatch::{ToolBatch, ToolCommand, ToolDispatcher, ToolEvent};
use crate::{
    access::{RunnerEvent, TurnAccess},
    control,
};
use agentelier_account::{CredentialClient, ExecutionIdentity};
use agentelier_conversation::RecordEvent;
use agentelier_conversation::{
    ConversationFailure, InputDecision, MAX_TURN_REQUESTS, SteerInput, TurnStatus,
};
use agentelier_llm_model::{GenerationResult, ModelFailure, ModelFailureKind};
use agentelier_tools::{ExecutionContext, ToolCatalog, ToolResult, ToolStatus};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL_PROGRESS_CAPACITY: usize = 32;
const MAX_TOOL_CALLS: usize = 64;

async fn publish(
    progress: &mpsc::Sender<RunnerEvent>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    tokio::select! { biased;
        _ = cancel.cancelled() => Ok(()),
        result = progress.send(RunnerEvent::Progress) => result.map_err(|_| ConversationFailure::new("任务进度接收端已关闭")),
    }
}

async fn request_once(
    active: &TurnAccess,
    dispatcher: &ToolDispatcher,
    credentials: CredentialClient,
    identity: &ExecutionIdentity,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
    steers: &mut mpsc::Receiver<SteerInput>,
) -> Result<Option<Result<GenerationResult, ModelFailure>>, ConversationFailure> {
    let (sender, mut events) = mpsc::channel(MODEL_PROGRESS_CAPACITY);
    let request = match active.call(|mut turn| turn.request()).await {
        Ok(request) => request,
        Err(failure) => return Ok(Some(Err(protocol_failure(failure)))),
    };
    let prepared = match agentelier_llm_model::prepare(request) {
        Ok(prepared) => prepared,
        Err(failure) => return Ok(Some(Err(failure))),
    };
    let projection = prepared.projection().clone();
    let event = active
        .call(move |mut turn| {
            turn.request_prepared(projection)?;
            Ok(RecordEvent::RequestPrepared {
                state: turn.stored_state(),
            })
        })
        .await?;
    active.record(event).await?;
    if cancel.is_cancelled() {
        return Ok(None);
    }
    if dispatcher.is_blocked() {
        return Err(ConversationFailure::new(
            "工具资源回收未确认，当前会话已暂停执行",
        ));
    }
    publish(progress, cancel).await?;
    let mut stopped_by_resources = false;
    let outcome = {
        let generation = agentelier_llm_model::generate(credentials, prepared, identity, sender);
        tokio::pin!(generation);
        loop {
            tokio::select! { biased;
                _ = dispatcher.blocked() => {
                    stopped_by_resources = true;
                    break None;
                },
                result = &mut generation => break Some(result),
                _ = cancel.cancelled() => break None,
                input = steers.recv(), if !steers.is_closed() => {
                    if let Some(input) = input {control::accept(active,input,progress).await?;publish(progress,cancel).await?;}
                }
                event = events.recv() => {
                    if let Some(event) = event {
                        if let Err(failure) = apply_stream_event(active, event).await? {
                            break Some(Err(failure));
                        }
                        publish(progress, cancel).await?;
                    }
                }
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        if let Err(failure) = apply_stream_event(active, event).await? {
            return Ok(Some(Err(failure)));
        }
    }
    if stopped_by_resources {
        return Err(ConversationFailure::new(
            "工具资源回收未确认，当前会话已暂停执行",
        ));
    }
    Ok(outcome)
}

async fn apply_stream_event(
    active: &TurnAccess,
    event: agentelier_llm_model::StreamEvent,
) -> Result<Result<(), ModelFailure>, ConversationFailure> {
    let progress = matches!(
        event,
        agentelier_llm_model::StreamEvent::Sending
            | agentelier_llm_model::StreamEvent::ResponseReceived { .. }
    );
    match active
        .call(move |mut turn| {
            turn.update(event)?;
            Ok(progress.then(|| RecordEvent::RequestProgress {
                state: turn.stored_state(),
            }))
        })
        .await
    {
        Ok(Some(event)) => {
            active.record(event).await?;
        }
        Ok(None) => {}
        Err(failure) => return Ok(Err(protocol_failure(failure))),
    }
    Ok(Ok(()))
}
fn protocol_failure(failure: ConversationFailure) -> ModelFailure {
    ModelFailure {
        usage: None,
        response_id: None,
        kind: ModelFailureKind::Protocol,
        message: failure.to_string(),
        status: None,
        code: None,
        request_id: None,
    }
}

pub async fn run(
    credentials: CredentialClient,
    cancel: CancellationToken,
    progress: mpsc::Sender<RunnerEvent>,
    mut steers: mpsc::Receiver<SteerInput>,
    user_skills_root: std::path::PathBuf,
    memory: Option<agentelier_memory::MemoryClient>,
    dispatcher: ToolDispatcher,
) -> Result<(), ConversationFailure> {
    let active = TurnAccess::new(progress.clone());
    let event = active
        .call(|turn| {
            let state = turn.stored_state();
            Ok(match turn.continuation() {
                Some(source) => RecordEvent::ContinuedWithoutCompaction { state, source },
                None => RecordEvent::Accepted { state },
            })
        })
        .await?;
    let admitted = active.record(event).await;
    if let Ok(sequence) = admitted {
        active
            .call(move |mut turn| {
                turn.recorded(sequence);
                Ok(())
            })
            .await?;
    }
    let outcome = if admitted.is_ok() {
        run_task(
            &active,
            &dispatcher,
            credentials,
            &cancel,
            &progress,
            &mut steers,
            user_skills_root,
            memory,
        )
        .await
    } else {
        admitted.map(|_| TurnStatus::Cancelled)
    };
    let phase = active.call(|mut turn| turn.reaping()).await;
    let projection = if phase.is_ok() {
        publish(&progress, &cancel).await
    } else {
        phase
    };
    let reason = match &outcome {
        Ok(TurnStatus::Completed) => "task_completed",
        Ok(TurnStatus::Cancelled) => "task_stopped",
        Ok(TurnStatus::Limited) => "task_budget_exhausted",
        _ => "task_failed",
    };
    let report = dispatcher.cleanup(reason).await?;
    let cleanup_failed = report.failed;
    active
        .call(move |mut turn| turn.process_cleanup(report))
        .await?;
    control::close(&mut steers, &progress).await?;
    let status = match outcome.and_then(|status| projection.map(|()| status)) {
        Ok(_) if cleanup_failed => TurnStatus::Failed("任务进程清理失败，执行已暂停".into()),
        Ok(status) => status,
        Err(failure) => {
            active
                .call(|mut turn| {
                    turn.block_execution();
                    Ok(())
                })
                .await?;
            TurnStatus::Failed(failure.to_string())
        }
    };
    let finalized = active
        .call(move |mut turn| {
            turn.finish(status)?;
            if !turn.history_confirmed() || turn.has_open_calls() {
                return Err(ConversationFailure::new(
                    "模型历史有未确认的提交或待归并结果，请从存档重新加载后继续",
                ));
            }
            Ok(RecordEvent::Finalized {
                state: turn.stored_state(),
                history: turn.archive(),
            })
        })
        .await?;
    if let Err(failure) = active.record(finalized).await {
        active
            .call(move |mut turn| {
                turn.block_execution();
                turn.finish(TurnStatus::Failed(failure.to_string()))
            })
            .await?;
    }
    Ok(())
}

async fn run_task(
    active: &TurnAccess,
    dispatcher: &ToolDispatcher,
    credentials: CredentialClient,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
    steers: &mut mpsc::Receiver<SteerInput>,
    user_skills_root: std::path::PathBuf,
    memory: Option<agentelier_memory::MemoryClient>,
) -> Result<TurnStatus, ConversationFailure> {
    if cancel.is_cancelled() {
        return Ok(TurnStatus::Cancelled);
    }
    let (settings, input) = active
        .call(|turn| Ok((turn.settings(), turn.input()?)))
        .await?;
    if !active
        .call(|turn| Ok(turn.continuation().is_some()))
        .await?
    {
        let directory = settings.working_directory.clone();
        let loading_cancel = cancel.clone();
        let discovered = tokio::task::spawn_blocking(move || {
            let catalog = agentelier_skills::discover_standard(
                &directory,
                &user_skills_root,
                &loading_cancel,
            )?;
            let selected = agentelier_skills::select(&catalog, &input, &loading_cancel)?;
            Ok::<_, agentelier_skills::Failure>((catalog, selected))
        })
        .await
        .map_err(|failure| {
            ConversationFailure::new(format!("Skills 发现工作者异常结束：{failure}"))
        })?;
        if cancel.is_cancelled() {
            return Ok(TurnStatus::Cancelled);
        }
        match discovered {
            Ok((catalog, selected)) => {
                tracing::debug!(
                    skills = catalog.skills.len(),
                    diagnostics = catalog.diagnostics.len(),
                    roots = catalog.roots.len(),
                    "任务 Skills 目录发现完成"
                );
                for diagnostic in &catalog.diagnostics {
                    tracing::warn!(path = %diagnostic.path.display(), error = diagnostic.message, "Skills 来源不可用或受限");
                }
                if let Err(failure) = active
                    .call(move |mut turn| turn.skills(catalog, selected))
                    .await
                {
                    return Ok(TurnStatus::Failed(failure.to_string()));
                }
                publish(progress, cancel).await?;
            }
            Err(failure) => return Ok(TurnStatus::Failed(failure.to_string())),
        }
    }
    let identity = tokio::select! { biased;
        _ = cancel.cancelled() => return Ok(TurnStatus::Cancelled),
        result = credentials.execution() => match result { Ok(identity) => identity, Err(failure) => return Ok(TurnStatus::Failed(failure.to_string())) },
    };
    let slug = settings.slug;
    let model = tokio::select! { biased;
        _ = cancel.cancelled() => return Ok(TurnStatus::Cancelled),
        result = agentelier_llm_model::task_model(credentials.clone(), &identity, &slug) => match result { Ok(model) => model, Err(failure) => return Ok(TurnStatus::Failed(failure.to_string())) },
    };
    if let Err(failure) = active.call(move |mut turn| turn.model_context(model)).await {
        return Ok(TurnStatus::Failed(failure.to_string()));
    }
    let catalog = active.call(|turn| Ok(turn.catalog())).await?;
    let (mut requests_used, mut tools_used) = active.call(|turn| turn.spent_budget()).await?;
    while requests_used < MAX_TURN_REQUESTS {
        append_saved_results(active).await?;
        control::drain(active, steers, progress).await?;
        if cancel.is_cancelled() {
            return Ok(TurnStatus::Cancelled);
        }
        if dispatcher.is_blocked() {
            return Err(ConversationFailure::new(
                "工具资源回收未确认，当前会话已暂停执行",
            ));
        }
        if let Some(status) = collect_project(active, cancel, progress).await? {
            return Ok(status);
        }
        let project = active
            .call(|mut turn| {
                let Some(candidate) = turn.project_candidate()? else {
                    return Ok(None);
                };
                let event = candidate.event(&turn);
                turn.begin_history_commit();
                Ok(Some((candidate, event)))
            })
            .await?;
        if let Some((candidate, event)) = project {
            let sequence = active.record(event).await?;
            active
                .call(move |mut turn| {
                    let changes_history = candidate.changes_history();
                    turn.install_project(candidate);
                    if changes_history {
                        turn.recorded(sequence);
                    }
                    Ok(())
                })
                .await?;
        }
        let applied = active
            .call(|mut turn| {
                let ids = turn.apply_steers()?;
                if ids.is_empty() {
                    return Ok(None);
                }
                let event = RecordEvent::SteerApplied {
                    state: turn.stored_state(),
                    history: turn.archive(),
                };
                Ok(Some((ids, event)))
            })
            .await?;
        if let Some((ids, event)) = applied {
            let sequence = active.record(event).await?;
            active
                .call(move |mut turn| {
                    turn.recorded(sequence);
                    Ok(())
                })
                .await?;
            for id in ids {
                control::receipt(progress, id, InputDecision::Applied).await?;
            }
        }
        requests_used += 1;
        let epoch = memory
            .as_ref()
            .and_then(agentelier_memory::MemoryClient::epoch);
        let user_turn = active
            .call(move |mut turn| {
                turn.use_memory_epoch(epoch);
                turn.tool_user_turn(epoch)
            })
            .await?;
        let context = Arc::new(ExecutionContext::new(
            settings.working_directory.clone(),
            settings.image_input,
            Arc::new(user_turn),
            requests_used,
        ));
        if active.call(|turn| Ok(turn.has_open_calls())).await? {
            return Err(ConversationFailure::new(
                "本阶段须确认整批工具结果后再发请求",
            ));
        }
        let result = match request_once(
            active,
            dispatcher,
            credentials.clone(),
            &identity,
            cancel,
            progress,
            steers,
        )
        .await?
        {
            Some(Ok(result)) => result,
            Some(Err(failure)) => return Ok(TurnStatus::Failed(failure.to_string())),
            None => return Ok(TurnStatus::Cancelled),
        };
        let limited = result.calls.len() > MAX_TOOL_CALLS.saturating_sub(tools_used);
        let (result, calls, registrations, acceptance, event) = match active
            .call(move |mut turn| {
                if turn.reject_unsafe_compaction(&result)? {
                    return Ok(None);
                }
                let (registrations, acceptance) = turn.accept_response(&result, epoch, !limited)?;
                let event = RecordEvent::Response {
                    state: turn.stored_state(),
                    output: result.output.archive(),
                    registrations: Some(registrations.clone()),
                    acceptance: Some(acceptance.clone()),
                };
                turn.begin_history_commit();
                let mut result = result;
                let calls = std::mem::take(&mut result.calls);
                Ok(Some((result, calls, registrations, acceptance, event)))
            })
            .await
        {
            Ok(Some(accepted)) => accepted,
            Ok(None) => {
                let source = context.user_turn();
                tracing::warn!(session = %source.session, turn = source.turn, request = requests_used,
                    "拒绝在冻结边界不允许的压缩响应");
                return Ok(TurnStatus::Failed(
                    "压缩响应越过开放调用或本次请求禁止压缩，已保留原历史".into(),
                ));
            }
            Err(failure) => return Ok(TurnStatus::Failed(failure.to_string())),
        };
        let response_sequence = active.record(event).await?;
        let (compaction, tasks_changed, rejected) = active
            .call(move |mut turn| {
                let compaction =
                    turn.confirm_response_history(result, acceptance.append, response_sequence)?;
                let changed = turn.confirm_tasks(
                    registrations,
                    &acceptance.delivery.settled,
                    response_sequence,
                )?;
                Ok((compaction, changed, acceptance.delivery.rejected))
            })
            .await?;
        if tasks_changed {
            active.publish_tasks().await?;
        }
        publish(progress, cancel).await?;
        if let Some(metadata) = compaction {
            checkpoint_response(active, metadata, progress, cancel).await?;
        }
        control::drain(active, steers, progress).await?;
        if calls.is_empty() {
            close_response(active, response_sequence, progress, cancel).await?;
            if cancel.is_cancelled() {
                return Ok(TurnStatus::Cancelled);
            }
            control::drain(active, steers, progress).await?;
            if !active
                .call(|mut turn| Ok(turn.close_steers_if_idle()))
                .await?
            {
                continue;
            }
            return Ok(TurnStatus::Completed);
        }
        tools_used += calls.len();
        let cleanup_failed = execute_batch(
            active,
            dispatcher,
            catalog.clone(),
            context,
            calls,
            limited,
            &rejected,
            cancel,
            progress,
            steers,
        )
        .await?;
        close_response(active, response_sequence, progress, cancel).await?;
        if cleanup_failed {
            active
                .call(|mut turn| {
                    turn.block_execution();
                    Ok(())
                })
                .await?;
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

async fn execute_batch(
    active: &TurnAccess,
    dispatcher: &ToolDispatcher,
    catalog: Arc<ToolCatalog>,
    context: Arc<ExecutionContext>,
    calls: Vec<agentelier_llm_model::FunctionCall>,
    limited: bool,
    rejected: &[String],
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
    steers: &mut mpsc::Receiver<SteerInput>,
) -> Result<bool, ConversationFailure> {
    let session = context.user_turn().session;
    let mut eligible = Vec::new();
    for call in calls {
        let skip = if limited {
            Some(ToolResult::not_executed(
                "tool_budget",
                "本批调用超过任务限额，整批未执行",
            ))
        } else if rejected.contains(&call.call_id) {
            Some(ToolResult::not_executed(
                "session_delivery_budget",
                "会话异步任务交付额度已满，此调用未执行",
            ))
        } else {
            None
        };
        if let Some(result) = skip {
            save_call(
                active,
                dispatcher,
                session,
                &call.call_id,
                None,
                result,
                progress,
                cancel,
            )
            .await?;
        } else {
            eligible.push(call);
        }
    }
    if eligible.is_empty() {
        return Ok(false);
    }
    let ids: Vec<_> = eligible.iter().map(|call| call.call_id.clone()).collect();
    let lookup = ids.clone();
    let executions = active
        .call(move |turn| {
            Ok(lookup
                .iter()
                .map(|id| turn.task_execution(id))
                .collect::<Vec<_>>())
        })
        .await?;
    let admission = cancel.child_token();
    if active.call(|turn| Ok(turn.has_pending_steer())).await? {
        admission.cancel();
    }
    let (events, mut incoming) = mpsc::unbounded_channel();
    dispatcher
        .send(ToolCommand::Batch(ToolBatch {
            calls: eligible,
            catalog,
            context,
            admission: admission.clone(),
            events,
        }))
        .await?;
    let mut remaining = ids.len();
    let mut failure = None;
    let mut cleanup_failed = false;
    while remaining > 0 {
        let result = tokio::select! {
            biased;
            event = incoming.recv() => {
                match event {
                    Some(ToolEvent::Ready { index, begin }) => {
                        let started = match ids.get(index).zip(executions.get(index)) {
                            Some((id, execution)) if !admission.is_cancelled() && !dispatcher.is_blocked() => {
                                start_call(active, id.clone(), *execution).await
                            }
                            Some(_) => Ok(false),
                            None => Err(ConversationFailure::new("工具准入不属于当前批次")),
                        };
                        let should_begin = matches!(&started, Ok(true));
                        if begin.send(should_begin).is_err() && should_begin {
                            Err(ConversationFailure::new("工具句柄无法接收已确认的开始记录"))
                        } else if should_begin {
                            publish(progress, cancel).await
                        } else {
                            started.map(|_| ())
                        }
                    }
                    Some(ToolEvent::Finished { index, result }) => {
                        remaining -= 1;
                        cleanup_failed |= result.cleanup_failed || result.status == ToolStatus::Unconfirmed;
                        if cleanup_failed {
                            admission.cancel();
                        }
                        match ids.get(index).zip(executions.get(index)) {
                            Some((id, execution)) => {
                                if failure.is_some() {
                                    retain_call(active, dispatcher, id.clone(), result).await.map(|_| ())
                                } else {
                                    save_call(active, dispatcher, session, id, *execution, result, progress, cancel).await
                                }
                            }
                            None => Err(ConversationFailure::new("工具终态不属于当前批次")),
                        }
                    }
                    None => return Err(failure.unwrap_or_else(|| ConversationFailure::new("工具批次结束前仍有调用未回收"))),
                }
            }
            input = steers.recv(), if !steers.is_closed() && failure.is_none() => match input {
                Some(input) => {
                    match control::accept(active, input, progress).await {
                        Ok(()) => {
                            match active.call(|turn| Ok(turn.has_pending_steer())).await {
                                Ok(true) => { admission.cancel(); publish(progress, cancel).await }
                                Ok(false) => Ok(()),
                                Err(failure) => Err(failure),
                            }
                        }
                        Err(failure) => Err(failure),
                    }
                }
                None => Ok(()),
            }
        };
        if let Err(error) = result {
            admission.cancel();
            failure = failure.or(Some(error));
        }
    }
    failure.map_or(Ok(cleanup_failed || dispatcher.is_blocked()), Err)
}

async fn start_call(
    active: &TurnAccess,
    call_id: String,
    execution: Option<agentelier_conversation::TaskExecution>,
) -> Result<bool, ConversationFailure> {
    let source = call_id.clone();
    let event = active
        .call(move |mut turn| {
            if turn.has_pending_steer() {
                return Ok(None);
            }
            turn.tool_started(&source)?;
            Ok(Some(RecordEvent::ToolStarted {
                state: turn.stored_state(),
                call_id: source,
                execution,
            }))
        })
        .await?;
    let Some(event) = event else {
        return Ok(false);
    };
    let sequence = active.record(event).await?;
    if active
        .call(move |mut turn| {
            turn.confirm_task_status(&call_id, execution, ToolStatus::Running, sequence)
        })
        .await?
    {
        active.publish_tasks().await?;
    }
    Ok(true)
}

async fn retain_call(
    active: &TurnAccess,
    dispatcher: &ToolDispatcher,
    call_id: String,
    result: ToolResult,
) -> Result<Option<(ToolStatus, agentelier_tools::ToolPayload)>, ConversationFailure> {
    let id = call_id.clone();
    let outcome = active
        .call(move |mut turn| turn.tool_result(&id, result))
        .await?;
    dispatcher.send(ToolCommand::Accepted(call_id)).await?;
    Ok(outcome)
}

async fn save_call(
    active: &TurnAccess,
    dispatcher: &ToolDispatcher,
    session: agentelier_conversation::SessionId,
    call_id: &str,
    execution: Option<agentelier_conversation::TaskExecution>,
    result: ToolResult,
    progress: &mpsc::Sender<RunnerEvent>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    let (result, storage_failure) = persist_image(active, session, result).await;
    let outcome = retain_call(active, dispatcher, call_id.to_owned(), result).await?;
    if let Some(failure) = storage_failure {
        return Err(ConversationFailure::new(format!(
            "图片已读取，附件保存失败，停止后续执行：{failure}"
        )));
    }
    if let Some((status, output)) = outcome {
        let id = call_id.to_owned();
        let event = active
            .call(move |turn| {
                Ok(RecordEvent::ToolReturned {
                    state: turn.stored_state(),
                    call_id: id,
                    status,
                    output: agentelier_llm_model::ArchivedPayload::capture(&output),
                    execution,
                })
            })
            .await?;
        let sequence = active.record(event).await?;
        let id = call_id.to_owned();
        if active
            .call(move |mut turn| turn.confirm_result_saved(&id, execution, status, sequence))
            .await?
        {
            active.publish_tasks().await?;
        }
    }
    append_saved_results(active).await?;
    publish(progress, cancel).await
}

async fn append_saved_results(active: &TurnAccess) -> Result<(), ConversationFailure> {
    while let Some((call_id, saved_sequence, append, event)) = active
        .call(|mut turn| {
            let Some((call_id, saved_sequence)) = turn.next_saved_result() else {
                return Ok(None);
            };
            let append = turn.result_append(&call_id, saved_sequence)?;
            let event = RecordEvent::ResultAppended {
                state: turn.stored_state(),
                call_id: call_id.clone(),
                saved_sequence,
                append,
            };
            turn.begin_history_commit();
            Ok(Some((call_id, saved_sequence, append, event)))
        })
        .await?
    {
        let sequence = active.record(event).await?;
        active
            .call(move |mut turn| {
                turn.confirm_result_append(call_id, saved_sequence, append, sequence)
            })
            .await?;
    }
    Ok(())
}

async fn persist_image(
    active: &TurnAccess,
    session: agentelier_conversation::SessionId,
    mut result: ToolResult,
) -> (ToolResult, Option<String>) {
    let agentelier_tools::ToolPayload::Image { summary, image } = &result.output else {
        return (result, None);
    };
    let stored = active
        .persist_image(session, summary.clone(), image.clone())
        .await;
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

async fn close_response(
    active: &TurnAccess,
    response_sequence: u64,
    progress: &mpsc::Sender<RunnerEvent>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    let event = active
        .call(move |mut turn| {
            if turn.has_open_calls() {
                return Err(ConversationFailure::new("本批工具结果尚未收齐"));
            }
            turn.begin_history_commit();
            Ok(RecordEvent::ResponseClosed {
                state: turn.stored_state(),
                response_sequence,
            })
        })
        .await?;
    let sequence = active.record(event).await?;
    active
        .call(move |mut turn| turn.confirm_response_closed(response_sequence, sequence))
        .await?;
    publish(progress, cancel).await
}

async fn checkpoint_response(
    active: &TurnAccess,
    metadata: agentelier_llm_model::CompactionMetadata,
    progress: &mpsc::Sender<RunnerEvent>,
    cancel: &CancellationToken,
) -> Result<(), ConversationFailure> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    let candidate = active
        .call(move |mut turn| turn.compaction_candidate(metadata))
        .await?;
    publish(progress, cancel).await?;
    if cancel.is_cancelled() {
        return Ok(());
    }
    let (candidate, event) = active
        .call(move |mut turn| {
            let event = candidate.event(&turn)?;
            turn.begin_history_commit();
            Ok((candidate, event))
        })
        .await?;
    let sequence = active.record(event).await?;
    let (session, window) = active
        .call(move |mut turn| {
            turn.install_compaction(candidate)?;
            turn.recorded(sequence);
            Ok((turn.session_id(), turn.context_window()))
        })
        .await?;
    tracing::info!(session = %session, window, sequence, "服务端压缩检查点已提交并安装");
    Ok(())
}

async fn collect_project(
    active: &TurnAccess,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<RunnerEvent>,
) -> Result<Option<TurnStatus>, ConversationFailure> {
    let directory = active
        .call(|mut turn| {
            turn.loading_project();
            Ok(turn.settings().working_directory)
        })
        .await?;
    publish(progress, cancel).await?;
    let loading_cancel = cancel.clone();
    // Join the worker even after Stop so no file operation outlives its owning task.
    let collected = tokio::task::spawn_blocking(move || {
        agentelier_project_context::collect(&directory, &loading_cancel)
    })
    .await
    .map_err(|failure| {
        ConversationFailure::new(format!("项目规则采集工作者异常结束：{failure}"))
    })?;
    if cancel.is_cancelled() {
        return Ok(Some(TurnStatus::Cancelled));
    }
    match collected {
        Ok(snapshot) => {
            tracing::debug!(sources = snapshot.sources.len(), source_bytes = snapshot.source_bytes, hash = %snapshot.hash, "项目规则采集完成");
            active
                .call(move |mut turn| {
                    turn.project_context(Ok(snapshot));
                    Ok(())
                })
                .await?;
        }
        Err(agentelier_project_context::ProjectContextFailure::Cancelled) => {
            return Ok(Some(TurnStatus::Cancelled));
        }
        Err(failure) => {
            tracing::error!(error = %failure, "项目规则采集失败，本次模型请求未启动");
            let message = failure.to_string();
            let error = message.clone();
            active
                .call(move |mut turn| {
                    turn.project_context(Err(error));
                    Ok(())
                })
                .await?;
            publish(progress, cancel).await?;
            return Ok(Some(TurnStatus::Failed(message)));
        }
    }
    publish(progress, cancel).await?;
    Ok(None)
}
