mod elements;
mod events;
mod find;
mod grid;
mod interaction;
mod metadata;
mod native;
mod object_model;
mod patterns;
mod properties;
mod references;
mod scrolling;
mod snapshot;
mod text;
mod transform;
mod tree;
mod wait;
mod window;

pub(crate) use elements::RootSummary;
pub(crate) use events::EventRequest;
pub(crate) use find::FindRequest;
pub(crate) use grid::GridAction;
pub(crate) use interaction::InteractionAction;
pub(crate) use metadata::{LegacyAction, MetadataAction};
pub(crate) use patterns::{BasicAction, Expansion, SelectionChange};
pub(crate) use properties::CacheRequest;
pub(crate) use scrolling::{ItemLookup, ScrollAmounts};
pub(crate) use snapshot::{SnapshotRequest, SnapshotResult};
pub(crate) use text::{TextAction, TextRangeAction};
pub(crate) use transform::TransformAction;
pub(crate) use tree::{TreePage, TreeRequest};
pub(crate) use wait::WaitRequest;
pub(crate) use window::WindowAction;

use crate::ToolFailure;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::UIAutomation;
use uiautomation::errors::Error as UiaError;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::UI::Accessibility::{
    UIA_E_ELEMENTNOTAVAILABLE, UIA_E_PROXYASSEMBLYNOTLOADED, UIA_E_TIMEOUT,
};
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
    snapshots: snapshot::SnapshotState,
    events: events::EventState,
    elements: elements::Registry,
    virtual_items: scrolling::VirtualItems,
    synchronized_input: interaction::SynchronizedInput,
    text_ranges: text::TextRanges,
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
    let input_cleanup = worker.synchronized_input.close();
    worker.close_snapshots();
    let event_cleanup = worker.events.close(&worker.automation);
    let cursor_cleanup = worker.tree.close(&worker.automation);
    let fallback = events::worker_disconnected(&worker.automation);
    let cleanup = if matches!(fallback, Ok(true)) {
        input_cleanup
    } else {
        let errors = [
            input_cleanup,
            event_cleanup,
            cursor_cleanup,
            fallback.map(|_| ()),
        ]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ToolFailure::new(
                "uia_worker_cleanup_failed",
                errors.join("；"),
            ))
        }
    };
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
    events::worker_connected()?;
    Ok((
        com,
        Worker {
            tree: tree::TreeState::default(),
            snapshots: snapshot::SnapshotState::default(),
            events: events::EventState::default(),
            elements: elements::Registry::default(),
            virtual_items: scrolling::VirtualItems::default(),
            synchronized_input: interaction::SynchronizedInput::default(),
            text_ranges: text::TextRanges::default(),
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
        UIA_E_TIMEOUT => "uia_timeout",
        _ if error.result() == Some(E_ACCESSDENIED) => "uia_provider_unavailable",
        _ => "uia_provider_error",
    };
    ToolFailure::new(
        code,
        format!("UIA 错误 0x{:08x}：{error}", error.code().cast_unsigned()),
    )
}
