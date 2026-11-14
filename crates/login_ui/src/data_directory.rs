use agentelier_login::DataEnvironment;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogPlugin, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry_path_display::display_path;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tracing::{debug, error, info};

pub(crate) type RememberDirectory = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;

pub struct LoginUiPlugin {
    options: StartupOptions,
}

#[derive(Clone)]
struct StartupOptions {
    saved_directory: Option<PathBuf>,
    environment: Arc<OnceLock<DataEnvironment>>,
    remember: RememberDirectory,
}

impl LoginUiPlugin {
    pub fn new(
        saved_directory: Option<PathBuf>,
        environment: Arc<OnceLock<DataEnvironment>>,
        remember: impl Fn(&Path) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            options: StartupOptions {
                saved_directory,
                environment,
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
        crate::directory_settings::install(app);
        crate::account::install(app);
        crate::model::install(app);
        app.insert_resource(StartupUi {
            options: self.options.clone(),
            host: None,
            screen: None,
            phase: Phase::Waiting,
        })
        .add_observer(initialize)
        .add_systems(Update, receive_check);
    }
}

#[derive(Component, Clone, Default)]
pub struct LoginUiHost;

#[derive(Event)]
pub struct DataDirectoryReady {
    pub environment: DataEnvironment,
}

#[derive(Resource)]
struct StartupUi {
    options: StartupOptions,
    host: Option<Entity>,
    screen: Option<Entity>,
    phase: Phase,
}

enum Phase {
    Waiting,
    Choosing,
    Checking(PendingCheck),
    Failed(PathBuf),
    Ready,
}

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

fn initialize(event: On<Add<LoginUiHost>>, mut commands: Commands) {
    let host = event.entity;
    commands.queue(move |world: &mut World| -> Result {
        let mut state = world.resource_mut::<StartupUi>();
        if state.host.is_some() {
            return Err(io::Error::other("登录启动界面已经存在").into());
        }
        state.host = Some(host);
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

fn host(world: &World) -> Result<Entity> {
    world
        .resource::<StartupUi>()
        .host
        .ok_or_else(|| io::Error::other("登录启动界面缺少内容宿主").into())
}

fn show_picker(world: &mut World) -> Result {
    clear_screen(world);
    let host = host(world)?;
    world.resource_mut::<StartupUi>().phase = Phase::Choosing;
    let screen = world.commands().spawn_scene_with_error_handler(bsn! {
        Name("LoginDirectorySelection") ChildOf(host)
        Node {
            width: percent(100), height: percent(100),
            flex_direction: FlexDirection::Column, padding: UiRect::all(px(16)), row_gap: px(12),
        }
        Children [
            Text("选择应用数据目录。浏览文件夹不会提交，点击确认后才会记住选择。") WidgetryText--
            @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
            Name("LoginDirectoryPicker")
            Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
            on(selected)
        ]
    }).id();
    world.resource_mut::<StartupUi>().screen = Some(screen);
    Ok(())
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
                info!("用户取消数据目录选择，结束启动");
                world.write_message(AppExit::Success);
                Ok(())
            }
            _ => Err(io::Error::other("目录选择器返回了不匹配的结果类型").into()),
        }
    });
}

fn start_check(world: &mut World, path: PathBuf) -> Result {
    clear_screen(world);
    let host = host(world)?;
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
            let result = match DataEnvironment::acquire(&checking_path) {
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
                            if options.environment.set(environment.clone()).is_err() {
                                CheckResult::Failed { title: "无法建立数据环境", reason: "本次运行的数据环境已建立，不能重复切换".into() }
                            } else {
                                debug!(path = %display_path(environment.root()), credentials_path = %display_path(environment.credentials_path()), "本次数据环境已就绪并取得独占占用");
                                CheckResult::Ready(environment)
                            }
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
    let screen = world
        .commands()
        .spawn_scene_with_error_handler(bsn! {
            Name("LoginDirectoryChecking") ChildOf(host)
            Node { width: percent(100), height: percent(100), padding: UiRect::all(px(24)) }
            Children [Text("正在检查应用数据目录并保存选择…") WidgetryText]
        })
        .id();
    let mut state = world.resource_mut::<StartupUi>();
    state.screen = Some(screen);
    state.phase = Phase::Checking(PendingCheck {
        path,
        id,
        receiver,
        result: None,
    });
    Ok(())
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
            world.resource_mut::<StartupUi>().phase = Phase::Ready;
            crate::directory_settings::start(
                world,
                host(world)?,
                environment.clone(),
                world.resource::<StartupUi>().options.remember.clone(),
            )?;
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
    bsn! {
        @WidgetryButton {
            @children: {Some(Box::new(bsn_list! {Text(label) WidgetryText}) as Box<dyn SceneList>)}
        }
        Name(name) Node { min_width: px(128), height: px(40) }
        template(move |_| Ok(choice))
        on(recover)
    }
}

fn show_failure(world: &mut World, path: PathBuf, title: &str, reason: &str) -> Result {
    clear_screen(world);
    let host = host(world)?;
    let text = format!("{title}\n\n目录：{}\n原因：{reason}", display_path(&path));
    let screen = world
        .commands()
        .spawn_scene_with_error_handler(bsn! {
            Name("LoginDirectoryFailure") ChildOf(host)
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
