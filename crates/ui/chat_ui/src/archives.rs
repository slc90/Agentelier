use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostActivity};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::ProjectId;
use agentelier_conversation_bridge::{ArchiveEntry, ConversationRequest, SessionId};
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{TooltipContentFactory, WidgetryTooltip, WidgetryTooltipPlugin};
use bevy_widgetry::tree::{
    WidgetryTreeAppExt, WidgetryTreeEvent, WidgetryTreeEventKind, WidgetryTreeModel,
    WidgetryTreeNode, WidgetryTreeRenderer, WidgetryTreeView,
};
use std::collections::HashMap;

const ROW_HEIGHT: f32 = 44.0;
const TITLE_PREVIEW_CHARACTERS: usize = 18;
const CONTROL_HEIGHT: f32 = 40.0;
const CONTROL_GAP: f32 = 12.0;

#[derive(Clone, Reflect, PartialEq)]
pub struct ArchiveChoice {
    pub id: SessionId,
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
    source: Entity,
    projects: Entity,
    standalone: Entity,
    project_nodes: HashMap<ProjectId, Entity>,
    session_nodes: HashMap<SessionId, Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Projects,
    Standalone,
    Project(ProjectId),
    Session(SessionId),
}

#[derive(Component, PartialEq, Eq)]
struct NavigatorEntry {
    target: Target,
    title: String,
    error: Option<String>,
}
#[derive(Component, Clone, Default)]
struct ArchiveSelector;
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
        .add_systems(Startup, initialize)
        .add_systems(PostUpdate, update.before(UiSystems::Content));
    Ok(())
}

fn initialize(mut commands: Commands) {
    let root = commands.spawn_empty().id();
    let projects = commands
        .spawn((
            WidgetryTreeNode,
            NavigatorEntry {
                target: Target::Projects,
                title: "项目".into(),
                error: None,
            },
            ChildOf(root),
        ))
        .id();
    let standalone = commands
        .spawn((
            WidgetryTreeNode,
            NavigatorEntry {
                target: Target::Standalone,
                title: "单独聊天".into(),
                error: None,
            },
            ChildOf(root),
        ))
        .id();
    let source = commands.spawn(WidgetryTreeModel::new(root)).id();
    commands.insert_resource(Navigator {
        source,
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
        Target::Projects => ("NavigatorProjects".into(), None),
        Target::Standalone => ("NavigatorStandalone".into(), None),
        Target::Project(id) => (format!("NavigatorProject-{id}"), None),
        Target::Session(id) => (format!("NavigatorSession-{id}"), Some(id)),
    };
    let actions = session.map(|id| bsn_list! { @crate::session_actions::more(id) });
    bsn_list! {
        @WidgetryTooltip { @content: {TooltipContentFactory::new(move || { let title = full.clone(); bsn_list! { Text(title) WidgetryText } })} }
        Name(name)
        Node { width: percent(100), min_width: px(0), overflow: Overflow::clip() }
        Children [
            Text(preview) WidgetryText TextLayout::no_wrap() Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }--
            {actions}
        ]
    }
}

pub(crate) fn panel(world: &World) -> impl Scene + use<> {
    let source = world.resource::<Navigator>().source;
    let projects = crate::projects::source(world);
    bsn! {
        Name("LocalSessionArchives") Node { width: percent(100), min_width: px(0), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(CONTROL_GAP), flex_shrink: 0.0 }
        Children [
            @crate::projects::panel(world)--
            @crate::conversation::standalone_button()--
            @WidgetryTreeView {
                @source: source,
                @item_height: ROW_HEIGHT,
            }
            Name("SessionNavigator") ArchiveSelector Node { width: percent(100), height: px(0), flex_basis: px(0), min_height: px(0), flex_grow: 1.0 } on(select)--
            @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text("刷新本地会话") WidgetryText }) as Box<dyn SceneList>)} }
            Name("RefreshSessionArchives") Node { height: px(CONTROL_HEIGHT) } on(refresh)--
            @crate::session_actions::panel(projects)--
            Text("") WidgetryText ArchiveStatus Name("SessionArchiveStatus")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
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
            Target::Projects => return Ok(()),
            Target::Standalone => return crate::projects::select_project(world, None),
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

fn refresh(_: On<Activate>, host: Res<AgentHost>, mut state: ResMut<ArchiveState>) {
    match host.refresh_archives() {
        Ok(()) => {
            state.refreshing = true;
            state.error = None;
        }
        Err(failure) => state.error = Some(failure.to_string()),
    }
}

pub(crate) fn catalog(world: &mut World, entries: Vec<ArchiveEntry>) -> Result {
    let mut choices = entries
        .into_iter()
        .filter(|entry| !world.resource::<ArchiveState>().deleted.contains(&entry.id))
        .map(|entry| ArchiveChoice {
            id: entry.id,
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
    if choices.is_empty() {
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
    let (active_moved, sections) = world.resource_scope(|world, mut nav: Mut<Navigator>| {
        let mut active_moved = false;
        let empty_sections = [nav.projects, nav.standalone]
            .into_iter()
            .filter(|section| world.get::<Children>(*section).is_none())
            .collect::<Vec<_>>();
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
        (active_moved, empty_sections)
    });
    for view in views(world) {
        for section in &sections {
            WidgetryTreeView::expand(world, view, *section)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    if active_moved {
        reveal_selection(world)?;
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

fn views(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, With<ArchiveSelector>>()
        .iter(world)
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
    {
        let nav = world.resource::<Navigator>();
        let section = if parent == nav.standalone {
            nav.standalone
        } else {
            nav.projects
        };
        for view in views(world) {
            WidgetryTreeView::expand(world, view, section)
                .map_err(|failure| failure.into_bevy_error())?;
            WidgetryTreeView::expand(world, view, parent)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    sync_selection(world)
}

pub(crate) fn sync_selection(world: &mut World) -> Result {
    let node = active_node(world);
    for view in views(world) {
        let visible = if let Some(node) = node
            && let Some(parent) = world.get::<ChildOf>(node).map(ChildOf::parent)
        {
            let tree = WidgetryTreeView::state(world, view)
                .map_err(|failure| failure.into_bevy_error())?;
            let nav = world.resource::<Navigator>();
            tree.is_expanded(parent) && (parent == nav.standalone || tree.is_expanded(nav.projects))
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
    } else if state.entries.is_empty() {
        "尚无本地会话".into()
    } else {
        String::new()
    };
    for mut text in &mut texts {
        text.set_if_neq(Text::new(status.clone()));
    }
}

pub(crate) fn rename_entry(world: &mut World, entry: ArchiveEntry) -> Result {
    let choice = ArchiveChoice {
        id: entry.id,
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
        state.cleanup_errors.push(format!(
            "会话已删除，但本地清理尚未完成；刷新列表可继续清理：{failure}"
        ));
    }
    crate::projects::forget_session(world, session);
    rebuild_model(world)
}
