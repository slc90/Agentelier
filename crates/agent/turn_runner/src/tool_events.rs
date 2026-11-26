use crate::{
    access::{GeneratedImageFeedback, RunnerEvent, TurnAccess},
    control,
    dispatch::{
        SessionTaskAction, SessionTaskCommand, SessionTaskOutcome, ToolAuthorization, ToolBatch,
        ToolCommand, ToolDispatcher, ToolEvent, ToolJobRequest,
    },
    runner::publish,
    task_control::{self, PendingWait, TaskControlAction, WaitInterruption},
};
use agentelier_conversation::{
    ConversationFailure, GeneratedImage, RecordEvent, SteerInput, TaskCancellationPlan,
    TaskPolicyChange,
};
use agentelier_tools::{
    ExecutionContext, GENERATED_IMAGE_MODEL, ToolCatalog, ToolMode, ToolPayload, ToolResult,
    ToolStatus,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct ToolWork {
    calls: HashMap<String, PendingCall>,
    pub(super) incoming: mpsc::UnboundedReceiver<ToolEvent>,
}

impl ToolWork {
    pub fn new(incoming: mpsc::UnboundedReceiver<ToolEvent>) -> Self {
        Self {
            calls: HashMap::new(),
            incoming,
        }
    }

    pub fn has_calls(&self) -> bool {
        !self.calls.is_empty()
    }

    pub async fn next(&mut self) -> Option<ToolEvent> {
        self.incoming.recv().await
    }

    pub fn stop_all(&self) {
        for call in self.calls.values() {
            call.stop.cancel();
        }
    }
}

pub(super) struct TurnTools {
    pub(super) work: tokio::sync::OwnedMutexGuard<ToolWork>,
    waits: Vec<PendingWait>,
    pub(super) controls: mpsc::Receiver<SessionTaskCommand>,
    turn: u64,
}

struct PendingCall {
    turn: u64,
    mode: ToolMode,
    execution: Option<agentelier_conversation::TaskExecution>,
    admission: CancellationToken,
    stop: CancellationToken,
}

impl TurnTools {
    pub(super) fn new(
        work: tokio::sync::OwnedMutexGuard<ToolWork>,
        controls: mpsc::Receiver<SessionTaskCommand>,
        turn: u64,
    ) -> Self {
        Self {
            work,
            waits: Vec::new(),
            controls,
            turn,
        }
    }

    pub(super) fn has_calls(&self) -> bool {
        self.work.has_calls()
    }

    fn has_own_calls(&self) -> bool {
        self.work.calls.values().any(|call| call.turn == self.turn)
    }

    pub(super) fn stop_own(&self) {
        for call in self
            .work
            .calls
            .values()
            .filter(|call| call.turn == self.turn)
        {
            call.stop.cancel();
        }
    }

    pub(super) async fn dispatch(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        catalog: Arc<ToolCatalog>,
        context: Arc<ExecutionContext>,
        calls: Vec<agentelier_llm_model::FunctionCall>,
        limited: bool,
        rejected: &[String],
        authorization: &ToolAuthorization,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        let session = context.user_turn().session;
        let mut eligible = Vec::new();
        let mut controls = Vec::new();
        let steered = active.call(|turn| Ok(turn.has_pending_steer())).await?;
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
            } else if let Some(control) = catalog.task_control(&call.name) {
                controls.push((call, control));
            } else {
                eligible.push(call);
            }
        }
        let ids: Vec<_> = eligible.iter().map(|call| call.call_id.clone()).collect();
        let executions = active
            .call(move |turn| {
                Ok(ids
                    .iter()
                    .map(|id| {
                        (
                            turn.task_execution(id),
                            turn.task_cancellation_requested(id),
                        )
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        let mut jobs = Vec::new();
        for (call, (execution, cancelled)) in eligible.into_iter().zip(executions) {
            let mode = call.mode();
            let admission = cancel.child_token();
            let stop = cancel.child_token();
            if cancelled {
                stop.cancel();
            }
            if steered && mode == ToolMode::Synchronous {
                admission.cancel();
            }
            self.work.calls.insert(
                call.call_id.clone(),
                PendingCall {
                    turn: context.user_turn().turn,
                    mode,
                    execution,
                    admission: admission.clone(),
                    stop: stop.clone(),
                },
            );
            jobs.push(ToolJobRequest {
                call,
                admission,
                stop,
                execution,
            });
        }
        if !jobs.is_empty() {
            dispatcher
                .send(ToolCommand::Batch(ToolBatch {
                    calls: jobs,
                    catalog,
                    context,
                    authorization: authorization.clone(),
                }))
                .await?;
        }
        for (call, control) in controls {
            let id = call.call_id;
            let started = !cancel.is_cancelled()
                && !steered
                && start_call(active, id.clone(), None, true).await?;
            let checked = if started {
                tokio::select! { biased;
                    _ = cancel.cancelled() => None,
                    result = authorization.validate() => Some(result),
                }
            } else {
                None
            };
            let result = if let Some(Err(failure)) = checked {
                ToolResult::not_executed("authorization_changed", &failure.to_string())
            } else if matches!(checked, Some(Ok(()))) {
                match task_control::control(active, dispatcher, control, &call.arguments).await? {
                    TaskControlAction::Return(result) => result,
                    TaskControlAction::Cancel(targets) => {
                        match self.cancel_tasks(active, dispatcher, targets, true).await? {
                            Some(plan) => task_control::success(plan.receipts)?,
                            None => task_control::unknown_tasks(),
                        }
                    }
                    TaskControlAction::Wait(wait) => {
                        self.waits.push(PendingWait::new(id, wait));
                        continue;
                    }
                }
            } else {
                ToolResult::not_executed(
                    "admission_cancelled",
                    "停止或补充输入已接纳，此调用未执行",
                )
            };
            save_call(
                active, dispatcher, session, &id, None, result, progress, cancel,
            )
            .await?;
        }
        Ok(())
    }

    async fn cancel_tasks(
        &self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        targets: agentelier_tools::TaskTargets,
        from_model: bool,
    ) -> Result<Option<TaskCancellationPlan>, ConversationFailure> {
        let Some((plan, state)) = active
            .call(move |turn| {
                Ok(turn
                    .task_cancellation(&targets, from_model)
                    .map(|plan| (plan, turn.stored_state())))
            })
            .await?
        else {
            return Ok(None);
        };
        if !plan.targets.is_empty() {
            let mut stops = Vec::with_capacity(plan.targets.len());
            for target in &plan.targets {
                match self.work.calls.get(&target.call_id) {
                    Some(call) if call.execution == Some(target.execution) => {
                        stops.push(call.stop.clone())
                    }
                    None if plan.receipts.iter().any(|receipt| {
                        receipt.task_id == target.execution.task
                            && receipt.status == ToolStatus::Pending
                    }) => {}
                    _ => return Err(ConversationFailure::new("取消目标缺少实际执行句柄")),
                }
            }
            let event = RecordEvent::TaskCancellationRequested {
                state,
                targets: plan.targets.clone(),
            };
            let sequence = match active.record(event).await {
                Ok(sequence) => sequence,
                Err(failure) => {
                    dispatcher.block();
                    return Err(failure);
                }
            };
            let targets = plan.targets.clone();
            active
                .call(move |mut turn| turn.confirm_task_cancellation(&targets, sequence))
                .await?;
            for stop in stops {
                stop.cancel();
            }
            active.publish_tasks().await?;
        }
        Ok(Some(plan))
    }

    pub(super) async fn command(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        control: SessionTaskCommand,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        let result = match control.action {
            SessionTaskAction::Cancel(targets) => self
                .cancel_tasks(active, dispatcher, targets, false)
                .await
                .and_then(|plan| {
                    plan.map(|plan| SessionTaskOutcome::Cancellation(plan.receipts))
                        .ok_or_else(|| ConversationFailure::new("目标任务不存在或所属会话已变化"))
                }),
            SessionTaskAction::StopAll => self
                .stop_background(active, dispatcher)
                .await
                .map(|()| SessionTaskOutcome::StopRequested),
            SessionTaskAction::Resume if self.has_calls() => {
                Err(ConversationFailure::new("仍有任务尚未回收"))
            }
            SessionTaskAction::Resume => {
                Self::change_policy(active, dispatcher, TaskPolicyChange::Resume)
                    .await
                    .map(|()| SessionTaskOutcome::Resumed)
            }
            SessionTaskAction::ConfirmExternal { call_ids } => Self::change_policy(
                active,
                dispatcher,
                TaskPolicyChange::ConfirmExternal { call_ids },
            )
            .await
            .map(|()| SessionTaskOutcome::ExternalConfirmed),
        };
        drop(control.reply.send(result));
        publish(progress, cancel).await
    }

    async fn change_policy(
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        change: TaskPolicyChange,
    ) -> Result<(), ConversationFailure> {
        let intended = change.clone();
        let event = active
            .call(move |turn| turn.task_policy_record(intended))
            .await?;
        if let Err(failure) = active.record(event).await {
            dispatcher.block();
            return Err(failure);
        }
        let policy = active
            .call(move |mut turn| turn.apply_task_policy(&change))
            .await?;
        dispatcher.task_policy(policy);
        active.publish_tasks().await?;
        Ok(())
    }

    async fn stop_background(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
    ) -> Result<(), ConversationFailure> {
        Self::change_policy(active, dispatcher, TaskPolicyChange::Pause).await?;
        self.cancel_tasks(
            active,
            dispatcher,
            agentelier_tools::TaskTargets::All,
            false,
        )
        .await?;
        for call in self.work.calls.values() {
            call.stop.cancel();
        }
        Self::reap_stopped_processes(active, dispatcher).await
    }

    async fn reap_stopped_processes(
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
    ) -> Result<(), ConversationFailure> {
        let report = dispatcher
            .reap_processes("background_stopped", agentelier_tools::ProcessScope::All)
            .await?;
        let failed = report.failed;
        active
            .call(move |mut turn| turn.process_cleanup(report))
            .await?;
        if failed {
            return Err(ConversationFailure::new("后台进程回收未确认"));
        }
        Ok(())
    }

    pub(super) async fn apply(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        event: Option<ToolEvent>,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        match event {
            Some(ToolEvent::Ready { call_id, begin }) => {
                let call = self
                    .work
                    .calls
                    .get(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具准入缺少调用身份"))?;
                let started = if call.admission.is_cancelled()
                    || call.stop.is_cancelled()
                    || dispatcher.is_blocked()
                {
                    Ok(false)
                } else {
                    start_call(
                        active,
                        call_id,
                        call.execution,
                        call.mode == ToolMode::Synchronous,
                    )
                    .await
                };
                let should_begin = matches!(&started, Ok(true));
                if begin.send(should_begin).is_err() && should_begin {
                    return Err(ConversationFailure::new("工具句柄无法接收已确认的开始记录"));
                }
                if started? {
                    publish(progress, cancel).await?;
                }
            }
            Some(ToolEvent::Finished { call_id, result }) => {
                let call = self
                    .work
                    .calls
                    .remove(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具终态缺少调用身份"))?;
                let external_unconfirmed = result.status == ToolStatus::ExternalUnconfirmed;
                let (session, paused) = active
                    .call(|turn| Ok((turn.session_id(), turn.background_paused())))
                    .await?;
                save_call(
                    active,
                    dispatcher,
                    session,
                    &call_id,
                    call.execution,
                    result,
                    progress,
                    cancel,
                )
                .await?;
                if external_unconfirmed {
                    dispatcher.external_result_saved();
                }
                if paused {
                    Self::reap_stopped_processes(active, dispatcher).await?;
                }
            }
            None => {
                return Err(ConversationFailure::new(
                    "工具执行事件已关闭，仍有调用未回收",
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn accept_steer(
        &self,
        active: &TurnAccess,
        input: SteerInput,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        control::accept(active, input, progress).await?;
        self.stop_admission_for_steer(active).await?;
        publish(progress, cancel).await
    }

    pub(super) async fn stop_admission_for_steer(
        &self,
        active: &TurnAccess,
    ) -> Result<(), ConversationFailure> {
        if !self.work.calls.is_empty() && active.call(|turn| Ok(turn.has_pending_steer())).await? {
            for call in self.work.calls.values() {
                if call.turn == self.turn && call.mode == ToolMode::Synchronous {
                    call.admission.cancel();
                }
            }
        }
        Ok(())
    }

    pub(super) async fn wait_for_sync(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
        steers: &mut mpsc::Receiver<SteerInput>,
    ) -> Result<(), ConversationFailure> {
        loop {
            if !self.waits.is_empty() {
                if let Err(failure) = self.boundary(active, dispatcher, progress, cancel).await {
                    self.fail_waits(active, dispatcher).await?;
                    return Err(failure);
                }
                self.advance_waits(active, dispatcher, progress, cancel)
                    .await?;
            }
            if self.waits.is_empty()
                && !self
                    .work
                    .calls
                    .values()
                    .any(|call| call.turn == self.turn && call.mode == ToolMode::Synchronous)
            {
                break;
            }
            let deadline = self.waits.iter().map(|wait| wait.deadline).min();
            tokio::select! { biased;
                _ = cancel.cancelled(), if deadline.is_some() => {},
                command = self.controls.recv(), if !self.controls.is_closed() => {
                    if let Some(command) = command {
                        self.command(active, dispatcher, command, progress, cancel).await?;
                    }
                }
                input = steers.recv(), if !steers.is_closed() => {
                    if let Some(input) = input {
                        self.accept_steer(active, input, progress, cancel).await?;
                    }
                }
                event = self.work.incoming.recv() => {
                    if let Err(failure) = self.apply(active, dispatcher, event, progress, cancel).await {
                        self.fail_waits(active, dispatcher).await?;
                        return Err(failure);
                    }
                }
                () = wait_deadline(deadline) => {},
            }
        }
        self.boundary(active, dispatcher, progress, cancel).await
    }

    async fn fail_waits(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
    ) -> Result<(), ConversationFailure> {
        for wait in std::mem::take(&mut self.waits) {
            let result = ToolResult::error(
                ToolStatus::Failed,
                "wait_unconfirmed",
                "目标执行或结果提交未确认，等待已终止",
            );
            retain_call(active, dispatcher, wait.call_id, result).await?;
        }
        Ok(())
    }

    async fn advance_waits(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        let interrupted = if cancel.is_cancelled() {
            Some(WaitInterruption::Stopped)
        } else if active.call(|turn| Ok(turn.has_pending_steer())).await? {
            Some(WaitInterruption::Steered)
        } else {
            None
        };
        let session = active.call(|turn| Ok(turn.session_id())).await?;
        for wait in std::mem::take(&mut self.waits) {
            if let Some(result) = wait.evaluate(active, interrupted).await? {
                save_call(
                    active,
                    dispatcher,
                    session,
                    &wait.call_id,
                    None,
                    result,
                    progress,
                    cancel,
                )
                .await?;
            } else {
                self.waits.push(wait);
            }
        }
        Ok(())
    }

    pub(super) async fn boundary(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        self.stop_admission_for_steer(active).await?;
        while let Ok(command) = self.controls.try_recv() {
            self.command(active, dispatcher, command, progress, cancel)
                .await?;
        }
        while let Ok(event) = self.work.incoming.try_recv() {
            self.apply(active, dispatcher, Some(event), progress, cancel)
                .await?;
        }
        append_saved_results(active).await?;
        close_responses(active).await
    }

    pub(super) async fn close_commands(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ConversationFailure> {
        self.controls.close();
        while let Ok(command) = self.controls.try_recv() {
            self.command(active, dispatcher, command, progress, cancel)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn reap(
        &mut self,
        active: &TurnAccess,
        dispatcher: &ToolDispatcher,
        progress: &mpsc::Sender<RunnerEvent>,
        cancel: &CancellationToken,
        reason: &'static str,
    ) -> Result<(), ConversationFailure> {
        let mut reap_processes = true;
        while self.has_own_calls() {
            // A yielded process can hold the permit needed by an already registered call.
            if reap_processes {
                let report = dispatcher
                    .reap_processes(reason, agentelier_tools::ProcessScope::Turn(self.turn))
                    .await?;
                active
                    .call(move |mut turn| turn.process_cleanup(report))
                    .await?;
            }
            let event = tokio::select! {
                event = self.work.incoming.recv() => event,
                command = self.controls.recv(), if !self.controls.is_closed() => {
                    if let Some(command) = command {
                        self.command(active, dispatcher, command, progress, cancel).await?;
                    }
                    continue;
                }
            };
            reap_processes = matches!(event, Some(ToolEvent::Finished { .. }));
            self.apply(active, dispatcher, event, progress, cancel)
                .await?;
            append_saved_results(active).await?;
        }
        self.boundary(active, dispatcher, progress, cancel).await
    }
}

async fn wait_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn start_call(
    active: &TurnAccess,
    call_id: String,
    execution: Option<agentelier_conversation::TaskExecution>,
    stop_on_steer: bool,
) -> Result<bool, ConversationFailure> {
    let source = call_id.clone();
    let event = active
        .call(move |mut turn| {
            if (stop_on_steer && turn.has_pending_steer())
                || turn.task_cancellation_requested(&source)
            {
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
    let (result, published_scene) = published_scene_result(result, cancel);
    let (result, published_audio) = persist_published_audio(active, session, call_id, result).await;
    let (result, generated_image) =
        persist_generated_image(active, session, call_id, result).await?;
    let (result, storage_failure) = if generated_image.is_some() {
        (result, None)
    } else {
        persist_image(active, session, result).await
    };
    let outcome = retain_call(active, dispatcher, call_id.to_owned(), result).await?;
    if let Some(failure) = storage_failure {
        return Err(ConversationFailure::new(format!(
            "图片已读取，附件保存失败，停止后续执行：{failure}"
        )));
    }
    if let Some((status, output)) = outcome {
        let id = call_id.to_owned();
        let image_record = generated_image.clone();
        let audio_record = published_audio.clone();
        let scene_record = published_scene.clone();
        let event = active
            .call(move |turn| {
                Ok(RecordEvent::ToolReturned {
                    state: turn.stored_state(),
                    call_id: id,
                    status,
                    output: agentelier_llm_model::ArchivedPayload::capture(&output),
                    generated_image: image_record,
                    published_audio: audio_record,
                    published_scene: scene_record,
                    execution,
                })
            })
            .await?;
        let sequence = active.record(event).await?;
        let id = call_id.to_owned();
        if active
            .call(move |mut turn| {
                turn.confirm_result_saved(
                    &id,
                    execution,
                    status,
                    sequence,
                    generated_image,
                    published_audio,
                    published_scene,
                )
            })
            .await?
        {
            active.publish_tasks().await?;
        }
    }
    publish(progress, cancel).await
}

fn published_scene_result(
    mut result: ToolResult,
    cancel: &CancellationToken,
) -> (ToolResult, Option<agentelier_conversation::PublishedScene>) {
    let ToolPayload::PublishScene(definition) = result.output else {
        return (result, None);
    };
    if cancel.is_cancelled() {
        return (
            ToolResult::error(
                ToolStatus::Cancelled,
                "cancelled",
                "任务已停止，Scene 未发布",
            ),
            None,
        );
    }
    let id = agentelier_conversation::SessionId::new_v4();
    result.output = serde_json::json!({"ok":true,"scene":{"id":id,"template":definition.template}})
        .to_string()
        .into();
    (
        result,
        Some(agentelier_conversation::PublishedScene { id, definition }),
    )
}

async fn persist_published_audio(
    active: &TurnAccess,
    session: agentelier_conversation::SessionId,
    call_id: &str,
    mut result: ToolResult,
) -> (ToolResult, Option<agentelier_conversation::AudioAttachment>) {
    let ToolPayload::PublishAudio(path) = result.output else {
        return (result, None);
    };
    match active
        .persist_published_audio(session, call_id.to_owned(), path)
        .await
    {
        Ok(audio) => {
            result.output = serde_json::json!({
                "ok": true,
                "audio": {
                    "id": audio.id, "name": audio.name, "mime": audio.format.mime(),
                    "sha256": audio.digest, "encoded_bytes": audio.encoded_bytes,
                    "duration_ms": audio.duration_ms
                }
            })
            .to_string()
            .into();
            (result, Some(audio))
        }
        Err(failure) => (
            ToolResult::error(ToolStatus::Failed, "audio_publish", &failure.to_string()),
            None,
        ),
    }
}

async fn persist_generated_image(
    active: &TurnAccess,
    session: agentelier_conversation::SessionId,
    call_id: &str,
    mut result: ToolResult,
) -> Result<(ToolResult, Option<GeneratedImage>), ConversationFailure> {
    let ToolPayload::GeneratedImage { prompt, image } = result.output else {
        return Ok((result, None));
    };
    let id = call_id.to_owned();
    let turn = active
        .call(move |turn| turn.generated_image_turn(&id))
        .await?;
    let descriptor = image.descriptor().clone();
    let saved = match active
        .persist_generated_image(session, call_id.to_owned(), image)
        .await
    {
        Ok(saved) => saved,
        Err(failure) => {
            return Ok((
                ToolResult::error(
                    ToolStatus::Failed,
                    "image_storage",
                    &format!("已生成图片，但无法确认存档原图：{failure}"),
                ),
                None,
            ));
        }
    };
    let mut metadata = serde_json::json!({
        "ok": true,
        "model": GENERATED_IMAGE_MODEL,
        "original": descriptor,
        "path": saved.path.to_string_lossy(),
        "visual_input": true
    });
    result.output = match saved.feedback {
        GeneratedImageFeedback::Ready(image) => {
            let summary = metadata.to_string();
            let payload = ToolPayload::Image {
                summary: summary.clone(),
                image: image.clone(),
            };
            if active
                .call(move |turn| Ok(turn.can_retain_image(&payload)))
                .await?
            {
                match active.persist_image(session, summary.clone(), image).await {
                    Ok(image) => ToolPayload::Image { summary, image },
                    Err(failure) => {
                        unavailable_feedback(&mut metadata, &format!("保存模型图片失败：{failure}"))
                    }
                }
            } else {
                unavailable_feedback(&mut metadata, "会话图片输入超过 16 MiB")
            }
        }
        GeneratedImageFeedback::Unsupported => {
            unavailable_feedback(&mut metadata, "当前模型不支持图片输入")
        }
        GeneratedImageFeedback::Unavailable(failure) => {
            unavailable_feedback(&mut metadata, &failure)
        }
    };
    Ok((
        result,
        Some(GeneratedImage {
            turn,
            prompt,
            model: GENERATED_IMAGE_MODEL.into(),
            image: descriptor,
        }),
    ))
}

fn unavailable_feedback(metadata: &mut serde_json::Value, message: &str) -> ToolPayload {
    metadata["visual_input"] = serde_json::Value::Bool(false);
    metadata["visual_input_error"] = message.into();
    metadata.to_string().into()
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
        if active
            .call(move |mut turn| {
                turn.confirm_result_append(call_id, saved_sequence, append, sequence)
            })
            .await?
        {
            active.publish_tasks().await?;
        }
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

async fn close_responses(active: &TurnAccess) -> Result<(), ConversationFailure> {
    while let Some((response_sequence, event)) = active
        .call(|mut turn| {
            let Some(response_sequence) = turn.next_closed_response() else {
                return Ok(None);
            };
            turn.begin_history_commit();
            Ok(Some((
                response_sequence,
                RecordEvent::ResponseClosed {
                    state: turn.stored_state(),
                    response_sequence,
                },
            )))
        })
        .await?
    {
        let sequence = active.record(event).await?;
        active
            .call(move |mut turn| turn.confirm_response_closed(response_sequence, sequence))
            .await?;
    }
    Ok(())
}
