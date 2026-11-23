use agentelier_agent_host::{GlobalTaskNotificationEvent, ReceiveAgentEvents};
use agentelier_conversation_bridge::{TaskId, TaskNotification, TaskNotificationKind};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future::or;
use bevy::window::{
    EnabledButtons, Monitor, OnMonitor, PrimaryMonitor, PrimaryWindow, WindowCloseRequested,
    WindowLevel, WindowPosition, WindowResolution,
};
use bevy_widgetry::job::{JobId, JobManager, JobOptions};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::ui_scale::WidgetryUiScale;
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowTitle,
    owned_widgetry_window,
};
use futures_timer::Delay;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

const LIFETIME: Duration = Duration::from_secs(5);
const WIDTH: u32 = 360;
const HEIGHT: u32 = 136;
const GAP: f32 = 12.0;
const RIGHT_MARGIN: f32 = 24.0;
const BOTTOM_MARGIN: f32 = 56.0;
const MAX_VISIBLE: usize = 3;

#[derive(Resource, Default)]
struct ToastQueue {
    pending: VecDeque<TaskNotification>,
    seen: HashSet<(TaskId, TaskNotificationKind)>,
    closing: bool,
}

#[derive(Component, Clone, Copy)]
struct Toast {
    slot: usize,
    deadline: Instant,
    timer: JobId,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ToastQueue>()
        .add_observer(cancel_timer)
        .add_systems(Update, (receive, show).chain().after(ReceiveAgentEvents));
}

fn receive(
    mut events: MessageReader<GlobalTaskNotificationEvent>,
    mut close: MessageReader<WindowCloseRequested>,
    mut exit: MessageReader<AppExit>,
    main: Query<Entity, With<PrimaryWindow>>,
    mut state: ResMut<ToastQueue>,
) {
    let primary = main.single().ok();
    state.closing |=
        exit.read().next().is_some() || close.read().any(|event| Some(event.window) == primary);
    if state.closing {
        state.pending.clear();
        return;
    }
    for GlobalTaskNotificationEvent(notification) in events.read() {
        if state.seen.insert((notification.task, notification.kind)) {
            state.pending.push_back(notification.clone());
        }
    }
}

fn show(
    mut commands: Commands,
    mut state: ResMut<ToastQueue>,
    main: Query<Option<&OnMonitor>, With<PrimaryWindow>>,
    monitors: Query<(&Monitor, Has<PrimaryMonitor>)>,
    windows: Query<(Entity, &Toast)>,
    scale: Res<WidgetryUiScale>,
    jobs: Res<JobManager>,
) -> Result {
    let main_monitor = main.single();
    if main_monitor.is_err() {
        state.closing = true;
        state.pending.clear();
    }
    let now = Instant::now();
    let mut occupied = [false; MAX_VISIBLE];
    for (entity, toast) in &windows {
        if state.closing || toast.deadline <= now {
            commands.entity(entity).try_despawn();
        } else {
            occupied[toast.slot] = true;
        }
    }
    let Ok(main_monitor) = main_monitor else {
        return Ok(());
    };
    let monitor = main_monitor
        .and_then(|on| monitors.get(on.0).ok().map(|(monitor, _)| monitor))
        .or_else(|| {
            monitors
                .iter()
                .find_map(|(monitor, primary)| primary.then_some(monitor))
        });
    let factor = scale.get();
    let capacity = monitor.map_or(1, |monitor| {
        (((monitor.physical_height as f32 / factor - BOTTOM_MARGIN) / (HEIGHT as f32 + GAP))
            as usize)
            .clamp(1, MAX_VISIBLE)
    });
    for (slot, used) in occupied.into_iter().enumerate().take(capacity) {
        if used {
            continue;
        }
        let Some(notification) = state.pending.pop_front() else {
            break;
        };
        let deadline = Instant::now() + LIFETIME;
        let (timer, _) = jobs
            .spawn_io::<(), _, _>(JobOptions::default(), move |context| async move {
                or(
                    Delay::new(deadline.saturating_duration_since(Instant::now())),
                    async {
                        context.cancelled().await;
                        context.acknowledge_cancel();
                    },
                )
                .await;
                Ok(())
            })
            .map_err(|failure| failure.into_bevy_error())?;
        let position = monitor.map_or(WindowPosition::Automatic, |monitor| {
            let x = monitor.physical_width as f32 - factor * (WIDTH as f32 + RIGHT_MARGIN);
            let y = monitor.physical_height as f32
                - factor * (BOTTOM_MARGIN + HEIGHT as f32 + slot as f32 * (HEIGHT as f32 + GAP));
            WindowPosition::At(
                monitor.physical_position + IVec2::new(x.max(0.0) as i32, y.max(0.0) as i32),
            )
        });
        let toast = Toast {
            slot,
            deadline,
            timer,
        };
        commands.spawn_scene_with_error_handler(scene(toast, position, notification));
    }
    Ok(())
}

fn cancel_timer(event: On<Despawn<Toast>>, toasts: Query<&Toast>, jobs: Res<JobManager>) {
    if let Ok(toast) = toasts.get(event.entity) {
        jobs.cancel_if_present(toast.timer);
    }
}

fn scene(toast: Toast, position: WindowPosition, notification: TaskNotification) -> impl Scene {
    let title = notification.session_title;
    let task = notification.name;
    let status = match notification.kind {
        TaskNotificationKind::Succeeded => "执行成功",
        TaskNotificationKind::Failed => "执行失败",
        TaskNotificationKind::NeedsConfirmation => "需要确认",
    };
    let name = format!("TaskToast-{}", notification.task.0);
    bsn! {
        Name(name) template(move |_| Ok(toast))
        @owned_widgetry_window(
            Window {
                title: "后台任务".into(),
                resolution: WindowResolution::new(WIDTH, HEIGHT),
                position,
                focused: false,
                resizable: false,
                skip_taskbar: true,
                enabled_buttons: EnabledButtons { minimize: false, maximize: false, close: true },
                window_level: WindowLevel::AlwaysOnTop,
                ..default()
            },
            WidgetryWindowControlsConfig {
                minimize_visible: false,
                maximize_visible: false,
                close_visible: true,
                resizable: false,
            },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::all(px(12)) }
                Children [
                    Node { overflow: Overflow::clip(), min_width: px(0) }
                    Children [ Text(title) WidgetryText TextLayout::no_wrap() ]--
                    Node { width: percent(100), align_items: AlignItems::Center, column_gap: px(10) }
                    Children [
                        Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }
                        Children [ Text(task) WidgetryText TextLayout::no_wrap() ]--
                        Text(status) WidgetryText TextLayout::no_wrap() Node { flex_shrink: 0.0 }
                    ]
                ]
            },
        )
    }
}
