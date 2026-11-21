use crate::data_directory::RememberDirectory;
use agentelier_login::{DataEnvironment, validate_data_directory};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry_path_display::display_path;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info};

const PANEL_PADDING: f32 = 24.0;
const FIELD_GAP: f32 = 16.0;
const BUTTON_HEIGHT: f32 = 40.0;
const BUTTON_WIDTH: f32 = 144.0;
const BUTTON_GAP: f32 = 12.0;

#[derive(Component, Clone, Default)]
pub struct HomeContent;

#[derive(Resource)]
struct DirectorySettings {
    host: Entity,
    screen: Option<Entity>,
    current: DataEnvironment,
    next_directory: PathBuf,
    remember: RememberDirectory,
    view: View,
    next_operation_id: u64,
    pending: Option<PendingOperation>,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum View {
    Home,
    Settings,
    Choosing(u64),
}

#[derive(Component, Clone, Copy)]
enum Action {
    Settings,
    Home,
    Change,
    Open,
}

#[derive(Component, Clone, Copy)]
struct SelectionOperation(u64);

struct PendingOperation {
    operation_id: u64,
    job_id: JobId,
    kind: Operation,
    receiver: JobReceiver<OperationReply>,
    reply: Option<Arc<OperationReply>>,
}

#[derive(Clone)]
enum Operation {
    Save(PathBuf),
    Open,
}

struct OperationReply {
    operation_id: u64,
    result: io::Result<OperationSuccess>,
}

enum OperationSuccess {
    Saved(PathBuf),
    Opened,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        Update,
        receive_operation.run_if(resource_exists::<DirectorySettings>),
    );
}

pub(crate) fn start(
    world: &mut World,
    host: Entity,
    current: DataEnvironment,
    remember: RememberDirectory,
) -> Result {
    world.insert_resource(DirectorySettings {
        host,
        screen: None,
        next_directory: current.root().to_path_buf(),
        current,
        remember,
        view: View::Home,
        next_operation_id: 0,
        pending: None,
        error: None,
    });
    render(world)
}

fn allocate_operation(world: &mut World) -> Result<u64> {
    let mut state = world.resource_mut::<DirectorySettings>();
    state.next_operation_id = state
        .next_operation_id
        .checked_add(1)
        .ok_or_else(|| io::Error::other("数据目录操作标识已耗尽"))?;
    Ok(state.next_operation_id)
}

fn button(label: &'static str, name: &'static str, action: Action, disabled: bool) -> impl Scene {
    (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! {Text(label) WidgetryText}) as Box<dyn SceneList>)}
            }
            Name(name) Node { min_width: px(BUTTON_WIDTH), height: px(BUTTON_HEIGHT), flex_shrink: 0.0 }
            template(move |_| Ok(action))
            on(activate)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn render(world: &mut World) -> Result {
    let state = world.resource::<DirectorySettings>();
    let host = state.host;
    let view = state.view;
    let current = state.current.root().to_path_buf();
    let current_text = display_path(&current);
    let next = state.next_directory.clone();
    let next_text = display_path(&next);
    let failure = state.error.clone().unwrap_or_default();
    let busy = state.pending.is_some();
    let status = match state.pending.as_ref().map(|pending| &pending.kind) {
        Some(Operation::Save(_)) => "正在检查并保存下次启动目录…",
        Some(Operation::Open) => "正在打开当前目录…",
        None => "",
    };
    if let Some(screen) = world.resource_mut::<DirectorySettings>().screen.take()
        && let Ok(entity) = world.get_entity_mut(screen)
    {
        entity.despawn();
    }
    let account = world.resource::<crate::AccountState>().clone();
    let model_busy = world
        .resource::<agentelier_agent_host::AgentHost>()
        .model_busy()?;
    let screen = match view {
        View::Home => world.commands().spawn_scene_with_error_handler(bsn! {
            Name("DirectorySettingsHome") ChildOf(host)
            Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Start, padding: UiRect::all(px(PANEL_PADDING)), row_gap: px(FIELD_GAP) }
            Children [
                @button("设置", "DirectorySettingsEntry", Action::Settings, false)--
                @crate::account::account_panel(&account, model_busy)--
                HomeContent Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
            ]
        }).id(),
        View::Settings => {
            let next_section = (next != current).then(|| bsn! {
                Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(FIELD_GAP), flex_shrink: 0.0 }
                Children [
                    Text("下次启动使用") WidgetryText--
                    Text(next_text) WidgetryText Name("DirectorySettingsNextPath")
                    TextLayout::linebreak(LineBreak::AnyCharacter)
                    Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
                    Text("更改将在下次启动生效；文件由你自行复制和整理") WidgetryText Name("DirectorySettingsRestartNotice")
                ]
            });
            world.commands().spawn_scene_with_error_handler(bsn! {
                Name("DirectorySettingsPage") ChildOf(host)
                Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(PANEL_PADDING)), row_gap: px(FIELD_GAP) }
                Children [
                    @button("返回主界面", "DirectorySettingsBack", Action::Home, false)--
                    @WidgetryScrollArea {
                        @children: {Some(Box::new(bsn_list! {
                            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(FIELD_GAP) }
                            Children [
                                Text("设置 / 数据与存储") WidgetryText--
                                Text("当前使用") WidgetryText--
                                Text(current_text) WidgetryText Name("DirectorySettingsCurrentPath")
                                TextLayout::linebreak(LineBreak::AnyCharacter)
                                Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
                                Node { column_gap: px(BUTTON_GAP), row_gap: px(BUTTON_GAP), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0 }
                                Children [
                                    @button("打开目录", "DirectorySettingsOpen", Action::Open, busy)--
                                    @button("更改目录", "DirectorySettingsChange", Action::Change, busy)
                                ]--
                                {next_section}--
                                Text(status) WidgetryText Name("DirectorySettingsStatus")--
                                Text(failure) WidgetryText Name("DirectorySettingsError")
                                TextLayout::linebreak(LineBreak::AnyCharacter)
                                Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
                            ]
                        }) as Box<dyn SceneList>)}
                    }
                    Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                ]
            }).id()
        }
        View::Choosing(operation_id) => world.commands().spawn_scene_with_error_handler(bsn! {
            Name("DirectorySettingsSelection") ChildOf(host)
            Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(PANEL_PADDING)), row_gap: px(FIELD_GAP) }
            Children [
                @button("返回主界面", "DirectorySettingsCloseSelection", Action::Home, false)--
                Text("选择下次启动使用的数据目录。当前目录保持不变，文件需要你自行整理。") WidgetryText--
                @WidgetryFileDialog {
                    @mode: WidgetryFileDialogMode::PickDirectory,
                    @initial_directory: {Some(current)},
                }
                Name("DirectorySettingsPicker")
                template(move |_| Ok(SelectionOperation(operation_id)))
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(selected)
            ]
        }).id(),
    };
    world.resource_mut::<DirectorySettings>().screen = Some(screen);
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        match action {
            Action::Home => {
                let mut state = world.resource_mut::<DirectorySettings>();
                if matches!(state.view, View::Choosing(_)) {
                    info!("用户关闭目录选择，保留已保存的下次启动位置");
                }
                state.view = View::Home;
                render(world)
            }
            Action::Settings => {
                world.resource_mut::<DirectorySettings>().view = View::Settings;
                render(world)
            }
            Action::Change | Action::Open => {
                let state = world.resource::<DirectorySettings>();
                if state.pending.is_some() || !matches!(state.view, View::Settings) {
                    return Ok(());
                }
                let operation_id = allocate_operation(world)?;
                world.resource_mut::<DirectorySettings>().error = None;
                if matches!(action, Action::Change) {
                    info!(operation_id, "用户请求更改下次启动的数据目录");
                    world.resource_mut::<DirectorySettings>().view = View::Choosing(operation_id);
                    render(world)
                } else {
                    start_operation(world, operation_id, Operation::Open)
                }
            }
        }
    });
    Ok(())
}

fn selected(
    event: On<WidgetryFileDialogResultEvent>,
    operations: Query<&SelectionOperation>,
    mut commands: Commands,
) -> Result {
    let operation_id = operations.get(event.entity)?.0;
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        if !matches!(world.resource::<DirectorySettings>().view, View::Choosing(id) if id == operation_id) {
            return Ok(());
        }
        world.resource_mut::<DirectorySettings>().view = View::Settings;
        match result {
            WidgetryFileDialogResult::Directory(path) => start_operation(world, operation_id, Operation::Save(path)),
            WidgetryFileDialogResult::Cancelled => {
                info!(operation_id, "用户取消更改目录，保留已保存的下次启动位置");
                render(world)
            }
            _ => Err(io::Error::other("目录设置选择器返回了不匹配的结果类型").into()),
        }
    });
    Ok(())
}

fn start_operation(world: &mut World, operation_id: u64, operation: Operation) -> Result {
    let state = world.resource::<DirectorySettings>();
    let current = state.current.clone();
    let remember = state.remember.clone();
    let work = operation.clone();
    let submission = world.resource::<JobManager>().spawn_thread(
        JobOptions { label: Some("数据目录设置操作".into()), ..default() },
        move |context| {
            debug!(operation_id, "数据目录设置任务开始");
            if context.is_cancelled() {
                context.acknowledge_cancel();
                return Ok(());
            }
            let result = match work {
                Operation::Open => current.open_in_explorer().map(|()| OperationSuccess::Opened),
                Operation::Save(path) => match validate_data_directory(&path) {
                    Err(failure) => Err(failure),
                    Ok(path) => {
                        if context.is_cancelled() {
                            context.acknowledge_cancel();
                            return Ok(());
                        }
                        remember(&path).map(|()| OperationSuccess::Saved(path))
                    }
                },
            };
            match &result {
                Ok(OperationSuccess::Opened) => info!(operation_id, path = %display_path(current.root()), "已请求资源管理器打开当前数据目录"),
                Ok(OperationSuccess::Saved(path)) => info!(operation_id, path = %display_path(path), "已保存下次启动使用的数据目录"),
                Err(failure) => error!(operation_id, error = %failure, "数据目录设置操作失败"),
            }
            if context.send_blocking(OperationReply {operation_id, result})? == SendStatus::Canceled {
                context.acknowledge_cancel();
            }
            debug!(operation_id, "数据目录设置任务结束");
            Ok(())
        },
    );
    match submission {
        Ok((job_id, receiver)) => {
            world.resource_mut::<DirectorySettings>().pending = Some(PendingOperation {
                operation_id,
                job_id,
                kind: operation,
                receiver,
                reply: None,
            });
        }
        Err(failure) => {
            error!(operation_id, error = %failure, "无法启动数据目录设置任务");
            world.resource_mut::<DirectorySettings>().error = Some(failure.to_string());
        }
    }
    render(world)
}

fn receive_operation(world: &mut World) -> Result {
    let Some(mut pending) = world.resource_mut::<DirectorySettings>().pending.take() else {
        return Ok(());
    };
    let outcome = loop {
        match pending
            .receiver
            .try_recv()
            .map_err(|failure| failure.into_bevy_error())?
        {
            JobRead::Item(reply) => pending.reply = Some(reply),
            JobRead::Finished(outcome) => break outcome,
            JobRead::Empty => {
                world.resource_mut::<DirectorySettings>().pending = Some(pending);
                return Ok(());
            }
            JobRead::Closed => return Err(io::Error::other("数据目录设置任务缺少终态").into()),
        }
    };
    world
        .resource::<JobManager>()
        .remove(pending.job_id)
        .map_err(|failure| failure.into_bevy_error())?;
    match outcome {
        JobOutcome::Completed => {
            let reply = pending
                .reply
                .ok_or_else(|| io::Error::other("数据目录设置任务缺少结果"))?;
            if reply.operation_id != pending.operation_id {
                return Err(io::Error::other("数据目录设置结果不属于当前操作").into());
            }
            match &reply.result {
                Ok(OperationSuccess::Saved(path)) => {
                    world.resource_mut::<DirectorySettings>().next_directory = path.clone()
                }
                Ok(OperationSuccess::Opened) => {}
                Err(failure) => {
                    world.resource_mut::<DirectorySettings>().error = Some(format!(
                        "{}：{failure}",
                        match pending.kind {
                            Operation::Save(path) =>
                                format!("无法设置下次启动目录 {}", display_path(&path)),
                            Operation::Open => "无法打开当前目录".into(),
                        }
                    ))
                }
            }
        }
        JobOutcome::Failed(failure) => {
            error!(operation_id = pending.operation_id, error = %failure, "数据目录设置任务失败");
            world.resource_mut::<DirectorySettings>().error = Some(failure.to_string());
        }
        JobOutcome::Canceled => debug!(
            operation_id = pending.operation_id,
            "数据目录设置任务已取消"
        ),
    }
    render(world)
}
