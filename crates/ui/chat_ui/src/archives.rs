use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostActivity};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::ProjectId;
use agentelier_conversation_bridge::{ArchiveEntry, ConversationRequest, SessionId};
use bevy::picking::{events::PointerClick, hover::HoverMap};
use bevy::prelude::*;
use bevy::ui::UiTransform;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy_app_assets::SidebarIcon;
use bevy_widgetry::icon::{
    WidgetryIcon, WidgetryIconColorOverrides, WidgetryIconStateColorOverrides,
};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{TooltipContentFactory, WidgetryTooltip, WidgetryTooltipPlugin};
use bevy_widgetry::tree::{
    WidgetryTreeAppExt, WidgetryTreeEvent, WidgetryTreeEventKind, WidgetryTreeModel,
    WidgetryTreeNode, WidgetryTreeRenderer, WidgetryTreeView,
};
use std::collections::HashMap;

const ROW_HEIGHT: f32 = 44.0;
const TITLE_PREVIEW_CHARACTERS: usize = 18;
const CONTROL_GAP: f32 = 12.0;

#[derive(Clone, Reflect, PartialEq)]
pub struct ArchiveChoice {
    pub id: SessionId,
    pub archived: bool,
    pub title: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_sequence: u64,
    pub error: Option<String>,
}

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct ArchiveState {
    pub entries: Vec<ArchiveChoice>,
    pub selected: Option<SessionId>,
    pub loading: bool,
    pub refreshing: bool,
    pub error: Option<String>,
    pub opening: Option<(u64, SessionId)>,
    deleted: Vec<SessionId>,
    pub cleanup_errors: Vec<String>,
}

impl Default for ArchiveState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            loading: true,
            refreshing: true,
            error: None,
            opening: None,
            deleted: Vec::new(),
            cleanup_errors: Vec::new(),
        }
    }
}

#[derive(Resource)]
struct Navigator {
    project_source: Entity,
    standalone_source: Entity,
    projects: Entity,
    standalone: Entity,
    project_nodes: HashMap<ProjectId, Entity>,
    session_nodes: HashMap<SessionId, Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Project(ProjectId),
    Session(SessionId),
}

#[derive(Component, PartialEq, Eq)]
struct NavigatorEntry {
    target: Target,
    title: String,
    error: Option<String>,
}
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ArchiveSelector {
    Projects,
    Standalone,
}

#[derive(Component, Clone, Default)]
struct RevealOnHover;

#[derive(Component, Clone, Copy)]
struct SessionIndicator(SessionId);

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SessionActivity {
    #[default]
    Idle,
    Busy,
    Error,
}

#[derive(Resource, Default)]
struct SessionActivities(HashMap<SessionId, SessionActivity>);
#[derive(Component, Clone, Default)]
struct ArchiveStatus;

pub(crate) fn install(app: &mut App) -> Result {
    if !app.is_plugin_added::<WidgetryTooltipPlugin>() {
        app.add_plugins(WidgetryTooltipPlugin);
    }
    app.register_renderer::<NavigatorEntry>(WidgetryTreeRenderer::new(
        |_, entry: &NavigatorEntry| render(entry),
    ))
    .map_err(|failure| failure.into_bevy_error())?;
    crate::session_actions::install(app);
    app.init_resource::<ArchiveState>()
        .register_type::<ArchiveState>()
        .init_resource::<SessionActivities>()
        .add_systems(Startup, initialize)
        .add_systems(
            PostUpdate,
            (update, reveal_on_hover, update_indicators).before(UiSystems::Content),
        );
    Ok(())
}

fn initialize(mut commands: Commands) {
    let projects = commands.spawn_empty().id();
    let standalone = commands.spawn_empty().id();
    let project_source = commands.spawn(WidgetryTreeModel::new(projects)).id();
    let standalone_source = commands.spawn(WidgetryTreeModel::new(standalone)).id();
    commands.insert_resource(Navigator {
        project_source,
        standalone_source,
        projects,
        standalone,
        project_nodes: HashMap::new(),
        session_nodes: HashMap::new(),
    });
}

fn render(entry: &NavigatorEntry) -> impl SceneList + use<> {
    let full = match &entry.error {
        Some(error) => format!("{}\n{error}", entry.title),
        None => entry.title.clone(),
    };
    let mut preview = entry
        .title
        .chars()
        .take(TITLE_PREVIEW_CHARACTERS)
        .collect::<String>();
    if entry.title.chars().count() > TITLE_PREVIEW_CHARACTERS {
        preview.push('…');
    }
    let (name, session) = match entry.target {
        Target::Project(id) => (format!("NavigatorProject-{id}"), None),
        Target::Session(id) => (format!("NavigatorSession-{id}"), Some(id)),
    };
    let actions = session.map(|id| {
        bsn_list! {
            @indicator(id)--
            @crate::session_actions::more(id)
        }
    });
    bsn_list! {
        @WidgetryTooltip { @content: {TooltipContentFactory::new(move || { let title = full.clone(); bsn_list! { Text(title) WidgetryText } })} }
        Name(name)
        Node { width: percent(100), height: percent(100), min_width: px(0), align_items: AlignItems::Center, column_gap: px(6) }
        Children [
            Text(preview) WidgetryText TextLayout::no_wrap() Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }--
            {actions}
        ]
    }
}

pub(crate) fn panel(world: &World) -> impl Scene + use<> {
    let nav = world.resource::<Navigator>();
    let directory = world.resource::<crate::working_directory::DirectoryState>();
    bsn! {
        Name("LocalSessionArchives") Node { width: percent(100), min_width: px(0), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(CONTROL_GAP), flex_shrink: 0.0 }
        Children [
            @section("项目", "NavigatorProjects", nav.project_source, ArchiveSelector::Projects, crate::projects::create_button())--
            @crate::projects::panel(world)--
            @section("单独聊天", "NavigatorStandalone", nav.standalone_source, ArchiveSelector::Standalone, crate::conversation::standalone_button())--
            @bevy_widgetry::collapsible::WidgetryCollapsible {
                @expanded: false,
                @header: bsn! { Text("新会话工作目录") WidgetryText },
                @children: bsn_list! { @crate::working_directory::panel(directory) },
            } Node { width: percent(100), flex_shrink: 0.0 }--
            @crate::session_actions::panel()--
            Text("") WidgetryText ArchiveStatus Name("SessionArchiveStatus")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
    }
}

fn section(
    title: &'static str,
    name: &'static str,
    source: Entity,
    section: ArchiveSelector,
    create: impl Scene,
) -> impl Scene {
    bsn! {
        Name(name)
        Node { width: percent(100), min_height: px(0), flex_basis: px(0), flex_grow: 1.0, flex_direction: FlexDirection::Column }
        Children [
            Node {
                width: percent(100), height: px(40), padding: UiRect::horizontal(px(8)),
                align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, flex_shrink: 0.0,
            }
            Children [
                Text(title) WidgetryText Pickable::IGNORE--
                @create
            ]--
            @WidgetryTreeView { @source: source, @item_height: ROW_HEIGHT }
            template(move |_| Ok(section))
            Node { width: percent(100), flex_basis: px(0), min_height: px(0), flex_grow: 1.0 }
            on(select)
        ]
    }
}

pub(crate) fn reveal_button(icon: SidebarIcon) -> impl Scene {
    let icon_size = 12;
    bsn! {
        @AgentelierButton {
            @style: AgentelierButtonStyle::Icon,
            @children: bsn_list! {
                @WidgetryIcon { @path: {icon.path()}, @max_size: {Some(UVec2::splat(icon_size))} }
                Pickable::IGNORE
            },
        }
        RevealOnHover Visibility::Hidden Node { min_height: px(28), height: px(28) }
    }
}

pub(crate) fn block_row_click(mut event: On<PointerClick>) {
    event.propagate(false);
}

fn reveal_on_hover(
    hover: Res<HoverMap>,
    parents: Query<&ChildOf>,
    mut buttons: Query<(&ChildOf, Option<&Children>, &mut Visibility), With<RevealOnHover>>,
    menus: Query<(), With<crate::session_actions::SessionMenu>>,
) {
    let mut ancestors = bevy::ecs::entity::EntityHashSet::default();
    for hits in hover.values() {
        for entity in hits.keys().copied() {
            ancestors.insert(entity);
            ancestors.extend(parents.iter_ancestors(entity));
        }
    }
    for (parent, children, mut visibility) in &mut buttons {
        visibility.set_if_neq(
            if ancestors.contains(&parent.parent())
                || children
                    .is_some_and(|children| children.iter().any(|child| menus.contains(child)))
            {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        );
    }
}

fn indicator(session: SessionId) -> impl Scene {
    let icon_size = 16;
    let size = icon_size as f32;
    let indicator_color = Color::srgb_u8(0x25, 0x66, 0x4C);
    let error_color = WidgetryIconStateColorOverrides {
        foreground: Some(Color::srgb_u8(0xB9, 0x19, 0x2E)),
    };
    bsn! {
        Name({format!("SessionActivity-{session}")})
        template(move |_| Ok(SessionIndicator(session)))
        Visibility::Hidden Pickable::IGNORE
        Node { width: px(size), height: px(size), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0 }
        Children [
            Name("SessionBusy") Pickable::IGNORE
            Node { display: Display::None, width: px(size), height: px(size), border: UiRect::all(px(2)), border_radius: BorderRadius::all(percent(50)) }
            BorderColor { top: indicator_color, left: indicator_color, right: Color::NONE, bottom: Color::NONE }--
            @WidgetryIcon {
                @path: {SidebarIcon::SessionError.path()},
                @max_size: {Some(UVec2::splat(icon_size))},
                @colors: {WidgetryIconColorOverrides { normal: error_color.clone(), disabled: error_color }},
            }
            Name("SessionError") Pickable::IGNORE Node { display: Display::None }
        ]
    }
}

fn update_indicators(
    chat: Res<ChatState>,
    archives: Res<ArchiveState>,
    tasks: Res<crate::global_tasks::GlobalTaskState>,
    time: Res<Time>,
    mut activities: ResMut<SessionActivities>,
    mut indicators: Query<(&SessionIndicator, &Children, &mut Visibility)>,
    mut parts: Query<(&mut Node, &mut UiTransform)>,
) {
    if chat.is_changed()
        && let Some(session) = &chat.session
    {
        use crate::conversation::ChatTurnStatus;
        let activity = if chat.error.is_some()
            || chat.model_error.is_some()
            || session.execution_blocked
            || session.turn.as_ref().is_some_and(|turn| {
                matches!(
                    turn.status,
                    ChatTurnStatus::Failed(_) | ChatTurnStatus::Limited
                ) || turn.tools.iter().any(|tool| {
                    matches!(
                        tool.status.as_str(),
                        "failed" | "timed_out" | "unconfirmed" | "external_unconfirmed"
                    )
                })
            }) {
            SessionActivity::Error
        } else if chat.operation.is_some() && archives.opening.is_none()
            || session
                .turn
                .as_ref()
                .is_some_and(|turn| matches!(turn.status, ChatTurnStatus::Running))
            || !session.queue_paused && !session.queue.is_empty()
        {
            SessionActivity::Busy
        } else {
            SessionActivity::Idle
        };
        activities.0.insert(session.id, activity);
    }
    let rotation = Rot2::turn_fraction(time.elapsed_secs().fract());
    for (indicator, children, mut visibility) in &mut indicators {
        let id = indicator.0;
        let projected = activities.0.get(&id).copied().unwrap_or_default();
        let catalog_error = archives
            .entries
            .iter()
            .any(|entry| entry.id == id && entry.error.is_some());
        let activity = if catalog_error {
            SessionActivity::Error
        } else {
            projected.max(tasks.activity(id))
        };
        visibility.set_if_neq(if activity == SessionActivity::Idle {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
        for (child, state) in children
            .iter()
            .zip([SessionActivity::Busy, SessionActivity::Error])
        {
            if let Ok((node, mut transform)) = parts.get_mut(child) {
                let display = if activity == state {
                    Display::Flex
                } else {
                    Display::None
                };
                node.map_unchanged(|node| &mut node.display)
                    .set_if_neq(display);
                if activity == SessionActivity::Busy && state == SessionActivity::Busy {
                    transform.rotation = rotation;
                }
            }
        }
    }
}

fn select(event: On<WidgetryTreeEvent>, entries: Query<&NavigatorEntry>, mut commands: Commands) {
    let WidgetryTreeEventKind::SelectionChanged { selected, .. } = &event.kind else {
        return;
    };
    let Some(target) = selected
        .iter()
        .find_map(|entity| entries.get(*entity).ok())
        .map(|entry| entry.target)
    else {
        return;
    };
    commands.queue(move |world: &mut World| -> Result {
        let session = match target {
            Target::Project(project) => {
                return crate::projects::select_project(world, Some(project));
            }
            Target::Session(session) => session,
        };
        if world.resource::<ArchiveState>().selected == Some(session) {
            return Ok(());
        }
        sync_selection(world)?;
        if world.resource::<ChatState>().operation.is_some()
            || world.resource::<ArchiveState>().loading
            || world.resource::<AgentHost>().model_busy()?
        {
            world.resource_mut::<ArchiveState>().error =
                Some("当前任务结束并回收后可切换，请等待或停止当前任务".into());
            return Ok(());
        }
        match world
            .resource::<AgentHost>()
            .request_conversation(ConversationRequest::Open { session })
        {
            Ok(id) => {
                let mut chat = world.resource_mut::<ChatState>();
                chat.operation = Some(id);
                chat.error = None;
                let mut archives = world.resource_mut::<ArchiveState>();
                archives.loading = true;
                archives.opening = Some((id, session));
                archives.error = None;
                tracing::info!(operation_id = id, session_id = %session, "开始打开本地会话");
            }
            Err(failure) => world.resource_mut::<ArchiveState>().error = Some(failure.to_string()),
        }
        Ok(())
    });
}

pub(crate) fn catalog(world: &mut World, entries: Vec<ArchiveEntry>) -> Result {
    let mut choices = entries
        .into_iter()
        .filter(|entry| !world.resource::<ArchiveState>().deleted.contains(&entry.id))
        .map(|entry| ArchiveChoice {
            id: entry.id,
            archived: entry.archived,
            title: entry.title,
            created_at_ms: entry.created_at_ms,
            last_activity_ms: entry.last_activity_ms,
            last_sequence: entry.last_sequence,
            error: entry.error,
        })
        .collect::<Vec<_>>();
    for entry in &mut choices {
        if let Some(current) = world
            .resource::<ArchiveState>()
            .entries
            .iter()
            .find(|current| current.id == entry.id && current.last_sequence > entry.last_sequence)
        {
            *entry = current.clone();
        }
    }
    choices.sort_by(|left, right| {
        right
            .last_activity_ms
            .cmp(&left.last_activity_ms)
            .then_with(|| left.id.as_bytes().cmp(right.id.as_bytes()))
    });
    let mut state = world.resource_mut::<ArchiveState>();
    if choices.iter().all(|entry| entry.archived) {
        state.loading = false;
    }
    state.entries = choices;
    state.refreshing = false;
    state.error = None;
    rebuild_model(world)
}

pub(crate) fn rebuild_model(world: &mut World) -> Result {
    let catalog = crate::projects::library(world);
    let projects = catalog
        .projects
        .iter()
        .map(|view| {
            (
                view.project.id,
                NavigatorEntry {
                    target: Target::Project(view.project.id),
                    title: view.project.name.clone(),
                    error: view.directory_error.clone(),
                },
            )
        })
        .collect::<Vec<_>>();
    let state = world.resource::<ArchiveState>();
    let active = state.selected;
    let sessions = state
        .entries
        .iter()
        .filter(|choice| !choice.archived)
        .map(|choice| {
            (
                choice.id,
                catalog.membership.get(&choice.id).copied(),
                NavigatorEntry {
                    target: Target::Session(choice.id),
                    title: choice.title.clone(),
                    error: choice.error.clone(),
                },
            )
        })
        .collect::<Vec<_>>();
    let active_moved = world.resource_scope(|world, mut nav: Mut<Navigator>| {
        let mut active_moved = false;
        let mut old_projects = std::mem::take(&mut nav.project_nodes);
        let mut old_sessions = std::mem::take(&mut nav.session_nodes);
        let mut project_order = Vec::with_capacity(projects.len());
        let mut sessions_by_parent = HashMap::<Entity, Vec<Entity>>::new();
        for (id, entry) in projects {
            let node = update_node(world, old_projects.remove(&id), entry);
            set_parent(world, node, nav.projects);
            project_order.push(node);
            nav.project_nodes.insert(id, node);
        }
        for (id, membership, entry) in sessions {
            let parent = membership
                .and_then(|project| nav.project_nodes.get(&project).copied())
                .unwrap_or(nav.standalone);
            let node = update_node(world, old_sessions.remove(&id), entry);
            if Some(id) == active && world.get::<ChildOf>(node).map(ChildOf::parent) != Some(parent)
            {
                active_moved = true;
            }
            set_parent(world, node, parent);
            sessions_by_parent.entry(parent).or_default().push(node);
            nav.session_nodes.insert(id, node);
        }
        // Reparent the surviving nodes before despawning their former project.
        for node in old_sessions.into_values().chain(old_projects.into_values()) {
            world.despawn(node);
        }
        order_children(world, nav.projects, &project_order);
        for parent in project_order.iter().copied().chain([nav.standalone]) {
            let children = sessions_by_parent.remove(&parent).unwrap_or_default();
            order_children(world, parent, &children);
        }
        active_moved
    });
    if active_moved {
        reveal_selection(world)?;
    } else {
        sync_selection(world)?;
    }
    Ok(())
}

fn update_node(world: &mut World, node: Option<Entity>, entry: NavigatorEntry) -> Entity {
    if let Some(node) = node {
        if let Some(mut current) = world.get_mut::<NavigatorEntry>(node) {
            current.set_if_neq(entry);
        }
        node
    } else {
        world.spawn((WidgetryTreeNode, entry)).id()
    }
}

fn set_parent(world: &mut World, node: Entity, parent: Entity) {
    if world.get::<ChildOf>(node).map(ChildOf::parent) != Some(parent) {
        world.entity_mut(node).insert(ChildOf(parent));
    }
}

fn order_children(world: &mut World, parent: Entity, children: &[Entity]) {
    let old = world
        .get::<Children>(parent)
        .map_or(&[][..], |children| &children[..]);
    if old != children {
        world.entity_mut(parent).replace_children(children);
    }
}

fn views(world: &mut World) -> Vec<(Entity, ArchiveSelector)> {
    world
        .query::<(Entity, &ArchiveSelector)>()
        .iter(world)
        .map(|(entity, section)| (entity, *section))
        .collect()
}

fn active_node(world: &World) -> Option<Entity> {
    let session = world.resource::<ArchiveState>().selected?;
    world
        .resource::<Navigator>()
        .session_nodes
        .get(&session)
        .copied()
}

pub(crate) fn reveal_selection(world: &mut World) -> Result {
    if let Some(node) = active_node(world)
        && let Some(parent) = world.get::<ChildOf>(node).map(ChildOf::parent)
        && parent != world.resource::<Navigator>().standalone
    {
        for (view, section) in views(world) {
            if section == ArchiveSelector::Projects {
                WidgetryTreeView::expand(world, view, parent)
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
    }
    sync_selection(world)
}

pub(crate) fn sync_selection(world: &mut World) -> Result {
    let node = active_node(world);
    let parent = node.and_then(|node| world.get::<ChildOf>(node).map(ChildOf::parent));
    let standalone = world.resource::<Navigator>().standalone;
    for (view, section) in views(world) {
        let visible = if let Some(parent) = parent {
            match section {
                ArchiveSelector::Standalone => parent == standalone,
                ArchiveSelector::Projects => {
                    parent != standalone
                        && WidgetryTreeView::state(world, view)
                            .map_err(|failure| failure.into_bevy_error())?
                            .is_expanded(parent)
                }
            }
        } else {
            false
        };
        WidgetryTreeView::select(world, view, node.filter(|_| visible))
            .map_err(|failure| failure.into_bevy_error())?;
    }
    Ok(())
}

fn update(
    state: Res<ArchiveState>,
    activity: Res<HostActivity>,
    chat: Res<ChatState>,
    mut selectors: Query<(Entity, Has<InteractionDisabled>), With<ArchiveSelector>>,
    mut texts: Query<&mut Text, With<ArchiveStatus>>,
    mut commands: Commands,
) {
    let disabled = state.loading || chat.replacement.is_some();
    for (entity, current) in &mut selectors {
        if disabled && !current {
            commands.entity(entity).insert(InteractionDisabled);
        } else if !disabled && current {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
    let status = if let Some(error) = &state.error {
        error.clone()
    } else if !state.cleanup_errors.is_empty() {
        state.cleanup_errors.join("\n")
    } else if activity.model_busy && !state.loading {
        "当前任务结束并回收后可切换".into()
    } else if state.loading {
        "正在打开本地会话…".into()
    } else if state.refreshing {
        "正在读取本地会话列表…".into()
    } else if state.entries.iter().all(|entry| entry.archived) {
        "尚无本地会话".into()
    } else {
        String::new()
    };
    for mut text in &mut texts {
        text.set_if_neq(Text::new(status.clone()));
    }
}

pub(crate) fn update_entry(world: &mut World, entry: ArchiveEntry) -> Result {
    let choice = ArchiveChoice {
        id: entry.id,
        archived: entry.archived,
        title: entry.title,
        created_at_ms: entry.created_at_ms,
        last_activity_ms: entry.last_activity_ms,
        last_sequence: entry.last_sequence,
        error: entry.error,
    };
    if let Some(value) = world
        .resource_mut::<ArchiveState>()
        .entries
        .iter_mut()
        .find(|entry| entry.id == choice.id && entry.last_sequence <= choice.last_sequence)
    {
        *value = choice;
    }
    rebuild_model(world)
}

pub(crate) fn remove_entry(
    world: &mut World,
    session: SessionId,
    cleanup_error: Option<String>,
) -> Result {
    let mut state = world.resource_mut::<ArchiveState>();
    state.deleted.push(session);
    state.entries.retain(|entry| entry.id != session);
    if state.selected == Some(session) {
        state.selected = None;
    }
    state.error = None;
    if let Some(failure) = cleanup_error {
        state
            .cleanup_errors
            .push(format!("会话已删除，但本地清理尚未完成：{failure}"));
    }
    crate::projects::forget_session(world, session);
    rebuild_model(world)
}
