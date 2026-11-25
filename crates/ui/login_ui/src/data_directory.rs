use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_login::DataEnvironment;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy::window::{MonitorSelection, WindowCloseRequested, WindowClosed, WindowPosition};
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogPlugin, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::window::{
    WidgetryModalWindow, WidgetryWindowBackground, owned_widgetry_window, widgetry_window_target,
};
use bevy_widgetry_path_display::display_path;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info};

type RememberDirectory = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;
const STARTUP_SURFACE: Color = Color::srgb_u8(0xF7, 0xFB, 0xF7);
const PICKER_WIDTH: u32 = 840;
const PICKER_HEIGHT: u32 = 580;

pub struct LoginUiPlugin {
    options: StartupOptions,
}

#[derive(Clone)]
struct StartupOptions {
    saved_directory: Option<PathBuf>,
    remember: RememberDirectory,
}

impl LoginUiPlugin {
    pub fn new(
        saved_directory: Option<PathBuf>,
        remember: impl Fn(&Path) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            options: StartupOptions {
                saved_directory,
                remember: Arc::new(remember),
            },
        }
    }
}

impl Plugin for LoginUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WidgetryFileDialogPlugin>() {
            app.add_plugins(WidgetryFileDialogPlugin);
        }
        crate::account::install(app);
        crate::authentication::install(app);
        app.insert_resource(StartupUi {
            options: self.options.clone(),
            host: None,
            main: None,
            screen: None,
            picker: None,
            phase: Phase::Waiting,
        })
        .add_observer(mount_host)
        .add_message::<BeginDataDirectorySetup>()
        .add_systems(Update, (begin_setup, close_picker, receive_check).chain());
    }
}

#[derive(Component, Clone, Default)]
pub struct LoginUiHost;

#[derive(Component, Clone, Default)]
pub struct HomeContent;

#[derive(Message, Clone, Copy)]
pub struct BeginDataDirectorySetup {
    pub main: Entity,
}

#[derive(Event)]
pub struct DataDirectoryReady {
    pub environment: DataEnvironment,
}

#[derive(Resource)]
struct StartupUi {
    options: StartupOptions,
    host: Option<Entity>,
    main: Option<Entity>,
    screen: Option<Entity>,
    picker: Option<Entity>,
    phase: Phase,
}

enum Phase {
    Waiting,
    Choosing,
    Checking(PendingCheck),
    Failed(PathBuf),
    Ready,
    Exiting,
}

#[derive(Component, Clone, Default)]
struct DirectoryPicker;

#[derive(Component, Clone, Default)]
struct DirectoryPickerStatus;

struct PendingCheck {
    path: PathBuf,
    id: JobId,
    receiver: JobReceiver<CheckResult>,
    result: Option<Arc<CheckResult>>,
}

enum CheckResult {
    Ready(DataEnvironment),
    Failed { title: &'static str, reason: String },
    Cancelled,
}

fn mount_host(event: On<Add<LoginUiHost>>, mut state: ResMut<StartupUi>) -> Result {
    if state.host.is_some() {
        return Err(io::Error::other("登录启动界面已经存在").into());
    }
    state.host = Some(event.entity);
    Ok(())
}

fn begin_setup(mut requests: MessageReader<BeginDataDirectorySetup>, mut commands: Commands) {
    let Some(BeginDataDirectorySetup { main }) = requests.read().last().copied() else {
        return;
    };
    commands.queue(move |world: &mut World| -> Result {
        host(world)?;
        let mut state = world.resource_mut::<StartupUi>();
        if state.main.is_some() {
            return Ok(());
        }
        state.main = Some(main);
        let saved = state.options.saved_directory.clone();
        if let Some(path) = saved {
            start_check(world, path)
        } else {
            show_picker(world)
        }
    });
}

fn clear_screen(world: &mut World) {
    if let Some(screen) = world.resource_mut::<StartupUi>().screen.take()
        && let Ok(entity) = world.get_entity_mut(screen)
    {
        entity.despawn();
    }
}

fn clear_picker(world: &mut World) {
    if let Some(picker) = world.resource_mut::<StartupUi>().picker.take()
        && let Ok(entity) = world.get_entity_mut(picker)
    {
        entity.despawn();
    }
}

fn host(world: &World) -> Result<Entity> {
    world
        .resource::<StartupUi>()
        .host
        .ok_or_else(|| io::Error::other("登录启动界面缺少内容宿主").into())
}

fn show_picker(world: &mut World) -> Result {
    let main = world
        .resource::<StartupUi>()
        .main
        .ok_or_else(|| io::Error::other("目录选择缺少主窗口"))?;
    clear_screen(world);
    let picker = world.commands().spawn_scene_with_error_handler(bsn! {
        Name("LoginDirectorySelection")
        template(move |_| Ok(WidgetryModalWindow { parent: main }))
        @owned_widgetry_window(
            Window {
                title: "选择应用数据目录".into(),
                resolution: (PICKER_WIDTH, PICKER_HEIGHT).into(),
                position: WindowPosition::Centered(MonitorSelection::Current),
                ..default()
            },
            agentelier_controls::FILE_DIALOG_WINDOW_CONTROLS,
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                Node {
                    width: percent(100), height: percent(100),
                    flex_direction: FlexDirection::Column, padding: UiRect::all(px(16)), row_gap: px(12),
                }
                Children [
                    Text("选择应用数据目录。浏览文件夹不会提交，点击确认后才会记住选择。")
                    WidgetryText DirectoryPickerStatus--
                    @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
                    Name("LoginDirectoryPicker") DirectoryPicker
                    Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                    on(selected)
                ]
            },
        )
    }).id();
    let mut state = world.resource_mut::<StartupUi>();
    state.picker = Some(picker);
    state.phase = Phase::Choosing;
    Ok(())
}

fn close_picker(
    world: &mut World,
    mut requested: Local<MessageCursor<WindowCloseRequested>>,
    mut closed: Local<MessageCursor<WindowClosed>>,
) {
    let state = world.resource::<StartupUi>();
    let native = state
        .picker
        .and_then(|root| widgetry_window_target(world, root));
    let picker_requested = requested
        .read(world.resource::<Messages<WindowCloseRequested>>())
        .any(|event| Some(event.window) == native);
    let window_closed = closed
        .read(world.resource::<Messages<WindowClosed>>())
        .any(|event| Some(event.window) == native || Some(event.window) == state.main);
    if state.picker.is_some() && (picker_requested || window_closed) {
        cancel_setup(world);
    }
}

fn cancel_setup(world: &mut World) {
    world.resource_mut::<StartupUi>().phase = Phase::Exiting;
    clear_picker(world);
    info!("用户取消数据目录选择，结束启动");
    world.write_message(AppExit::Success);
}

fn selected(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        if !matches!(world.resource::<StartupUi>().phase, Phase::Choosing) {
            return Ok(());
        }
        match result {
            WidgetryFileDialogResult::Directory(path) => start_check(world, path),
            WidgetryFileDialogResult::Cancelled => {
                cancel_setup(world);
                Ok(())
            }
            _ => Err(io::Error::other("目录选择器返回了不匹配的结果类型").into()),
        }
    });
}

fn start_check(world: &mut World, path: PathBuf) -> Result {
    clear_screen(world);
    let host = host(world)?;
    let screen = show_checking(world, host)?;
    world.resource_mut::<StartupUi>().screen = screen;
    let options = world.resource::<StartupUi>().options.clone();
    let manager = world.resource::<JobManager>().clone();
    let checking_path = path.clone();
    let submission = manager.spawn_thread(
        JobOptions { label: Some("检查应用数据目录并保存选择".into()), ..default() },
        move |context| {
            debug!(path = %display_path(&checking_path), "开始检查应用数据目录");
            if context.is_cancelled() {
                context.acknowledge_cancel();
                return Ok(());
            }
            let result = match DataEnvironment::open(&checking_path) {
                Err(failure) => {
                    error!(path = %display_path(&checking_path), error = %failure, "应用数据目录不可用");
                    CheckResult::Failed { title: "应用数据目录不可用", reason: failure.to_string() }
                }
                Ok(environment) => {
                    if context.is_cancelled() {
                        context.acknowledge_cancel();
                        return Ok(());
                    }
                    let save = if options.saved_directory.as_deref() != Some(checking_path.as_path()) {
                        (options.remember)(&checking_path)
                    } else {
                        Ok(())
                    };
                    match save {
                        Err(failure) => {
                            error!(path = %display_path(&checking_path), error = %failure, "无法保存数据目录选择");
                            CheckResult::Failed { title: "无法保存数据目录选择", reason: format!("{failure}\n原配置保持不变。") }
                        }
                        Ok(()) => {
                            if options.saved_directory.as_deref() != Some(checking_path.as_path()) {
                                info!(path = %display_path(&checking_path), "已保存应用数据目录选择");
                            }
                            debug!(path = %display_path(environment.root()), "本次数据环境已就绪");
                            CheckResult::Ready(environment)
                        }
                    }
                }
            };
            if context.send_blocking(result)? == SendStatus::Canceled {
                context.acknowledge_cancel();
            }
            debug!(path = %display_path(&checking_path), "数据目录准备任务已结束");
            Ok(())
        },
    );
    let (id, receiver) = match submission {
        Ok(submission) => submission,
        Err(failure) => {
            error!(path = %display_path(&path), error = %failure, "无法启动数据目录准备任务");
            return show_failure(
                world,
                path,
                "无法启动数据目录准备任务",
                &failure.to_string(),
            );
        }
    };
    let mut state = world.resource_mut::<StartupUi>();
    state.phase = Phase::Checking(PendingCheck {
        path,
        id,
        receiver,
        result: None,
    });
    Ok(())
}

fn show_checking(world: &mut World, host: Entity) -> Result<Option<Entity>> {
    let checking = "正在检查应用数据目录并保存选择…";
    if world.resource::<StartupUi>().picker.is_some() {
        let picker = world
            .query_filtered::<Entity, With<DirectoryPicker>>()
            .single(world)?;
        world
            .query_filtered::<&mut Text, With<DirectoryPickerStatus>>()
            .single_mut(world)?
            .0 = checking.into();
        world.entity_mut(picker).insert(InteractionDisabled);
        return Ok(None);
    }
    Ok(Some(
        world
            .commands()
            .spawn_scene_with_error_handler(bsn! {
                Name("LoginDirectoryChecking") ChildOf(host) BackgroundColor(STARTUP_SURFACE)
                Node { width: percent(100), height: percent(100), padding: UiRect::all(px(24)) }
                Children [Text(checking) WidgetryText]
            })
            .id(),
    ))
}

fn receive_check(world: &mut World) -> Result {
    let phase = std::mem::replace(&mut world.resource_mut::<StartupUi>().phase, Phase::Waiting);
    let Phase::Checking(mut pending) = phase else {
        world.resource_mut::<StartupUi>().phase = phase;
        return Ok(());
    };
    let completed = loop {
        match pending
            .receiver
            .try_recv()
            .map_err(|failure| failure.into_bevy_error())?
        {
            JobRead::Item(result) => pending.result = Some(result),
            JobRead::Finished(outcome) => break outcome,
            JobRead::Empty => {
                world.resource_mut::<StartupUi>().phase = Phase::Checking(pending);
                return Ok(());
            }
            JobRead::Closed => return Err(io::Error::other("数据目录准备任务未交付终态").into()),
        }
    };
    world
        .resource::<JobManager>()
        .remove(pending.id)
        .map_err(|failure| failure.into_bevy_error())?;
    let result = match completed {
        JobOutcome::Completed => pending
            .result
            .ok_or_else(|| io::Error::other("数据目录准备任务未交付结果"))?,
        JobOutcome::Canceled => Arc::new(CheckResult::Cancelled),
        JobOutcome::Failed(failure) => {
            error!(error = %failure, "数据目录后台任务失败");
            Arc::new(CheckResult::Failed {
                title: "数据目录后台任务失败",
                reason: failure.to_string(),
            })
        }
    };
    match result.as_ref() {
        CheckResult::Ready(environment) => {
            clear_screen(world);
            clear_picker(world);
            world.resource_mut::<StartupUi>().phase = Phase::Ready;
            let host = host(world)?;
            world.commands().spawn_scene_with_error_handler(bsn! {
                HomeContent Name("HomeContent") ChildOf(host)
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
            });
            world.flush();
            world.trigger(DataDirectoryReady {
                environment: environment.clone(),
            });
            Ok(())
        }
        CheckResult::Failed { title, reason } => show_failure(world, pending.path, title, reason),
        CheckResult::Cancelled => {
            world.write_message(AppExit::Success);
            Ok(())
        }
    }
}

#[derive(Component, Clone, Copy)]
enum RecoveryChoice {
    Retry,
    Select,
    Cancel,
}

fn recovery_button(label: &'static str, name: &'static str, choice: RecoveryChoice) -> impl Scene {
    let style = match choice {
        RecoveryChoice::Retry => AgentelierButtonStyle::Primary,
        RecoveryChoice::Select | RecoveryChoice::Cancel => AgentelierButtonStyle::Secondary,
    };
    bsn! {
        @AgentelierButton {
            @style: style,
            @children: {Some(Box::new(bsn_list! {Text(label) WidgetryText}) as Box<dyn SceneList>)}
        }
        Name(name) Node { min_width: px(128), height: px(40) }
        template(move |_| Ok(choice))
        on(recover)
    }
}

fn show_failure(world: &mut World, path: PathBuf, title: &str, reason: &str) -> Result {
    clear_screen(world);
    clear_picker(world);
    let host = host(world)?;
    let text = format!("{title}\n\n目录：{}\n原因：{reason}", display_path(&path));
    let screen = world
        .commands()
        .spawn_scene_with_error_handler(bsn! {
            Name("LoginDirectoryFailure") ChildOf(host) BackgroundColor(STARTUP_SURFACE)
            Node {
                width: percent(100), height: percent(100), flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(24)), row_gap: px(24),
            }
            Children [
                Text(text) WidgetryText Name("LoginDirectoryFailureReason")--
                Node { column_gap: px(12), flex_wrap: FlexWrap::Wrap }
                Children [
                    @recovery_button("重试", "LoginDirectoryRetry", RecoveryChoice::Retry)--
                    @recovery_button("重新选择", "LoginDirectoryReselect", RecoveryChoice::Select)--
                    @recovery_button("取消", "LoginDirectoryCancel", RecoveryChoice::Cancel)
                ]
            ]
        })
        .id();
    let mut state = world.resource_mut::<StartupUi>();
    state.screen = Some(screen);
    state.phase = Phase::Failed(path);
    Ok(())
}

fn recover(event: On<Activate>, choices: Query<&RecoveryChoice>, mut commands: Commands) -> Result {
    let choice = *choices.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        let Phase::Failed(path) = &world.resource::<StartupUi>().phase else {
            return Ok(());
        };
        let path = path.clone();
        match choice {
            RecoveryChoice::Retry => start_check(world, path),
            RecoveryChoice::Select => show_picker(world),
            RecoveryChoice::Cancel => {
                info!("用户取消数据目录准备，结束启动");
                world.write_message(AppExit::Success);
                Ok(())
            }
        }
    });
    Ok(())
}
