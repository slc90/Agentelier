use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RequestRedraw, WindowCloseRequested};
use bevy::winit::{WINIT_WINDOWS, WinitWindows};
use bevy_widgetry::tray::{
    TrayMenuEntry, WidgetryTrayClick, WidgetryTrayMenu, WidgetryTrayMenuClick, WidgetryTrayPlugin,
    WidgetryTrayStatus,
};
use bevy_widgetry::window::{WidgetryModalWindow, taskbar_icon, widgetry_window_target};
use tracing::info;

const TRAY_ICON: &[u8] = include_bytes!("../../assets/src/assets/system-tray/tray-icon.ico");
const EXIT_ITEM: &str = "exit";

#[derive(Resource, Default)]
struct HiddenWindows(Option<Vec<HiddenWindow>>);

struct HiddenWindow {
    entity: Entity,
    minimized: Option<bool>,
}

pub(crate) fn install(app: &mut App) -> Result {
    let icons = taskbar_icon!("../assets/src/assets/system-tray/window-taskbar-icon.png")
        .map_err(|failure| failure.into_bevy_error())?;
    let tray = WidgetryTrayPlugin::new(TRAY_ICON, "Agentelier")
        .map_err(|failure| failure.into_bevy_error())?;
    let menu = WidgetryTrayMenu::new(vec![TrayMenuEntry::item(EXIT_ITEM, "退出 Agentelier")])
        .map_err(|failure| failure.into_bevy_error())?;
    app.init_resource::<HiddenWindows>()
        .insert_resource(menu)
        .add_plugins((icons, tray))
        .add_observer(hide_added_window)
        .add_observer(forget_removed_window)
        .add_systems(Update, request_exit)
        .add_systems(
            PostUpdate,
            (hide_on_primary_close, restore_from_tray).chain(),
        );
    Ok(())
}

fn request_exit(
    mut menu_clicks: MessageReader<WidgetryTrayMenuClick>,
    mut exits: MessageWriter<AppExit>,
) {
    if menu_clicks
        .read()
        .filter(|click| click.id == EXIT_ITEM)
        .count()
        > 0
    {
        info!("从系统托盘请求退出应用");
        exits.write(AppExit::Success);
    }
}

fn hide_on_primary_close(
    _main_thread: NonSendMarker,
    mut closes: MessageReader<WindowCloseRequested>,
    primary: Single<Entity, With<PrimaryWindow>>,
    status: Res<WidgetryTrayStatus>,
    mut hidden: ResMut<HiddenWindows>,
    mut windows: Query<(Entity, &mut Window)>,
) {
    if closes
        .read()
        .filter(|close| close.window == *primary)
        .count()
        == 0
    {
        return;
    }
    if *status != WidgetryTrayStatus::Ready {
        info!(status = ?*status, "托盘未就绪，主窗口保持可见");
        return;
    }
    if hidden.0.is_some() {
        return;
    }
    let snapshot = WINIT_WINDOWS.with_borrow(|native| {
        windows
            .iter_mut()
            .filter_map(|(entity, mut window)| {
                if !window.visible {
                    return None;
                }
                let minimized = native
                    .get_window(entity)
                    .and_then(|window| window.is_minimized());
                window.visible = false;
                Some(HiddenWindow { entity, minimized })
            })
            .collect::<Vec<_>>()
    });
    info!(windows = snapshot.len(), "已隐藏应用窗口");
    hidden.0 = Some(snapshot);
}

fn hide_added_window(
    added: On<Add<Window>>,
    mut hidden: ResMut<HiddenWindows>,
    mut windows: Query<&mut Window>,
) {
    let Some(snapshot) = &mut hidden.0 else {
        return;
    };
    if let Ok(mut window) = windows.get_mut(added.entity)
        && window.visible
    {
        // Winit reads Window when it handles the queued WindowAdded notification.
        window.visible = false;
        snapshot.push(HiddenWindow {
            entity: added.entity,
            minimized: None,
        });
    }
}

fn forget_removed_window(removed: On<Remove<Window>>, mut hidden: ResMut<HiddenWindows>) {
    if let Some(snapshot) = &mut hidden.0 {
        snapshot.retain(|window| window.entity != removed.entity);
    }
}

fn restore_from_tray(
    _main_thread: NonSendMarker,
    mut clicks: MessageReader<WidgetryTrayClick>,
    mut commands: Commands,
) {
    if clicks.read().count() > 0 {
        commands.queue(restore_hidden_windows);
    }
}

fn restore_hidden_windows(world: &mut World) {
    let Some(snapshot) = world.resource_mut::<HiddenWindows>().0.take() else {
        return;
    };
    let primary = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .ok();
    let focus = primary.filter(|&primary| {
        !world
            .query::<(Entity, &WidgetryModalWindow)>()
            .iter(world)
            .any(|(root, modal)| {
                modal.parent == primary
                    && widgetry_window_target(world, root)
                        .is_some_and(|window| world.get::<Window>(window).is_some())
            })
    });
    let restored = WINIT_WINDOWS.with_borrow(|native| {
        let restored = restore_windows(world, native, snapshot);
        if let Some(window) = focus
            .filter(|&primary| {
                world
                    .get::<Window>(primary)
                    .is_some_and(|window| window.visible)
            })
            .and_then(|primary| native.get_window(primary))
        {
            window.focus_window();
        }
        restored
    });
    world.write_message(RequestRedraw);
    info!(windows = restored, "已从系统托盘恢复应用窗口");
}

fn restore_windows(world: &mut World, native: &WinitWindows, snapshot: Vec<HiddenWindow>) -> usize {
    let mut restored = 0;
    for HiddenWindow { entity, minimized } in snapshot {
        let Some(mut window) = world.get_mut::<Window>(entity) else {
            continue;
        };
        window.visible = true;
        if let Some(native) = native.get_window(entity) {
            // Bevy applies focus before visibility. Show the window before requesting focus.
            native.set_visible(true);
            if let Some(minimized) = minimized {
                native.set_minimized(minimized);
            }
        }
        restored += 1;
    }
    restored
}
