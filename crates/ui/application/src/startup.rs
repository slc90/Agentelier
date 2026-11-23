use crate::{config, diagnostics, settings, shutdown};
use agentelier_agent_host::AgentHostPlugin;
use agentelier_chat_ui::{MAIN_WINDOW_WIDTH, MIN_MAIN_WINDOW_WIDTH, workspace_title};
use agentelier_controls::{AGENTELIER_THEME, AgentelierControlsPlugin};
use agentelier_login::DataEnvironment;
use agentelier_login_ui::{DataDirectoryReady, HomeContent, LoginUiHost, LoginUiPlugin};
use bevy::log::LogPlugin;
use bevy::window::{
    ExitCondition, MonitorSelection, PrimaryWindow, WindowPosition, WindowResolution,
};
use bevy::winit::WinitSettings;
use bevy::{prelude::*, render::RenderPlugin, tasks::block_on};
use bevy_app_assets::AppAssetPlugin;
use bevy_brp_runtime::BrpRuntimePlugin;
use bevy_widgetry::app_logging::{AppLogging, LoggingGuard, log_plugin};
use bevy_widgetry::job::JobManager;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::ui_scale::{WidgetryUiScale, WidgetryUiScalePlugin};
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowPlugin,
    prepare_native_window, transparent_render_creation, widgetry_window,
};
use bevy_widgetry_path_display::display_path;
use std::sync::{Arc, OnceLock};
use tracing::debug;

const MAIN_WINDOW_HEIGHT: f32 = 1080.0;

pub fn run() -> std::process::ExitCode {
    let mut logging_ready = false;
    let mut guard: Option<LoggingGuard> = None;
    let data_environment = Arc::new(OnceLock::new());
    let result = run_application(&mut logging_ready, &mut guard, data_environment.clone());
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

fn run_application(
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
    let mut logging_plugin = log_plugin();
    logging_plugin.filter.push_str(",rmcp=off");
    app.add_plugins(logging_plugin);
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
                exit_condition: ExitCondition::OnPrimaryClosed,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: block_on(transparent_render_creation())
                    .map_err(|failure| failure.into_bevy_error())?,
                ..default()
            }),
    );
    if config.brp_enabled {
        app.add_plugins(BrpRuntimePlugin::with_ports(
            config.brp_port,
            config.brp_render_port,
        ));
    }
    let host_plugin = AgentHostPlugin::default();
    let backend = host_plugin.host();
    app.insert_resource(config_store.clone())
        .insert_resource(agentelier_chat_ui::WorkspaceLayout::new(
            config.workspace_layout,
        ))
        .insert_resource(AGENTELIER_THEME)
        .add_plugins((
            AppAssetPlugin,
            AgentelierControlsPlugin,
            WidgetryWindowPlugin,
            host_plugin,
            login_ui,
        ))
        .add_systems(Startup, setup)
        .add_observer(enter_main)
        .add_observer(mount_chat);
    agentelier_chat_ui::install(&mut app, move || config_store.session_model_defaults())?;
    settings::install(&mut app)?;
    let manager = app.world().resource::<JobManager>().clone();
    shutdown::install(&mut app);
    debug!("应用装配完成，进入主循环");
    let exit = app.run();
    let backend_result = backend.finish();
    let jobs_result = shutdown::finish_jobs(&manager);
    backend_result?;
    jobs_result?;
    Ok(exit)
}

fn enter_main(
    _event: On<DataDirectoryReady>,
    mut primary: Query<&mut Window, With<PrimaryWindow>>,
) -> Result {
    let mut window = primary.single_mut().map_err(diagnostics::operation)?;
    window.title = "agentelier".into();
    window.resize_constraints.min_width = MIN_MAIN_WINDOW_WIDTH;
    window.resolution.set(MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT);
    window.position = WindowPosition::Centered(MonitorSelection::Primary);
    debug!("数据目录准备完成，进入主界面");
    Ok(())
}

fn initial_window() -> Window {
    let mut window = Window {
        title: "选择应用数据目录".into(),
        resolution: WindowResolution::new(1100, 760),
        position: WindowPosition::Centered(MonitorSelection::Primary),
        ..default()
    };
    window.enabled_buttons.maximize = false;
    prepare_native_window(window)
}

fn setup(mut commands: Commands, primary_window: Query<Entity, With<PrimaryWindow>>) -> Result {
    let target = primary_window.single().map_err(diagnostics::operation)?;
    let camera = commands.spawn(Camera2d).id();
    commands.spawn_scene_with_error_handler(bsn! {
        @widgetry_window(
            target,
            camera,
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { @workspace_title() },
            bsn_list! {
                LoginUiHost
                Node { width: percent(100), height: percent(100) }
            }
        )
    });
    Ok(())
}

fn mount_chat(event: On<Add<HomeContent>>, mut commands: Commands) {
    let host = event.entity;
    commands.spawn_scene_with_error_handler(bsn! {
        agentelier_chat_ui::ChatUiHost ChildOf(host)
        Node { width: percent(100), height: percent(100), min_height: px(0) }
    });
}
