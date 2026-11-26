use crate::{agent_scenes, config, diagnostics, settings, shutdown, splash, tray};
use agentelier_agent_host::AgentHostPlugin;
use agentelier_chat_ui::{MAIN_WINDOW_WIDTH, OpenSettings, workspace_navigation};
use agentelier_controls::{AGENTELIER_THEME, AgentelierControlsPlugin};
use agentelier_login_ui::{HomeContent, LoginUiHost, LoginUiPlugin};
use bevy::log::LogPlugin;
use bevy::window::{
    ExitCondition, MonitorSelection, PrimaryWindow, WindowPosition, WindowResolution,
};
use bevy::winit::WinitSettings;
use bevy::{prelude::*, render::RenderPlugin, tasks::block_on};
use bevy_app_assets::{AppAssetPlugin, MainWindowBackground};
use bevy_brp_runtime::BrpRuntimePlugin;
use bevy_widgetry::app_logging::{AppLogging, LoggingGuard, log_plugin};
use bevy_widgetry::job::JobManager;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::ui_scale::{WidgetryUiScale, WidgetryUiScalePlugin};
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowImageBackground,
    WidgetryWindowImageMode, WidgetryWindowPlugin, prepare_native_window,
    transparent_render_creation, widgetry_window,
};
use bevy_widgetry_path_display::display_path;
use tracing::debug;

const MAIN_WINDOW_HEIGHT: f32 = 1080.0;

pub fn run() -> std::process::ExitCode {
    let mut logging_ready = false;
    let mut guard: Option<LoggingGuard> = None;
    let result = run_application(&mut logging_ready, &mut guard);
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
    match exit {
        AppExit::Success => std::process::ExitCode::SUCCESS,
        AppExit::Error(code) => std::process::ExitCode::from(code.get()),
    }
}

fn run_application(logging_ready: &mut bool, guard: &mut Option<LoggingGuard>) -> Result<AppExit> {
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
    let login_ui = LoginUiPlugin::new(config.data_directory.clone(), move |directory| {
        save_config.remember_data_directory(directory)
    });
    app.insert_resource(
        WidgetryUiScale::new(config.scale_factor).map_err(|failure| failure.into_bevy_error())?,
    )
    .add_plugins(WidgetryUiScalePlugin);
    app.insert_resource(WinitSettings::continuous());

    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<LogPlugin>()
            .set(WindowPlugin {
                primary_window: Some(initial_window()),
                exit_condition: ExitCondition::DontExit,
                close_when_requested: false,
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
    let host_plugin = AgentHostPlugin::default().with_ecs_unit_event(
        "OpenSettings",
        "Open Agentelier's settings through its application observer.",
        || OpenSettings,
    )?;
    let host_plugin = agent_scenes::register(host_plugin)?;
    let backend = host_plugin.host();
    app.insert_resource(config_store.clone())
        .insert_resource(AGENTELIER_THEME)
        .add_plugins((
            AppAssetPlugin,
            AgentelierControlsPlugin,
            WidgetryWindowPlugin,
            host_plugin,
            login_ui,
        ))
        .add_systems(Startup, setup)
        .add_observer(mount_chat);
    agent_scenes::install(&mut app);
    agentelier_chat_ui::install(&mut app, move || config_store.session_model_defaults())?;
    settings::install(&mut app)?;
    tray::install(&mut app)?;
    splash::install(&mut app);
    crate::window_alpha::install(&mut app);
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

fn initial_window() -> Window {
    let mut window = Window {
        title: "agentelier".into(),
        visible: false,
        resizable: false,
        resolution: WindowResolution::new(MAIN_WINDOW_WIDTH as u32, MAIN_WINDOW_HEIGHT as u32),
        position: WindowPosition::Centered(MonitorSelection::Primary),
        ..default()
    };
    window.enabled_buttons.maximize = false;
    prepare_native_window(window)
}

fn setup(
    mut commands: Commands,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    background: Res<MainWindowBackground>,
) -> Result {
    let target = primary_window.single().map_err(diagnostics::operation)?;
    let camera = commands.spawn(Camera2d).id();
    let background = WidgetryWindowBackground::Image(WidgetryWindowImageBackground {
        image: background.0.clone(),
        mode: WidgetryWindowImageMode::Cover,
        opacity: 1.0,
    });
    commands.spawn_scene_with_error_handler(bsn! {
        @widgetry_window(
            target,
            camera,
            WidgetryWindowControlsConfig { maximize_visible: false, resizable: false, double_click_maximize: false, ..default() },
            background,
            Default::default(),
            bsn_list! { @workspace_navigation() },
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
