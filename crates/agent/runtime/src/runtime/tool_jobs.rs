use agentelier_llm_model::FunctionCall;
use agentelier_tools::{
    ExecutionContext, ProcessSessions, ToolAccess, ToolAdmission, ToolCatalog, ToolMode,
    ToolResult, ToolStatus,
};
use agentelier_turn_runner::{ToolBatch, ToolCommand, ToolEvent, ToolJobRequest};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) type RetainedResults = BTreeMap<String, ToolResult>;

#[derive(Clone)]
pub(super) struct SessionTools {
    pub(super) admission: ToolAdmission,
    processes: Arc<ProcessSessions>,
}

impl SessionTools {
    pub(super) fn for_session(
        active: &mut Option<(agentelier_conversation::SessionId, Self)>,
        session: agentelier_conversation::SessionId,
    ) -> Self {
        if let Some((id, tools)) = active
            && *id == session
        {
            return tools.clone();
        }
        let admission = ToolAdmission::default();
        let tools = Self {
            processes: Arc::new(ProcessSessions::new(admission.clone())),
            admission,
        };
        *active = Some((session, tools.clone()));
        tools
    }
}

struct JobIdentity {
    call_id: String,
    started: Arc<AtomicBool>,
    following: Option<oneshot::Sender<()>>,
}

struct Execution {
    calls: JoinSet<ToolResult>,
    events: mpsc::UnboundedSender<ToolEvent>,
    identities: HashMap<Id, JobIdentity>,
    processes: Arc<ProcessSessions>,
    admission: ToolAdmission,
    cancel: CancellationToken,
    ordinary: Option<oneshot::Receiver<()>>,
    controls: Option<oneshot::Receiver<()>>,
}

impl Execution {
    fn enqueue(&mut self, batch: ToolBatch) {
        let serial = batch.catalog.contract().is_flat();
        for ToolJobRequest {
            call,
            admission: admission_stop,
        } in batch.calls
        {
            let policy = batch.catalog.access(&call.name);
            let policy = match (policy, call.mode()) {
                (ToolAccess::SharedRead, ToolMode::Synchronous) => ToolAccess::Exclusive,
                _ => policy,
            };
            let order = if policy == ToolAccess::ProcessControl {
                &mut self.controls
            } else {
                &mut self.ordinary
            };
            let (following, wait) = oneshot::channel();
            let previous = order.replace(wait);
            let (admitted, finished) = if serial && policy != ToolAccess::ProcessControl {
                (None, Some(following))
            } else {
                (Some(following), None)
            };
            let identity = JobIdentity {
                call_id: call.call_id.clone(),
                started: Arc::default(),
                following: finished,
            };
            let job = Job {
                call,
                policy,
                context: batch.context.clone(),
                catalog: batch.catalog.clone(),
                started: identity.started.clone(),
                previous,
                admitted,
            };
            let cancel = self.cancel.clone();
            let admission = self.admission.clone();
            let processes = self.processes.clone();
            let events = self.events.clone();
            let handle =
                self.calls
                    .spawn(job.run(admission, processes, admission_stop, cancel, events));
            self.identities.insert(handle.id(), identity);
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

struct Job {
    call: FunctionCall,
    policy: ToolAccess,
    context: Arc<ExecutionContext>,
    catalog: Arc<ToolCatalog>,
    started: Arc<AtomicBool>,
    previous: Option<oneshot::Receiver<()>>,
    admitted: Option<oneshot::Sender<()>>,
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
        admission_stop: CancellationToken,
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<ToolEvent>,
    ) -> ToolResult {
        if let Some(previous) = self.previous {
            drop(previous.await);
        }
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
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            permit = admission.acquire(self.policy, &admission_stop) => permit,
        };
        let Some(permit) = permit else {
            return skipped();
        };
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
        if !confirmation.await.unwrap_or(false)
            || cancel.is_cancelled()
            || admission_stop.is_cancelled()
            || (admission.is_blocked() && self.policy != ToolAccess::ProcessControl)
        {
            return skipped();
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
        let outcome = agentelier_tools::execute(
            &self.catalog,
            &self.context,
            &processes,
            &self.call.name,
            &self.call.arguments,
            &cancel,
            permit,
        )
        .await;
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

pub(super) async fn run(
    mut incoming: mpsc::Receiver<ToolCommand>,
    events: mpsc::UnboundedSender<ToolEvent>,
    cancel: CancellationToken,
    tools: SessionTools,
) -> RetainedResults {
    let mut execution = Execution {
        calls: JoinSet::new(),
        events,
        identities: HashMap::new(),
        processes: tools.processes,
        admission: tools.admission,
        cancel: cancel.child_token(),
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
                Some(ToolCommand::ReapProcesses { reason, reply }) => {
                    let report = execution.processes.cleanup(reason).await;
                    if report.failed {
                        execution.admission.block();
                    }
                    drop(reply.send(report));
                }
                Some(ToolCommand::Cleanup { reason, reply }) => {
                    cleanup = Some((reason, Some(reply)));
                    execution.cancel.cancel();
                }
                None => {
                    cleanup = Some(("runner_closed", None));
                    execution.cancel.cancel();
                }
            },
        }
    }
    if let Some((reason, reply)) = cleanup {
        let mut report = execution.processes.cleanup(reason).await;
        report.failed |= execution.admission.is_blocked();
        if let Some(reply) = reply {
            drop(reply.send(report));
        } else if report.failed {
            tracing::error!(reason, "聊天执行异常退出后的工具资源回收未确认");
        }
    }
    retained
}
