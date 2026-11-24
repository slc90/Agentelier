use crate::splash;
use agentelier_agent_host::AgentHost;
use bevy::app::OnAppExitSystems;
use bevy::prelude::*;
use bevy::window::{CursorSystems, ExitSystems};
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
use bevy_widgetry::file_dialog::WidgetryFileDialogHeadless;
use bevy_widgetry::job::{JobManager, JobManagerSystems, ShutdownHandle, ShutdownPhase};
use bevy_widgetry::tray::WidgetryTraySystems;
use std::time::Duration;
use tracing::{debug, error, warn};

const JOB_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const JOB_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Resource, Default)]
struct ExitState {
    requested: Option<AppExit>,
    agent_finished: bool,
    jobs: Option<ShutdownHandle>,
    failed_jobs: bool,
    timeout_reported: bool,
    complete: bool,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ExitState>()
        .add_systems(
            Last,
            retire_file_dialogs
                .after(CursorSystems::Update)
                .before(JobManagerSystems::EndFrame),
        )
        .add_systems(
            Last,
            splash::retire
                .after(ExitSystems)
                .before(coordinate)
                .before(JobManagerSystems::EndFrame),
        )
        .add_systems(
            Last,
            coordinate
                .after(JobManagerSystems::EndFrame)
                .after(ExitSystems)
                .before(WidgetryTraySystems::Sync)
                .before(OnAppExitSystems),
        );
}

fn retire_file_dialogs(
    mut exits: MessageReader<AppExit>,
    dialogs: Query<Entity, With<WidgetryFileDialogHeadless>>,
    mut commands: Commands,
) {
    if exits.read().count() == 0 {
        return;
    }
    // The FileDialog snapshot worker waits for the UI to release its snapshots.
    for dialog in &dialogs {
        commands.entity(dialog).try_despawn();
    }
}

fn coordinate(
    mut exits: ResMut<Messages<AppExit>>,
    mut state: ResMut<ExitState>,
    backend: Res<AgentHost>,
    manager: Res<JobManager>,
    proxy: Res<EventLoopProxyWrapper>,
) {
    if state.complete {
        return;
    }
    let first_request = state.requested.is_none();
    // Winit observes AppExit after Last. Hold requests until their owners have joined.
    for request in exits.drain() {
        if state.requested.is_none() || request.is_error() {
            state.requested = Some(request);
        }
    }
    if state.requested.is_none() {
        return;
    }
    if first_request && let Err(failure) = backend.begin_shutdown() {
        fail(&mut state, failure);
    }
    if !state.agent_finished {
        match backend.is_finished() {
            Ok(false) => {
                wake(&proxy, &mut state);
                return;
            }
            Ok(true) => match backend.finish() {
                Ok(()) => debug!("共享执行后台已真实结束"),
                Err(failure) => fail(&mut state, failure),
            },
            Err(failure) => fail(&mut state, failure),
        }
        state.agent_finished = true;
    }
    if state.jobs.is_none() && !state.failed_jobs {
        match manager.begin_shutdown(JOB_SHUTDOWN_TIMEOUT) {
            Ok(shutdown) => state.jobs = Some(shutdown),
            Err(failure) => {
                state.failed_jobs = true;
                fail(&mut state, failure.into_bevy_error());
            }
        }
    }
    if let Some(shutdown) = &state.jobs {
        let status = shutdown.status();
        if status.phase != ShutdownPhase::Finished {
            if status.phase == ShutdownPhase::TimedOut && !state.timeout_reported {
                warn!(
                    remaining = status.remaining.len(),
                    "后台任务收尾超时，继续等待实际结束"
                );
                state.timeout_reported = true;
            }
            wake(&proxy, &mut state);
            return;
        }
        debug!("应用辅助后台任务已全部结束");
    }
    state.complete = true;
    if let Some(request) = state.requested.take() {
        exits.write(request);
    }
    debug!("后台收尾完成，允许主循环退出");
}

fn fail(state: &mut ExitState, failure: BevyError) {
    error!(error = %failure, "应用后台收尾失败");
    state.requested = Some(AppExit::error());
}

fn wake(proxy: &EventLoopProxyWrapper, state: &mut ExitState) {
    if let Err(failure) = proxy.send_event(WinitUserEvent::WakeUp) {
        fail(state, crate::diagnostics::operation(failure));
    }
}

pub(crate) fn finish_jobs(manager: &JobManager) -> Result {
    let shutdown = manager
        .begin_shutdown(JOB_SHUTDOWN_TIMEOUT)
        .map_err(|failure| failure.into_bevy_error())?;
    loop {
        if shutdown.status().phase == ShutdownPhase::Finished {
            return Ok(());
        }
        std::thread::sleep(JOB_SHUTDOWN_POLL_INTERVAL);
    }
}
