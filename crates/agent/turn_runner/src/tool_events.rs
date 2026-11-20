use crate::{
    access::{RunnerEvent, TurnAccess},
    control,
    dispatch::{ToolBatch, ToolCommand, ToolDispatcher, ToolEvent, ToolJobRequest},
    runner::publish,
    task_control::{self, PendingWait, TaskControlAction, WaitInterruption},
};
use agentelier_conversation::{ConversationFailure, RecordEvent, SteerInput};
use agentelier_tools::{ExecutionContext, ToolCatalog, ToolMode, ToolResult, ToolStatus};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) struct TurnTools {
    calls: HashMap<String, PendingCall>,
    waits: Vec<PendingWait>,
    pub(super) incoming: mpsc::UnboundedReceiver<ToolEvent>,
}

struct PendingCall {
    mode: ToolMode,
    execution: Option<agentelier_conversation::TaskExecution>,
    admission: CancellationToken,
}

impl TurnTools {
    pub(super) fn new(incoming: mpsc::UnboundedReceiver<ToolEvent>) -> Self {
        Self {
            calls: HashMap::new(),
            waits: Vec::new(),
            incoming,
        }
    }

    pub(super) fn has_calls(&self) -> bool {
        !self.calls.is_empty()
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
                    .map(|id| turn.task_execution(id))
                    .collect::<Vec<_>>())
            })
            .await?;
        let mut jobs = Vec::new();
        for (call, execution) in eligible.into_iter().zip(executions) {
            let mode = call.mode();
            let admission = cancel.child_token();
            if steered && mode == ToolMode::Synchronous {
                admission.cancel();
            }
            self.calls.insert(
                call.call_id.clone(),
                PendingCall {
                    mode,
                    execution,
                    admission: admission.clone(),
                },
            );
            jobs.push(ToolJobRequest { call, admission });
        }
        if !jobs.is_empty() {
            dispatcher
                .send(ToolCommand::Batch(ToolBatch {
                    calls: jobs,
                    catalog,
                    context,
                }))
                .await?;
        }
        for (call, control) in controls {
            let id = call.call_id;
            let started = !cancel.is_cancelled()
                && !steered
                && start_call(active, id.clone(), None, true).await?;
            let result = if started {
                match task_control::control(active, control, &call.arguments).await? {
                    TaskControlAction::Return(result) => result,
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
                    .calls
                    .get(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具准入缺少调用身份"))?;
                let started = if call.admission.is_cancelled() || dispatcher.is_blocked() {
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
                    .calls
                    .remove(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具终态缺少调用身份"))?;
                let session = active.call(|turn| Ok(turn.session_id())).await?;
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
        if !self.calls.is_empty() && active.call(|turn| Ok(turn.has_pending_steer())).await? {
            for call in self.calls.values() {
                if call.mode == ToolMode::Synchronous {
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
                    .calls
                    .values()
                    .any(|call| call.mode == ToolMode::Synchronous)
            {
                break;
            }
            let deadline = self.waits.iter().map(|wait| wait.deadline).min();
            tokio::select! { biased;
                _ = cancel.cancelled(), if deadline.is_some() => {},
                input = steers.recv(), if !steers.is_closed() => {
                    if let Some(input) = input {
                        self.accept_steer(active, input, progress, cancel).await?;
                    }
                }
                event = self.incoming.recv() => {
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
        while let Ok(event) = self.incoming.try_recv() {
            self.apply(active, dispatcher, Some(event), progress, cancel)
                .await?;
        }
        append_saved_results(active).await?;
        close_responses(active).await
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
        while !self.calls.is_empty() {
            // A yielded process can hold the permit needed by an already registered call.
            if reap_processes {
                let report = dispatcher.reap_processes(reason).await?;
                active
                    .call(move |mut turn| turn.process_cleanup(report))
                    .await?;
            }
            let event = self.incoming.recv().await;
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
            if stop_on_steer && turn.has_pending_steer() {
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
