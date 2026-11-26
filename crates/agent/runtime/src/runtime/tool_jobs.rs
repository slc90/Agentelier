use agentelier_conversation::ProcessLedger;
use agentelier_llm_model::FunctionCall;
use agentelier_skills::UserSkills;
use agentelier_tools::{
    ExecutionContext, ProcessSessions, ScriptSession, ToolAccess, ToolAdmission, ToolCatalog,
    ToolMode, ToolResult, ToolStatus, ecs_access::EcsAccess,
};
use agentelier_turn_runner::{
    ToolAuthorization, ToolBatch, ToolCommand, ToolDispatcher, ToolEvent, ToolJobRequest, ToolWork,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) type RetainedResults = BTreeMap<String, ToolResult>;

#[derive(Clone)]
pub(super) struct SessionTools {
    pub(super) model_connection:
        Arc<tokio::sync::Mutex<agentelier_llm_model::GenerationConnection>>,
    pub(super) admission: ToolAdmission,
    pub(super) processes: Arc<ProcessSessions>,
    pub(super) process_views: watch::Receiver<ProcessLedger>,
    pub(super) dispatcher: ToolDispatcher,
    pub(super) work: Arc<tokio::sync::Mutex<ToolWork>>,
    worker: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<RetainedResults>>>>,
}

impl SessionTools {
    pub(super) fn for_session(
        active: &mut Option<(agentelier_conversation::SessionId, Self)>,
        view: &agentelier_conversation::SessionView,
        conversation: &agentelier_conversation::Conversation,
        store: agentelier_session_store::SessionStoreClient,
        ecs_access: &EcsAccess,
        skills: &UserSkills,
    ) -> Self {
        let session = view.id;
        if let Some((id, tools)) = active
            && *id == session
        {
            tools.admission.pause(view.task_policy.background_paused);
            tools
                .admission
                .external_unconfirmed(!view.task_policy.external_unconfirmed.is_empty());
            return tools.clone();
        }
        let admission = ToolAdmission::default();
        admission.pause(view.task_policy.background_paused);
        admission.external_unconfirmed(!view.task_policy.external_unconfirmed.is_empty());
        let (update, process_views) = watch::channel(conversation.restored_processes());
        let journal: agentelier_tools::SaveProcessFact = Arc::new(move |fact| {
            let store = store.clone();
            let update = update.clone();
            Box::pin(async move {
                let commit = store
                    .append(agentelier_conversation::RecordEvent::Process { fact: fact.clone() })
                    .await?;
                if commit.session != fact.origin.session {
                    return Err(std::io::Error::other("进程提交属于另一会话"));
                }
                let mut result = Ok(());
                update.send_if_modified(|ledger| {
                    result = ledger
                        .apply(&fact, commit.sequence)
                        .map_err(std::io::Error::other);
                    result.is_ok()
                });
                result
            })
        });
        let processes = Arc::new(ProcessSessions::new(admission.clone(), journal));
        let (dispatcher, commands, events, incoming) =
            ToolDispatcher::channel(admission.clone(), process_views.clone());
        let work = Arc::new(tokio::sync::Mutex::new(ToolWork::new(incoming)));
        let worker = tokio::spawn(run(
            commands,
            events,
            processes.clone(),
            process_views.clone(),
            admission.clone(),
            ScriptSession::new(ecs_access.clone(), skills.clone()),
        ));
        let tools = Self {
            model_connection: Arc::default(),
            processes,
            admission,
            process_views,
            dispatcher,
            work,
            worker: Arc::new(tokio::sync::Mutex::new(Some(worker))),
        };
        *active = Some((session, tools.clone()));
        tools
    }

    pub(super) fn busy(&self) -> bool {
        self.work.try_lock().map_or(true, |work| work.has_calls())
    }

    pub(super) async fn join(
        &self,
        reason: &'static str,
    ) -> Result<agentelier_tools::CleanupReport, agentelier_conversation::ConversationFailure> {
        let report = self.dispatcher.cleanup(reason).await;
        let handle = self.worker.lock().await.take().ok_or_else(|| {
            agentelier_conversation::ConversationFailure::new("会话工具执行者已回收")
        })?;
        let retained = handle.await.map_err(|failure| {
            agentelier_conversation::ConversationFailure::new(format!(
                "会话工具执行者异常结束：{failure}"
            ))
        })?;
        if !retained.is_empty() {
            return Err(agentelier_conversation::ConversationFailure::new(
                "仍有执行结果未接纳，须从会话记录恢复",
            ));
        }
        report
    }
}

struct JobIdentity {
    call_id: String,
    started: Arc<AtomicBool>,
    following: Option<OrderSignal>,
    stop: CancellationToken,
}

struct Execution {
    calls: JoinSet<ToolResult>,
    events: mpsc::UnboundedSender<ToolEvent>,
    identities: HashMap<Id, JobIdentity>,
    processes: Arc<ProcessSessions>,
    process_views: watch::Receiver<ProcessLedger>,
    scripts: ScriptSession,
    admission: ToolAdmission,
    ordinary: Option<Arc<JobOrder>>,
    controls: Option<Arc<JobOrder>>,
}

impl Execution {
    fn enqueue(&mut self, batch: ToolBatch) {
        let serial = batch.catalog.contract().is_flat();
        let controls_finished = CancellationToken::new();
        let control_guard = Arc::new(controls_finished.clone().drop_guard());
        for ToolJobRequest {
            call,
            admission: admission_stop,
            stop,
            execution,
        } in batch.calls
        {
            let mode = call.mode();
            let policy = match (batch.catalog.access(&call.name), mode) {
                (ToolAccess::SharedRead, ToolMode::Synchronous) => ToolAccess::Exclusive,
                (policy, _) => policy,
            };
            let process_control = policy == ToolAccess::ProcessControl;
            let order = if process_control {
                &mut self.controls
            } else {
                &mut self.ordinary
            };
            while order
                .as_ref()
                .is_some_and(|entry| entry.released.is_cancelled())
            {
                *order = order.as_ref().and_then(|entry| entry.previous.clone());
            }
            let next = Arc::new(JobOrder {
                previous: order.clone(),
                released: CancellationToken::new(),
            });
            let previous = order.replace(next.clone());
            let following = OrderSignal(next.released.clone());
            let (admitted, finished) = if serial && !process_control {
                (None, Some(following))
            } else {
                (Some(following), None)
            };
            let identity = JobIdentity {
                call_id: call.call_id.clone(),
                started: Arc::default(),
                following: finished,
                stop: stop.clone(),
            };
            let job = Job {
                call,
                policy,
                context: batch.context.clone(),
                catalog: batch.catalog.clone(),
                started: identity.started.clone(),
                previous,
                admitted,
                execution,
                authorization: batch.authorization.clone(),
                process_block: (!process_control && mode == ToolMode::Synchronous).then(|| {
                    ProcessBlock {
                        views: self.process_views.clone(),
                        controls_finished: controls_finished.clone(),
                    }
                }),
            };
            let admission = self.admission.clone();
            let processes = self.processes.clone();
            let scripts = self.scripts.clone();
            let events = self.events.clone();
            let control = process_control.then(|| control_guard.clone());
            let handle = self.calls.spawn(async move {
                let result = job
                    .run(admission, processes, scripts, admission_stop, stop, events)
                    .await;
                drop(control);
                result
            });
            self.identities.insert(handle.id(), identity);
        }
    }

    fn stop_all(&self) {
        for job in self.identities.values() {
            job.stop.cancel();
        }
    }

    fn complete(
        &mut self,
        result: Result<(Id, ToolResult), tokio::task::JoinError>,
        retained: &mut RetainedResults,
    ) {
        let id = result
            .as_ref()
            .map_or_else(tokio::task::JoinError::id, |(id, _)| *id);
        let Some(job) = self.identities.remove(&id) else {
            self.admission.block();
            tracing::error!(?id, "工具句柄缺少调用身份");
            return;
        };
        let result = match result {
            Ok((_, result)) => result,
            Err(failure) => {
                let started = job.started.load(Ordering::Acquire);
                tracing::error!(call_id = job.call_id, %failure, started, "工具执行句柄异常结束");
                let mut result = ToolResult::error(
                    if started {
                        ToolStatus::Unconfirmed
                    } else {
                        ToolStatus::NotExecuted
                    },
                    "executor_join",
                    "工具执行句柄异常结束，实际结果未确认",
                );
                result.cleanup_failed = true;
                result
            }
        };
        if result.cleanup_failed || result.status == ToolStatus::Unconfirmed {
            self.admission.block();
        }
        retained.insert(job.call_id.clone(), result.clone());
        drop(self.events.send(ToolEvent::Finished {
            call_id: job.call_id,
            result,
        }));
        drop(job.following);
    }
}

struct JobOrder {
    previous: Option<Arc<Self>>,
    released: CancellationToken,
}

struct OrderSignal(CancellationToken);

impl Drop for OrderSignal {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct Job {
    call: FunctionCall,
    policy: ToolAccess,
    context: Arc<ExecutionContext>,
    catalog: Arc<ToolCatalog>,
    started: Arc<AtomicBool>,
    previous: Option<Arc<JobOrder>>,
    admitted: Option<OrderSignal>,
    execution: Option<agentelier_conversation::TaskExecution>,
    authorization: ToolAuthorization,
    process_block: Option<ProcessBlock>,
}

struct ProcessBlock {
    views: watch::Receiver<ProcessLedger>,
    controls_finished: CancellationToken,
}

impl ProcessBlock {
    async fn wait(block: Option<Self>) -> ToolResult {
        let Some(Self {
            mut views,
            controls_finished,
        }) = block
        else {
            return std::future::pending().await;
        };
        // A control in the same batch can release the permit without another model request.
        controls_finished.cancelled().await;
        loop {
            let process_id = views
                .borrow_and_update()
                .views()
                .find(|process| process.holds_exclusive_permit())
                .map(|process| process.session_id.clone());
            if let Some(id) = process_id {
                return ToolResult::not_executed(
                    "process_holds_exclusive_permit",
                    &format!(
                        "进程 {id} 仍持有独占许可，此调用未执行。可用 write_stdin 和该 session_id 检查或终止进程，许可释放后重新调用。"
                    ),
                );
            }
            if views.changed().await.is_err() {
                return ToolResult::not_executed(
                    "process_state_unavailable",
                    "进程状态通道已关闭，此调用未执行",
                );
            }
        }
    }
}

struct RunningJob<'a> {
    admission: &'a ToolAdmission,
    completed: bool,
}

impl Drop for RunningJob<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.admission.block();
        }
    }
}

impl Job {
    async fn run(
        self,
        admission: ToolAdmission,
        processes: Arc<ProcessSessions>,
        scripts: ScriptSession,
        admission_stop: CancellationToken,
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<ToolEvent>,
    ) -> ToolResult {
        let skipped = || {
            if admission.is_blocked() {
                ToolResult::not_executed("cleanup_failed", "工具资源清理未确认，此调用未执行")
            } else {
                ToolResult::not_executed(
                    "admission_cancelled",
                    "停止或补充输入已接纳，此调用未执行",
                )
            }
        };
        let process_block = ProcessBlock::wait(self.process_block);
        tokio::pin!(process_block);
        let mut previous = self.previous.as_deref();
        while let Some(order) = previous {
            tokio::select! { biased;
                _ = cancel.cancelled() => return skipped(),
                _ = admission_stop.cancelled() => return skipped(),
                _ = order.released.cancelled() => {}
                result = &mut process_block => return result,
            }
            previous = order.previous.as_deref();
        }
        if let Some(code) = admission.rejection(self.policy) {
            return ToolResult::not_executed(code, "会话派发暂停，本次调用未执行");
        }
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            permit = admission.acquire(self.policy, &admission_stop) => permit,
            result = &mut process_block => return result,
        };
        let Some(permit) = permit else {
            return skipped();
        };
        if let Some(code) = admission.rejection(self.policy) {
            return ToolResult::not_executed(code, "会话派发暂停，本次调用未执行");
        }
        let (begin, confirmation) = oneshot::channel();
        if events
            .send(ToolEvent::Ready {
                call_id: self.call.call_id.clone(),
                begin,
            })
            .is_err()
        {
            return skipped();
        }
        drop(self.admitted);
        let confirmed = confirmation.await.unwrap_or(false);
        if !confirmed
            || cancel.is_cancelled()
            || admission_stop.is_cancelled()
            || (admission.is_blocked() && self.policy != ToolAccess::ProcessControl)
        {
            return skipped();
        }
        if let Some(code) = admission.rejection(self.policy) {
            return ToolResult::not_executed(code, "会话派发暂停，本次调用未执行");
        }
        let authorized = tokio::select! { biased;
            _ = cancel.cancelled() => return skipped(),
            _ = admission_stop.cancelled() => return skipped(),
            result = self.authorization.validate() => result,
        };
        if let Err(failure) = authorized {
            return ToolResult::not_executed("authorization_changed", &failure.to_string());
        }
        if let Some(code) = admission.rejection(self.policy) {
            return ToolResult::not_executed(code, "会话派发暂停，本次调用未执行");
        }
        self.started.store(true, Ordering::Release);
        let mut running = RunningJob {
            admission: &admission,
            completed: false,
        };
        let source = self.context.user_turn();
        tracing::debug!(
            session = %source.session, turn = source.turn, call_id = self.call.call_id,
            request = self.context.request(),
            tool = self.call.name, access = ?self.policy, mode = ?self.call.mode(),
            "工具执行开始"
        );
        let started = std::time::Instant::now();
        let origin = agentelier_tools::ProcessOrigin {
            session: source.session,
            turn: source.turn,
            request: self.context.request(),
            call_id: self.call.call_id.clone(),
            task_id: self.execution.map(|execution| execution.task.0),
            instance: self
                .execution
                .map_or_else(agentelier_conversation::SessionId::new_v4, |execution| {
                    execution.instance
                }),
        };
        let outcome = if self.catalog.is_codex_image(&self.call.name) {
            let result = self
                .authorization
                .generate_image(&self.call.arguments, &cancel)
                .await;
            drop(permit);
            result
        } else {
            agentelier_tools::execute(
                &self.catalog,
                &self.context,
                &processes,
                &scripts,
                &self.call.name,
                &self.call.arguments,
                &cancel,
                permit,
                origin,
            )
            .await
        };
        if outcome.cleanup_failed || outcome.status == ToolStatus::Unconfirmed {
            admission.block();
        }
        running.completed = true;
        tracing::debug!(
            session = %source.session, turn = source.turn, call_id = self.call.call_id,
            request = self.context.request(),
            tool = self.call.name, elapsed_ms = started.elapsed().as_millis(),
            status = ?outcome.status,
            "工具执行已结束"
        );
        outcome
    }
}

async fn run(
    mut incoming: mpsc::Receiver<ToolCommand>,
    events: mpsc::UnboundedSender<ToolEvent>,
    processes: Arc<ProcessSessions>,
    process_views: watch::Receiver<ProcessLedger>,
    admission: ToolAdmission,
    scripts: ScriptSession,
) -> RetainedResults {
    let mut execution = Execution {
        calls: JoinSet::new(),
        events,
        identities: HashMap::new(),
        processes,
        process_views,
        scripts,
        admission,
        ordinary: None,
        controls: None,
    };
    let mut retained = RetainedResults::new();
    let mut cleanup = None;
    loop {
        if cleanup.is_some() && execution.calls.is_empty() {
            break;
        }
        tokio::select! {
            joined = execution.calls.join_next_with_id(), if !execution.calls.is_empty() => {
                if let Some(joined) = joined {
                    execution.complete(joined, &mut retained);
                }
            }
            command = incoming.recv(), if cleanup.is_none() => match command {
                Some(ToolCommand::Batch(batch)) => execution.enqueue(batch),
                Some(ToolCommand::Accepted(call_id)) => { retained.remove(&call_id); }
                Some(ToolCommand::ReapProcesses { reason, scope, reply }) => {
                    let report = execution.processes.cleanup(reason, scope).await;
                    if report.failed {
                        execution.admission.block();
                    }
                    drop(reply.send(report));
                }
                Some(ToolCommand::Cleanup { reason, reply }) => {
                    cleanup = Some((reason, Some(reply)));
                    execution.stop_all();
                }
                None => {
                    cleanup = Some(("runner_closed", None));
                    execution.stop_all();
                }
            },
        }
    }
    if let Some((reason, reply)) = cleanup {
        let uia_cleanup = execution.scripts.close_uia().await;
        let mut report = execution
            .processes
            .cleanup(reason, agentelier_tools::ProcessScope::All)
            .await;
        if let Err(error) = uia_cleanup {
            report.failed = true;
            execution.admission.block();
            tracing::error!(reason, code = error.code, %error, "UIA 会话资源回收未确认");
        }
        report.failed |= execution.admission.is_blocked();
        if let Some(reply) = reply {
            drop(reply.send(report));
        } else if report.failed {
            tracing::error!(reason, "聊天执行异常退出后的工具资源回收未确认");
        }
    }
    retained
}
