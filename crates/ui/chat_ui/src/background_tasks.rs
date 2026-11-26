use crate::conversation::ChatState;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{BackgroundStatus, BackgroundTask, SessionId};
use bevy::picking::events::PointerDrag;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;

const WIDTH: f32 = 272.0;
const GAP: f32 = 8.0;
const MARGIN: f32 = 12.0;
const LIST_HEIGHT: f32 = 164.0;
const MAX_DETAIL_CHARS: usize = 160;

#[derive(Resource, Default)]
pub(crate) struct Tasks {
    pub(crate) items: Vec<BackgroundTask>,
}

impl Tasks {
    pub(crate) fn count(&self, source: SessionId) -> usize {
        self.items
            .iter()
            .filter(|task| task.source == source)
            .count()
    }
}

#[derive(Resource, Default)]
struct View {
    expanded: bool,
    position: Option<Vec2>,
}

#[derive(Component, Default, Clone)]
pub(crate) struct Bounds;
#[derive(Component, Default, Clone)]
struct Overlay;
#[derive(Component, Default, Clone)]
struct Summary;
#[derive(Component, Default, Clone)]
struct ToggleLabel;
#[derive(Component, Default, Clone)]
struct Viewport;
#[derive(Component, Default, Clone)]
struct Rows {
    rendered: Vec<BackgroundTask>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Tasks>()
        .init_resource::<View>()
        .add_systems(PostUpdate, synchronize.before(UiSystems::Content));
}

pub(crate) fn panel() -> impl Scene {
    bsn! {
        Name("BackgroundTaskOverlay") Overlay
        ZIndex(10) BackgroundColor({Color::srgb_u8(242, 248, 241)})
        BorderColor::all({Color::srgb_u8(184, 210, 195)})
        BoxShadow(vec![ShadowStyle { color: Color::srgba(0.11, 0.24, 0.18, 0.13), x_offset: px(0), y_offset: px(4), spread_radius: px(0), blur_radius: px(12) }])
        Node {
            display: Display::None, position_type: PositionType::Absolute, top: px(MARGIN), right: px(MARGIN),
            width: px(WIDTH), max_width: percent(100), padding: UiRect::all(px(GAP)), border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(8)), flex_direction: FlexDirection::Column, row_gap: px(GAP)
        }
        Children [
            Name("BackgroundTaskHeader")
            Node { width: percent(100), align_items: AlignItems::Center, column_gap: px(GAP) }
            Children [
                Name("BackgroundTaskDragHandle") Node { flex_grow: 1.0, padding: UiRect::all(px(4)) }
                on(drag)
                Children [
                    Name("BackgroundTaskSummary") Summary Text("后台任务") TextColor({Color::srgb_u8(35, 94, 71)})
                    WidgetryText Pickable::IGNORE
                ]--
                @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text("展开") WidgetryText ToggleLabel }) as Box<dyn SceneList>)} }
                Name("ToggleBackgroundTaskOverlay") on(toggle)
            ]--
            @WidgetryScrollArea { @children: {Some(Box::new(bsn_list! {
                Name("BackgroundTaskRows") Rows::default()
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
            }) as Box<dyn SceneList>)} }
            Name("BackgroundTaskViewport") Viewport
            Node { display: Display::None, width: percent(100), height: px(LIST_HEIGHT), min_height: px(0) }
        ]
    }
}

fn toggle(_: On<Activate>, mut view: ResMut<View>) {
    view.expanded = !view.expanded;
}

fn drag(
    event: On<PointerDrag>,
    bounds: Query<&ComputedNode, With<Bounds>>,
    overlay: Query<(&ComputedNode, &Node), With<Overlay>>,
    mut view: ResMut<View>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let (Ok(bounds), Ok((computed, node))) = (bounds.single(), overlay.single()) else {
        return;
    };
    let available = (logical_size(bounds) - logical_size(computed)).max(Vec2::ZERO);
    let initial = view.position.unwrap_or_else(|| {
        Vec2::new(
            if let Val::Px(left) = node.left {
                left
            } else {
                available.x - MARGIN
            },
            if let Val::Px(top) = node.top {
                top
            } else {
                MARGIN
            },
        )
    });
    view.position = Some((initial + event.delta).clamp(Vec2::ZERO, available));
}

fn logical_size(node: &ComputedNode) -> Vec2 {
    node.size() * node.inverse_scale_factor()
}

fn synchronize(world: &mut World) {
    let current = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|session| session.id)
        .filter(|session| {
            !world
                .resource::<crate::ArchiveState>()
                .entries
                .iter()
                .any(|entry| entry.id == *session && entry.archived)
        });
    let visible = world
        .resource::<Tasks>()
        .items
        .iter()
        .filter(|task| {
            Some(task.source) == current
                && !matches!(
                    task.status,
                    BackgroundStatus::Succeeded | BackgroundStatus::Stopped
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let View { expanded, position } = *world.resource::<View>();
    let size = world
        .query_filtered::<&ComputedNode, With<Bounds>>()
        .single(world)
        .map_or(Vec2::ZERO, logical_size);
    for (mut node, computed) in world
        .query_filtered::<(&mut Node, &ComputedNode), With<Overlay>>()
        .iter_mut(world)
    {
        let display = if visible.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != display {
            node.display = display;
        }
        if let Some(position) = position {
            let position =
                position.clamp(Vec2::ZERO, (size - logical_size(computed)).max(Vec2::ZERO));
            if node.left != px(position.x) || node.top != px(position.y) {
                node.left = px(position.x);
                node.top = px(position.y);
                node.right = Val::Auto;
            }
        }
    }
    for mut viewport in world
        .query_filtered::<&mut Node, With<Viewport>>()
        .iter_mut(world)
    {
        let display = if expanded {
            Display::Flex
        } else {
            Display::None
        };
        if viewport.display != display {
            viewport.display = display;
        }
    }
    for mut text in world
        .query_filtered::<&mut Text, With<Summary>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text::new(format!("后台任务 · {}", visible.len())));
    }
    for mut text in world
        .query_filtered::<&mut Text, With<ToggleLabel>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text::new(if expanded { "折叠" } else { "展开" }));
    }
    let mut changed = Vec::new();
    for (entity, mut rows, children) in world
        .query::<(Entity, &mut Rows, Option<&Children>)>()
        .iter_mut(world)
    {
        if rows.rendered != visible {
            rows.rendered.clone_from(&visible);
            changed.push((
                entity,
                children.map(|children| children.iter().collect::<Vec<_>>()),
            ));
        }
    }
    for (entity, children) in changed {
        let mut commands = world.commands();
        for child in children.into_iter().flatten() {
            commands.entity(child).try_despawn();
        }
        for task in &visible {
            commands.spawn_scene_with_error_handler(bsn! { @row(task) ChildOf(entity) });
        }
    }
}

fn row(task: &BackgroundTask) -> impl Scene {
    let name = format!("BackgroundTask-{}", task.session);
    let status = match task.status {
        BackgroundStatus::Pending => "未执行",
        BackgroundStatus::Running => "执行中",
        BackgroundStatus::NeedsInput => "待处理",
        BackgroundStatus::Failed => "失败",
        BackgroundStatus::Interrupted => "上次中断",
        BackgroundStatus::Unconfirmed => "结果未确认",
        BackgroundStatus::Succeeded => "已完成",
        BackgroundStatus::Stopped => "已停止",
    };
    let title = format!("{} · {status}", task.title);
    let mut chars = task.detail.as_deref().unwrap_or_default().chars();
    let mut detail: String = chars.by_ref().take(MAX_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        detail.push('…');
    }
    let detail_display = if detail.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    bsn! {
        Name(name)
        Node { min_width: px(0), flex_direction: FlexDirection::Column }
        Children [
            Text(title) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) Pickable::IGNORE--
            Text(detail) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) Pickable::IGNORE
            Node { display: detail_display }
        ]
    }
}
