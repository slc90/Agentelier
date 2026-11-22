mod elements;
mod tree;

pub(crate) use elements::RootSummary;
pub(crate) use tree::{TreePage, TreeRequest};

use crate::ToolFailure;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::UIAutomation;
use uiautomation::errors::Error as UiaError;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::UI::Accessibility::{UIA_E_ELEMENTNOTAVAILABLE, UIA_E_PROXYASSEMBLYNOTLOADED};
use winsafe::{co, guard::CoUninitializeGuard};

const REQUEST_CAPACITY: usize = 16;

type Request = Box<dyn FnOnce(&mut Worker) + Send>;

#[derive(Default)]
pub(crate) struct Runtime {
    state: Mutex<State>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Running(WorkerHandle),
    Closed,
}

struct WorkerHandle {
    requests: SyncSender<Request>,
    thread: JoinHandle<Result<(), ToolFailure>>,
}

struct Worker {
    tree: tree::TreeState,
    elements: elements::Registry,
    automation: UIAutomation,
}

struct RequestControl {
    cancel: CancellationToken,
    deadline: Instant,
}

impl RequestControl {
    fn check(&self) -> Result<(), ToolFailure> {
        if self.cancel.is_cancelled() {
            Err(ToolFailure::new("cancelled", "UIA 请求已取消"))
        } else if Instant::now() >= self.deadline {
            Err(ToolFailure::new("timeout", "UIA 请求超过脚本执行期限"))
        } else {
            Ok(())
        }
    }
}

impl Runtime {
    pub(crate) fn status(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<bool, ToolFailure> {
        self.call(cancel, deadline, |worker, _| {
            worker
                .automation
                .get_control_view_condition()
                .map(|_| true)
                .map_err(provider_error)
        })
    }

    fn call<T: Send + 'static>(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
        action: impl FnOnce(&mut Worker, &RequestControl) -> Result<T, ToolFailure> + Send + 'static,
    ) -> Result<T, ToolFailure> {
        let control = RequestControl {
            cancel: cancel.clone(),
            deadline,
        };
        control.check()?;
        let (reply, result) = sync_channel(1);
        {
            let mut state = self.state.lock().map_err(|_| worker_failed())?;
            if matches!(*state, State::Idle) {
                *state = State::Running(WorkerHandle::start()?);
            }
            let State::Running(worker) = &*state else {
                return Err(ToolFailure::new("uia_closed", "UIA 会话已关闭"));
            };
            control.check()?;
            let request: Request = Box::new(move |worker| {
                let response = control.check().and_then(|()| action(worker, &control));
                drop(reply.send(control.check().and(response)));
            });
            worker
                .requests
                .try_send(request)
                .map_err(|error| match error {
                    TrySendError::Full(_) => ToolFailure::new("uia_queue_full", "UIA 请求队列已满"),
                    TrySendError::Disconnected(_) => worker_failed(),
                })?;
        }

        // Keep desktop admission until the worker finishes any active provider call.
        result.recv().map_err(|_| worker_failed())?
    }

    pub(crate) async fn close(&self) -> Result<(), ToolFailure> {
        let state = {
            let mut state = self.state.lock().map_err(|_| worker_failed())?;
            std::mem::replace(&mut *state, State::Closed)
        };
        if let State::Running(worker) = state {
            tokio::task::spawn_blocking(move || worker.close())
                .await
                .map_err(|_| worker_failed())??;
        }
        Ok(())
    }
}

impl WorkerHandle {
    fn start() -> Result<Self, ToolFailure> {
        let (requests, incoming) = sync_channel(REQUEST_CAPACITY);
        let (ready, startup) = sync_channel(1);
        let thread = thread::Builder::new()
            .name("agentelier-uia".into())
            .spawn(move || run_worker(incoming, ready))
            .map_err(|error| ToolFailure::new("uia_worker_start_failed", error.to_string()))?;
        let worker = Self { requests, thread };
        match startup.recv() {
            Ok(Ok(())) => Ok(worker),
            Err(_) => {
                worker.close()?;
                Err(worker_failed())
            }
            Ok(Err(error)) => {
                worker.close()?;
                Err(error)
            }
        }
    }

    fn close(self) -> Result<(), ToolFailure> {
        drop(self.requests);
        self.thread.join().map_err(|_| worker_failed())?
    }
}

fn run_worker(
    incoming: Receiver<Request>,
    ready: SyncSender<Result<(), ToolFailure>>,
) -> Result<(), ToolFailure> {
    let (com, mut worker) = match connect() {
        Ok(connection) => connection,
        Err(error) => {
            drop(ready.send(Err(error)));
            return Ok(());
        }
    };
    let thread = thread::current().id();
    tracing::debug!(?thread, "UIA MTA 工作线程已就绪");
    if ready.send(Ok(())).is_ok() {
        for request in incoming {
            request(&mut worker);
        }
    }
    let cleanup = worker.tree.close(&worker.automation);
    drop(worker);
    drop(com);
    tracing::debug!(?thread, "UIA MTA 工作线程已释放 COM 资源");
    cleanup
}

fn connect() -> Result<(CoUninitializeGuard, Worker), ToolFailure> {
    let com = winsafe::CoInitializeEx(co::COINIT::MULTITHREADED)
        .map_err(|error| ToolFailure::new("uia_com_initialization_failed", error.to_string()))?;
    if com.hr() == co::HRESULT::RPC_E_CHANGED_MODE {
        // WinSafe also returns a guard on this failure. There is no COM init to balance.
        std::mem::forget(com);
        return Err(ToolFailure::new(
            "uia_com_initialization_failed",
            "UIA 工作线程已使用不兼容的 COM apartment",
        ));
    }
    let automation = UIAutomation::new_direct()
        .map_err(|error| ToolFailure::new("uia_initialization_failed", error.to_string()))?;
    Ok((
        com,
        Worker {
            tree: tree::TreeState::default(),
            elements: elements::Registry::default(),
            automation,
        },
    ))
}

fn worker_failed() -> ToolFailure {
    ToolFailure::new("uia_worker_failed", "UIA 工作线程异常退出")
}

fn provider_error(error: UiaError) -> ToolFailure {
    let code = match error.code().cast_unsigned() {
        UIA_E_ELEMENTNOTAVAILABLE => "uia_element_unavailable",
        UIA_E_PROXYASSEMBLYNOTLOADED => "uia_provider_unavailable",
        _ if error.result() == Some(E_ACCESSDENIED) => "uia_provider_unavailable",
        _ => "uia_provider_error",
    };
    ToolFailure::new(
        code,
        format!("UIA 错误 0x{:08x}：{error}", error.code().cast_unsigned()),
    )
}
