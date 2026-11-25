use bevy::prelude::*;
use bevy::ui::{ScrollPosition, UiSystems};
use bevy::window::RequestRedraw;
use bevy_widgetry::scroll_area::WidgetryScrollAreaViewport;
const BOTTOM_TOLERANCE: f32 = 3.0;
#[derive(Component, Clone, Default)]
pub(crate) struct ChatReadingScroll;
#[derive(Component, Clone, Default)]
pub(super) struct MemoryReadingScroll;
#[derive(Resource, Default)]
struct MemoryReading(ChatReading);
#[derive(Resource, Reflect, Clone)]
#[reflect(Resource)]
struct ChatReading {
    following: bool,
    position: f32,
    maximum: f32,
    viewport: Option<Entity>,
    restore: bool,
}
impl Default for ChatReading {
    fn default() -> Self {
        Self {
            following: true,
            position: 0.0,
            maximum: 0.0,
            viewport: None,
            restore: false,
        }
    }
}
pub(crate) fn install(app: &mut App) {
    app.init_resource::<ReadingStates>()
        .init_resource::<ChatReading>()
        .init_resource::<MemoryReading>()
        .register_type::<ChatReading>()
        .add_systems(
            PostUpdate,
            (observe, observe_memory).before(UiSystems::Content),
        )
        .add_systems(
            PostUpdate,
            (follow, follow_memory)
                .after(UiSystems::Layout)
                .before(UiSystems::PostLayout),
        );
}
#[derive(Resource, Default)]
struct ReadingStates(
    std::collections::HashMap<agentelier_conversation_bridge::SessionId, ChatReading>,
);

pub(crate) fn switch(
    world: &mut World,
    previous: Option<agentelier_conversation_bridge::SessionId>,
    next: agentelier_conversation_bridge::SessionId,
) {
    let old = world.resource::<ChatReading>().clone();
    let mut restored = {
        let mut states = world.resource_mut::<ReadingStates>();
        if let Some(previous) = previous {
            states.0.insert(previous, old);
        }
        states.0.remove(&next).unwrap_or_default()
    };
    restored.viewport = None;
    restored.restore = true;
    *world.resource_mut::<ChatReading>() = restored;
}

fn observe(
    mut reading: ResMut<ChatReading>,
    roots: Query<&Children, With<ChatReadingScroll>>,
    viewports: Query<(Entity, &ScrollPosition), With<WidgetryScrollAreaViewport>>,
) {
    observe_viewports(&mut reading, roots.iter(), &viewports);
}

fn observe_memory(
    mut reading: ResMut<MemoryReading>,
    roots: Query<&Children, With<MemoryReadingScroll>>,
    viewports: Query<(Entity, &ScrollPosition), With<WidgetryScrollAreaViewport>>,
) {
    observe_viewports(&mut reading.0, roots.iter(), &viewports);
}

fn observe_viewports<'a>(
    reading: &mut ChatReading,
    roots: impl Iterator<Item = &'a Children>,
    viewports: &Query<(Entity, &ScrollPosition), With<WidgetryScrollAreaViewport>>,
) {
    for children in roots {
        for child in children {
            let Ok((entity, position)) = viewports.get(*child) else {
                continue;
            };
            if reading.viewport != Some(entity) {
                reading.viewport = Some(entity);
                reading.restore = true;
                continue;
            }
            if reading.restore {
                continue;
            }
            if (position.0.y - reading.position).abs() > BOTTOM_TOLERANCE {
                reading.following = position.0.y >= reading.maximum - BOTTOM_TOLERANCE;
            }
        }
    }
}
fn follow(
    mut reading: ResMut<ChatReading>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<WidgetryScrollAreaViewport>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    follow_viewport(&mut reading, &mut viewports, &mut redraw);
}

fn follow_memory(
    mut reading: ResMut<MemoryReading>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<WidgetryScrollAreaViewport>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    follow_viewport(&mut reading.0, &mut viewports, &mut redraw);
}

fn follow_viewport(
    reading: &mut ChatReading,
    viewports: &mut Query<(&ComputedNode, &mut ScrollPosition), With<WidgetryScrollAreaViewport>>,
    redraw: &mut MessageWriter<RequestRedraw>,
) {
    let Some(entity) = reading.viewport else {
        return;
    };
    let Ok((node, mut position)) = viewports.get_mut(entity) else {
        return;
    };
    let maximum = ((node.content_size.y - node.size.y) * node.inverse_scale_factor).max(0.0);
    if !reading.restore && (position.0.y - reading.position).abs() > BOTTOM_TOLERANCE {
        reading.following = position.0.y >= reading.maximum - BOTTOM_TOLERANCE;
    }
    let target = if reading.following {
        maximum
    } else if reading.restore {
        reading.position.min(maximum)
    } else {
        position.0.y.min(maximum).max(0.0)
    };
    if (position.0.y - target).abs() > 0.1 {
        position.0.y = target;
        redraw.write(RequestRedraw);
    }
    reading.position = target;
    reading.maximum = maximum;
    reading.restore = false;
}

pub(crate) fn forget(
    world: &mut World,
    session: agentelier_conversation_bridge::SessionId,
    current: bool,
) {
    world.resource_mut::<ReadingStates>().0.remove(&session);
    if current {
        *world.resource_mut::<ChatReading>() = default();
    }
}
