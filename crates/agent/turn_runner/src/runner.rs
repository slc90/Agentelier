use crate::dispatch::{SessionTaskCommand, ToolAuthorization, ToolDispatcher, ToolEvent};
use crate::tool_events::ToolWork;
use crate::{
    access::{RunnerEvent, TurnAccess},
    control,
    tool_events::TurnTools,
};
use agentelier_account::{CredentialClient, ExecutionIdentity};
use agentelier_conversation::RecordEvent;
use agentelier_conversation::{
    ConversationFailure, InputDecision, MAX_TURN_REQUESTS, SteerInput, TurnStatus,
};
use agentelier_llm_model::{
    GenerationConnection, GenerationResult, ModelFailure, ModelFailureKind,
};
use agentelier_tools::ExecutionContext;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL_PROGRESS_CAPACITY: usize = 32;
const MAX_TOOL_CALLS: usize = 64;

pub(super) async fn publish(
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
    jobs: &mut TurnTools,
    connection: &mut GenerationConnection,
) -> Result<
    Option<Result<(GenerationResult, GenerationConnection), ModelFailure>>,
    ConversationFailure,
> {
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
        let generation = agentelier_llm_model::generate(
            std::mem::take(connection),
            credentials,
            prepared,
            identity,
            sender,
        );
        tokio::pin!(generation);
        loop {
            tokio::select! { biased;
                event = jobs.work.incoming.recv(), if jobs.has_calls() => {
                    jobs.apply(active, dispatcher, event, progress, cancel).await?;
                }
                command = jobs.controls.recv(), if !jobs.controls.is_closed() => {
                    if let Some(command) = command {
                        jobs.command(active, dispatcher, command, progress, cancel).await?;
                    }
                }
                _ = dispatcher.blocked() => {
                    stopped_by_resources = true;
                    break None;
                },
                result = &mut generation => break Some(result),
                _ = cancel.cancelled() => break None,
                input = steers.recv(), if !steers.is_closed() => {
                    if let Some(input) = input {
                        jobs.accept_steer(active, input, progress, cancel).await?;
                    }
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
        return Ok(Some(Err(protocol_failure(ConversationFailure::new(
            "工具资源回收未确认，当前会话已暂停执行",
        )))));
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
    work: Arc<tokio::sync::Mutex<ToolWork>>,
    controls: mpsc::Receiver<SessionTaskCommand>,
    connection: Arc<tokio::sync::Mutex<GenerationConnection>>,
) -> Result<(), ConversationFailure> {
    let active = TurnAccess::new(progress.clone());
    let (turn_id, native) = active
        .call(|turn| Ok((turn.turn_id()?, turn.supports_detached_work())))
        .await?;
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
    let mut jobs = TurnTools::new(work.lock_owned().await, controls, turn_id);
    let mut session_connection = connection.lock().await;
    let mut connection = std::mem::take(&mut *session_connection);
    let mut outcome = if admitted.is_ok() {
        run_task(
            &active,
            &dispatcher,
            credentials,
            &cancel,
            &progress,
            &mut steers,
            user_skills_root,
            memory,
            &mut jobs,
            &mut connection,
        )
        .await
    } else {
        admitted.map(|_| TurnStatus::Cancelled)
    };
    let detach = native && outcome.is_ok() && !cancel.is_cancelled() && !dispatcher.is_blocked();
    let projection = if detach {
        Ok(())
    } else {
        if dispatcher.is_blocked() {
            jobs.work.stop_all();
        }
        jobs.stop_own();
        active.call(|mut turn| turn.reaping()).await?;
        publish(&progress, &cancel).await
    };
    let reason = match &outcome {
        Ok(TurnStatus::Completed) => "task_completed",
        Ok(TurnStatus::Cancelled) => "task_stopped",
        Ok(TurnStatus::Limited) => "task_budget_exhausted",
        _ => "task_failed",
    };
    let projection = projection.and(control::close(&mut steers, &progress).await);
    if !detach && outcome.is_ok() && projection.is_ok() {
        if let Err(failure) = jobs
            .reap(&active, &dispatcher, &progress, &cancel, reason)
            .await
        {
            outcome = Err(failure);
        } else if cancel.is_cancelled() {
            outcome = Ok(TurnStatus::Cancelled);
        }
    }
    if outcome.is_ok()
        && projection.is_ok()
        && let Err(failure) = jobs
            .close_commands(&active, &dispatcher, &progress, &cancel)
            .await
    {
        outcome = Err(failure);
    }
    let cleanup_failed = if detach && !cancel.is_cancelled() {
        false
    } else {
        if detach {
            jobs.stop_own();
            jobs.reap(&active, &dispatcher, &progress, &cancel, "task_stopped")
                .await?;
            outcome = Ok(TurnStatus::Cancelled);
        }
        let report = dispatcher
            .reap_processes(reason, agentelier_tools::ProcessScope::Turn(turn_id))
            .await?;
        let failed = report.failed;
        active
            .call(move |mut turn| turn.process_cleanup(report))
            .await?;
        failed
    };
    drop(jobs);
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
    let keep_connection = matches!(status, TurnStatus::Completed | TurnStatus::Limited);
    let finalized = active
        .call(move |mut turn| {
            turn.finish(status)?;
            if !turn.history_confirmed() || (!native && turn.has_open_calls()) {
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
        dispatcher.block();
        active
            .call(move |mut turn| {
                turn.block_execution();
                turn.finish(TurnStatus::Failed(failure.to_string()))
            })
            .await?;
    } else if keep_connection {
        *session_connection = connection;
    }
    Ok(())
}

pub enum IdleAction {
    Tool(ToolEvent),
    Control(SessionTaskCommand),
}

pub async fn run_idle(
    work: Arc<tokio::sync::Mutex<ToolWork>>,
    dispatcher: ToolDispatcher,
    action: IdleAction,
    progress: mpsc::Sender<RunnerEvent>,
) -> Result<(), ConversationFailure> {
    let active = TurnAccess::new(progress.clone());
    let (unused, controls) = mpsc::channel(1);
    drop(unused);
    let mut jobs = TurnTools::new(work.lock_owned().await, controls, 0);
    let cancel = CancellationToken::new();
    match action {
        IdleAction::Tool(event) => {
            jobs.apply(&active, &dispatcher, Some(event), &progress, &cancel)
                .await?
        }
        IdleAction::Control(command) => {
            jobs.command(&active, &dispatcher, command, &progress, &cancel)
                .await?
        }
    }
    if active.call(|turn| Ok(turn.history_confirmed())).await? {
        jobs.boundary(&active, &dispatcher, &progress, &cancel)
            .await?;
    }
    publish(&progress, &cancel).await
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
    jobs: &mut TurnTools,
    connection: &mut GenerationConnection,
) -> Result<TurnStatus, ConversationFailure> {
    if cancel.is_cancelled() {
        return Ok(TurnStatus::Cancelled);
    }
    let (settings, input, child) = active
        .call(|turn| Ok((turn.settings(), turn.input()?, turn.child_materials())))
        .await?;
    if settings.kind.is_user()
        && !active
            .call(|turn| Ok(turn.continuation().is_some()))
            .await?
    {
        let loading_cancel = cancel.clone();
        let include_builtin = active
            .call(|turn| {
                Ok(turn
                    .catalog()
                    .definitions()
                    .any(|tool| tool.name == agentelier_skills::READ_RESOURCE_TOOL))
            })
            .await?;
        let frozen_skills = child.as_ref().map(|materials| materials.skills.clone());
        let discovered = tokio::task::spawn_blocking(move || {
            let catalog = match frozen_skills {
                Some(catalog) => {
                    let mut catalog = catalog.as_ref().clone();
                    catalog.exclude_archived();
                    catalog
                }
                None => agentelier_skills::discover_standard(
                    &user_skills_root,
                    include_builtin,
                    &loading_cancel,
                )?,
            };
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
    if let Err(failure) = settings.validate_source(identity.identity()) {
        return Ok(TurnStatus::Failed(failure.to_string()));
    }
    let tool_authorization = ToolAuthorization::new(credentials.clone(), identity.clone());
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
        jobs.boundary(active, dispatcher, progress, cancel).await?;
        control::drain(active, steers, progress).await?;
        jobs.stop_admission_for_steer(active).await?;
        if cancel.is_cancelled() {
            return Ok(TurnStatus::Cancelled);
        }
        if dispatcher.is_blocked() {
            return Err(ConversationFailure::new(
                "工具资源回收未确认，当前会话已暂停执行",
            ));
        }
        if settings.kind.is_user()
            && child.is_none()
            && let Some(status) = collect_project(active, cancel, progress).await?
        {
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
        active.apply_inbox().await?;
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
        jobs.boundary(active, dispatcher, progress, cancel).await?;
        let (result, next_connection) = match request_once(
            active,
            dispatcher,
            credentials.clone(),
            &identity,
            cancel,
            progress,
            steers,
            jobs,
            connection,
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
        *connection = next_connection;
        if tasks_changed {
            active.publish_tasks().await?;
        }
        publish(progress, cancel).await?;
        if let Some(metadata) = compaction {
            checkpoint_response(active, metadata, progress, cancel).await?;
        }
        control::drain(active, steers, progress).await?;
        jobs.boundary(active, dispatcher, progress, cancel).await?;
        if calls.is_empty() {
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
        jobs.dispatch(
            active,
            dispatcher,
            catalog.clone(),
            context,
            calls,
            limited,
            &rejected,
            &tool_authorization,
            progress,
            cancel,
        )
        .await?;
        jobs.wait_for_sync(active, dispatcher, progress, cancel, steers)
            .await?;
        if dispatcher.is_blocked() {
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
