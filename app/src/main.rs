#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application template supports Windows x64 only");

mod config;
mod diagnostics;

use agentelier_login::{DataEnvironment, display_path};
use agentelier_login_ui::{DataDirectoryReady, LoginUiHost, LoginUiPlugin};
use bevy::log::LogPlugin;
use bevy::window::{MonitorSelection, PrimaryWindow, WindowPosition, WindowResolution};
use bevy::winit::WinitSettings;
use bevy::{prelude::*, render::RenderPlugin, tasks::block_on};
use bevy_app_assets::AppAssetPlugin;
use bevy_brp_runtime::BrpRuntimePlugin;
use bevy_widgetry::app_logging::{AppLogging, LoggingGuard, log_plugin};
use bevy_widgetry::job::{JobManager, ShutdownPhase};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::ui_scale::{WidgetryUiScale, WidgetryUiScalePlugin};
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowPlugin,
    prepare_native_window, transparent_render_creation, widgetry_window,
};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{debug, warn};

const JOB_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const JOB_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn main() -> std::process::ExitCode {
    let mut logging_ready = false;
    let mut guard: Option<LoggingGuard> = None;
    let data_environment = Arc::new(OnceLock::new());
    let result = run(&mut logging_ready, &mut guard, data_environment.clone());
    let mut exit = match result {
        Ok(exit) => exit,
        Err(error) => {
            diagnostics::startup(error, logging_ready);
            AppExit::error()
        }
    };
    if let Some(guard) = guard
        && let Err(failure) = guard.finish()
    {
        diagnostics::logging_shutdown(&failure);
        exit = AppExit::error();
    }
    drop(data_environment);
    match exit {
        AppExit::Success => std::process::ExitCode::SUCCESS,
        AppExit::Error(code) => std::process::ExitCode::from(code.get()),
    }
}

fn run(
    logging_ready: &mut bool,
    guard: &mut Option<LoggingGuard>,
    data_environment: Arc<OnceLock<DataEnvironment>>,
) -> Result<AppExit> {
    let executable = std::env::current_exe().map_err(diagnostics::operation)?;
    let logging = AppLogging::new(executable.with_file_name("logs"))
        .map_err(|failure| failure.into_bevy_error())?;
    let mut app = App::new();
    app.set_error_handler(diagnostics::handler);
    *guard = Some(logging.install(&mut app));
    app.add_plugins(log_plugin());
    *logging_ready = true;

    let config_path = executable.with_file_name("config").join("config.ron");
    let loaded = config::load(config_path.clone()).map_err(|error| {
        diagnostics::operation(std::io::Error::new(
            error.kind(),
            format!(
                "无法加载应用配置 {}：{error}；原文件保持不变",
                display_path(&config_path)
            ),
        ))
    })?;
    let config = loaded.config.clone();
    let config_store = config::ConfigStore::new(loaded);
    let save_config = config_store.clone();
    let login_ui = LoginUiPlugin::new(
        config.data_directory.clone(),
        data_environment,
        move |directory| save_config.remember_data_directory(directory),
    );
    app.insert_resource(
        WidgetryUiScale::new(config.scale_factor).map_err(|failure| failure.into_bevy_error())?,
    )
    .add_plugins(WidgetryUiScalePlugin);
    app.insert_resource(WinitSettings::desktop_app());

    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<LogPlugin>()
            .set(WindowPlugin {
                primary_window: Some(initial_window()),
                ..default()
            })
            .set(RenderPlugin {
                render_creation: block_on(transparent_render_creation())
                    .map_err(|failure| failure.into_bevy_error())?,
                ..default()
            }),
    );
    if config.brp_enabled {
        app.add_plugins(BrpRuntimePlugin::with_port(config.brp_port));
    }
    app.insert_resource(config_store)
        .add_plugins((AppAssetPlugin, WidgetryWindowPlugin, login_ui))
        .add_systems(Startup, setup)
        .add_observer(enter_main);
    let manager = app.world().resource::<JobManager>().clone();
    debug!("应用装配完成，进入主循环");
    let exit = app.run();
    finish_jobs(&manager)?;
    Ok(exit)
}

fn finish_jobs(manager: &JobManager) -> Result {
    let shutdown = manager
        .begin_shutdown(JOB_SHUTDOWN_TIMEOUT)
        .map_err(|failure| failure.into_bevy_error())?;
    let mut timeout_reported = false;
    loop {
        let status = shutdown.status();
        if status.phase == ShutdownPhase::Finished {
            debug!("应用辅助后台任务已全部结束");
            return Ok(());
        }
        if status.phase == ShutdownPhase::TimedOut && !timeout_reported {
            warn!(
                remaining = status.remaining.len(),
                "后台任务收尾超时，继续等待实际结束"
            );
            timeout_reported = true;
        }
        std::thread::sleep(JOB_SHUTDOWN_POLL_INTERVAL);
    }
}

fn enter_main(
    _event: On<DataDirectoryReady>,
    mut primary: Query<&mut Window, With<PrimaryWindow>>,
) -> Result {
    let mut window = primary.single_mut().map_err(diagnostics::operation)?;
    window.title = "agentelier".into();
    window.resolution.set(1920.0, 1080.0);
    debug!("数据目录准备完成，进入主界面");
    Ok(())
}

fn initial_window() -> Window {
    prepare_native_window(Window {
        title: "选择应用数据目录".into(),
        resolution: WindowResolution::new(1100, 760),
        position: WindowPosition::Centered(MonitorSelection::Primary),
        ..default()
    })
}

fn setup(mut commands: Commands, primary_window: Query<Entity, With<PrimaryWindow>>) -> Result {
    let target = primary_window.single().map_err(diagnostics::operation)?;
    let camera = commands.spawn(Camera2d).id();
    commands.spawn_scene_with_error_handler(bsn! {
        @widgetry_window(
            target,
            camera,
            WidgetryWindowControlsConfig::default(),
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! {
                bevy_widgetry::window::WidgetryWindowTitle
                bevy_widgetry::text::WidgetryText
                template(|_| Ok(Pickable::IGNORE))
            },
            bsn_list! {
                LoginUiHost
                Node { width: percent(100), height: percent(100) }
            }
        )
    });
    Ok(())
}
