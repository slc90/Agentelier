use crate::workspace::{
    CHAT_MIN, HANDLE_WIDTH, RIGHT_INITIAL, RIGHT_MIN, SESSION_INITIAL, SESSION_MIN,
    WORKSPACE_CONTENT_WIDTH,
};
use bevy::picking::events::PointerCancel;
use bevy::prelude::*;
use bevy::ui::{Pressed, UiSystems};
use bevy::ui_widgets::ValueChange;
use bevy::window::RequestRedraw;
use bevy_widgetry::splitter::{SplitterError, SplitterLayoutError, WidgetrySplitter};
use serde::{Deserialize, Serialize};

const MIN_CONTENT_WIDTH: f32 = SESSION_MIN + CHAT_MIN + RIGHT_MIN;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRatios {
    pub session: f32,
    pub right: f32,
}

impl Default for WorkspaceRatios {
    fn default() -> Self {
        Self {
            session: SESSION_INITIAL / WORKSPACE_CONTENT_WIDTH,
            right: RIGHT_INITIAL / WORKSPACE_CONTENT_WIDTH,
        }
    }
}

impl WorkspaceRatios {
    pub fn is_valid(self) -> bool {
        self.session.is_finite()
            && self.right.is_finite()
            && self.session > 0.0
            && self.right > 0.0
            && self.session + self.right < 1.0
    }

    fn widths(self, total: f32) -> Widths {
        let extra_session = (total * self.session - SESSION_MIN).max(0.0);
        let extra_right = (total * self.right - RIGHT_MIN).max(0.0);
        let extra_chat = (total * (1.0 - self.session - self.right) - CHAT_MIN).max(0.0);
        let wanted = extra_session + extra_right + extra_chat;
        let factor = if wanted > 0.0 {
            (total - MIN_CONTENT_WIDTH) / wanted
        } else {
            0.0
        };
        Widths {
            session: SESSION_MIN + extra_session * factor,
            right: RIGHT_MIN + extra_right * factor,
            total,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Component)]
pub(crate) enum Split {
    Session,
    Chat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutSide {
    Session,
    Right,
}

#[derive(Event)]
pub struct CommitWorkspaceLayout;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct WorkspaceLayout {
    pub ratios: WorkspaceRatios,
    pub committed: WorkspaceRatios,
    pub error: Option<String>,
    width: f32,
    #[reflect(ignore)]
    gesture: Option<Gesture>,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self::new(WorkspaceRatios::default())
    }
}

impl WorkspaceLayout {
    pub fn new(ratios: WorkspaceRatios) -> Self {
        Self {
            ratios,
            committed: ratios,
            error: None,
            width: WORKSPACE_CONTENT_WIDTH,
            gesture: None,
        }
    }

    pub fn adjust(&mut self, side: LayoutSide, value: f32) {
        if !value.is_finite() || self.gesture.is_some() {
            return;
        }
        let current = self.ratios.widths(self.width);
        let mut updated = current;
        match side {
            LayoutSide::Session => {
                updated.session = (self.width * value).clamp(
                    SESSION_MIN,
                    (self.width - CHAT_MIN - current.right).max(SESSION_MIN),
                );
            }
            LayoutSide::Right => {
                updated.right = (self.width * value).clamp(
                    RIGHT_MIN,
                    (self.width - CHAT_MIN - current.session).max(RIGHT_MIN),
                );
            }
        }
        if (current.session - updated.session).abs() > 0.1
            || (current.right - updated.right).abs() > 0.1
        {
            self.ratios = updated.ratios();
            self.error = None;
        }
    }
}

#[derive(Clone, Copy)]
struct Gesture {
    side: Split,
    previous: WorkspaceRatios,
}

#[derive(Clone, Copy)]
struct Widths {
    session: f32,
    right: f32,
    total: f32,
}

impl Widths {
    fn ratios(self) -> WorkspaceRatios {
        WorkspaceRatios {
            session: self.session / self.total,
            right: self.right / self.total,
        }
    }

    fn widget_ratio(self, split: Split) -> f32 {
        match split {
            Split::Session => self.session / (self.total + HANDLE_WIDTH),
            Split::Chat => 1.0 - self.right / (self.total - self.session),
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<WorkspaceLayout>()
        .register_type::<WorkspaceLayout>()
        .add_systems(Update, finish_gesture)
        .add_systems(PostUpdate, sync.after(UiSystems::Layout));
}

pub(crate) fn initial_ratio(layout: WorkspaceRatios, split: Split) -> f32 {
    layout.widths(WORKSPACE_CONTENT_WIDTH).widget_ratio(split)
}

pub(crate) fn changed(
    event: On<ValueChange<f32>>,
    splits: Query<&Split>,
    mut layout: ResMut<WorkspaceLayout>,
) {
    if event.is_final {
        return;
    }
    let Ok(&split) = splits.get(event.source) else {
        return;
    };
    if layout.gesture.is_some_and(|gesture| gesture.side != split) {
        return;
    }
    let width = layout.width;
    let mut current = layout.ratios.widths(width);
    let previous = layout.ratios;
    layout.gesture.get_or_insert(Gesture {
        side: split,
        previous,
    });
    match split {
        Split::Session => {
            current.session = (event.value * (width + HANDLE_WIDTH))
                .clamp(SESSION_MIN, (width - CHAT_MIN - RIGHT_MIN).max(SESSION_MIN));
            current.right = current
                .right
                .min((width - CHAT_MIN - current.session).max(RIGHT_MIN));
        }
        Split::Chat => {
            current.right = ((1.0 - event.value) * (width - current.session)).clamp(
                RIGHT_MIN,
                (width - CHAT_MIN - current.session).max(RIGHT_MIN),
            );
        }
    }
    layout.ratios = current.ratios();
    layout.error = None;
}

fn finish_gesture(
    mut cancel: MessageReader<PointerCancel>,
    parents: Query<&ChildOf>,
    splits: Query<&Split>,
    held: Query<&ChildOf, With<Pressed>>,
    mut layout: ResMut<WorkspaceLayout>,
    mut commands: Commands,
) {
    let cancelled = cancel
        .read()
        .filter_map(|event| parents.get(event.entity).ok())
        .filter_map(|parent| splits.get(parent.parent()).ok())
        .copied()
        .collect::<Vec<_>>();
    let Some(gesture) = layout.gesture else {
        return;
    };
    if cancelled.contains(&gesture.side) {
        layout.ratios = gesture.previous;
        layout.gesture = None;
    } else if !held
        .iter()
        .any(|parent| splits.get(parent.parent()) == Ok(&gesture.side))
    {
        layout.gesture = None;
        commands.trigger(CommitWorkspaceLayout);
    }
}

fn sync(world: &mut World, mut prepared: Local<Option<(Entity, u8)>>) -> Result {
    let nodes = world
        .query::<(Entity, &Split, &ComputedNode)>()
        .iter(world)
        .map(|(entity, split, node)| {
            (
                entity,
                *split,
                node.unrounded_size.x * node.inverse_scale_factor(),
            )
        })
        .collect::<Vec<_>>();
    let Some(&(root, _, outer)) = nodes.iter().find(|(_, split, _)| *split == Split::Session)
    else {
        *prepared = None;
        return Ok(());
    };
    if outer + 0.5 < MIN_CONTENT_WIDTH + 2.0 * HANDLE_WIDTH {
        return Ok(());
    }
    let frames = prepared.get_or_insert((root, 0));
    if frames.0 != root {
        *frames = (root, 0);
    }
    // Widgetry reads the previous geometry, establishes pane bounds, then initializes the ratio.
    if frames.1 < 2 {
        frames.1 += 1;
        world.write_message(RequestRedraw);
        return Ok(());
    }
    let total = (outer - 2.0 * HANDLE_WIDTH).max(MIN_CONTENT_WIDTH);
    let (widths, gesture) = {
        let mut layout = world.resource_mut::<WorkspaceLayout>();
        if (layout.width - total).abs() > 0.01 {
            layout.width = total;
        }
        (layout.ratios.widths(total), layout.gesture)
    };
    for (entity, split, current_width) in nodes {
        if gesture.is_some_and(|active| active.side == split) || current_width <= HANDLE_WIDTH {
            continue;
        }
        let desired = widths.widget_ratio(split);
        let current = WidgetrySplitter::split_ratio(world, entity)
            .map_err(|failure| failure.into_bevy_error())?;
        if (current - desired).abs() > 0.0001
            && let Err(failure) = WidgetrySplitter::set_split_ratio_in_world(world, entity, desired)
        {
            if matches!(failure.kind(), SplitterError::Layout { source, .. }
                if matches!(source.as_ref(), SplitterLayoutError::LayoutUnavailable))
            {
                world.write_message(RequestRedraw);
            } else {
                return Err(failure.into_bevy_error());
            }
        }
    }
    Ok(())
}
