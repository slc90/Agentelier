use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::window::{CursorSystems, RequestRedraw, WindowCloseRequested, WindowClosing};
use bevy_widgetry::window::widgetry_window_target;

#[derive(Component)]
struct OwnedWindowClosing;

pub fn install_owned_window_close<F: QueryFilter + 'static>(app: &mut App) {
    app.add_systems(Last, close_owned_windows::<F>.after(CursorSystems::Update));
}

fn close_owned_windows<F: QueryFilter + 'static>(
    mut requests: MessageReader<WindowCloseRequested>,
    roots: Query<(Entity, Has<OwnedWindowClosing>), F>,
    mut commands: Commands,
) {
    let requested: EntityHashSet = requests.read().map(|request| request.window).collect();
    let mut candidates = Vec::new();
    for (root, closing) in &roots {
        if closing {
            commands.entity(root).try_despawn();
        } else if !requested.is_empty() {
            candidates.push(root);
        }
    }
    if candidates.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for root in candidates {
            if let Some(window) = widgetry_window_target(world, root)
                && requested.contains(&window)
                && world.get::<Window>(window).is_some()
            {
                // Give render extraction a frame to release the DX12 swap chain.
                world.write_message(WindowClosing { window });
                world.entity_mut(root).insert(OwnedWindowClosing);
                world.write_message(RequestRedraw);
            }
        }
    });
}
