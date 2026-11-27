use crate::conversation::{ChatState, NewChatTarget, SessionUiStates};
use agentelier_agent_host::AgentHost;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::ProjectId;
use agentelier_conversation_bridge::{ArchiveEntry, ConversationRequest, SessionId};
use bevy::picking::{events::PointerClick, hover::HoverMap};
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy_app_assets::SidebarIcon;
use bevy_widgetry::icon::{
    WidgetryIcon, WidgetryIconColorOverrides, WidgetryIconStateColorOverrides,
};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tree::{
    WidgetryListViewContainerColorOverrides, WidgetryListViewContainerStateColorOverrides,
    WidgetryTreeAppExt, WidgetryTreeColorOverrides, WidgetryTreeEvent, WidgetryTreeEventKind,
    WidgetryTreeExpanderColorOverrides, WidgetryTreeExpanderInteractionColorOverrides,
    WidgetryTreeExpanderStateColorOverrides, WidgetryTreeModel, WidgetryTreeNode,
    WidgetryTreeRenderer, WidgetryTreeState, WidgetryTreeView,
};
use std::collections::HashMap;

const ROW_HEIGHT: f32 = 44.0;
const SESSION_INDENT: f32 = 8.0;
const SESSION_TITLE_LEFT_SHIFT: f32 = 18.0;
const CONTROL_GAP: f32 = 12.0;
const FOLDER_SIZE: u32 = 24;
const ROW_ACTION_SIZE: u32 = 28;
const SESSION_SPINNER_SIZE: f32 = 16.0;
// Center the rendered folder over Widgetry's expander hit area, which precedes the row content.
const FOLDER_LEFT_OFFSET: f32 = 28.0;

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
    startup: ArchiveStartup,
    pub error: Option<String>,
    pub opening: Option<(u64, SessionId)>,
    deleted: Vec<SessionId>,
    pub cleanup_errors: Vec<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Reflect)]
enum ArchiveStartup {
    #[default]
    Scanning,
    Restoring,
    Ready,
}

impl Default for ArchiveState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            startup: ArchiveStartup::default(),
            error: None,
            opening: None,
            deleted: Vec::new(),
            cleanup_errors: Vec::new(),
        }
    }
}

impl ArchiveState {
    pub(crate) fn busy(&self) -> bool {
        self.startup != ArchiveStartup::Ready || self.opening.is_some()
    }

    pub(crate) fn finish_startup(&mut self) {
        self.startup = ArchiveStartup::Ready;
    }
}

#[derive(Resource)]
struct Navigator {
    source: Entity,
    root: Entity,
    standalone: Entity,
    project_nodes: HashMap<ProjectId, Entity>,
    session_nodes: HashMap<SessionId, Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Group(NewChatTarget),
    Session(SessionId),
}

#[derive(Component, PartialEq, Eq)]
struct NavigatorEntry {
    target: Target,
    title: String,
}
#[derive(Component, Clone, Default)]
struct ProjectTree;

#[derive(Component, Clone, Copy)]
struct ProjectFolder(Entity);

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

#[derive(Component, Clone, Default)]
struct ArchiveStatus;

pub(crate) fn install(app: &mut App) -> Result {
    app.register_renderer::<NavigatorEntry>(WidgetryTreeRenderer::new(
        |node, entry: &NavigatorEntry| render(node, entry),
    ))
    .map_err(|failure| failure.into_bevy_error())?;
    crate::session_actions::install(app);
    app.init_resource::<ArchiveState>()
        .register_type::<ArchiveState>()
        .add_systems(Startup, initialize)
        .add_systems(
            PostUpdate,
            (update, reveal_on_hover, update_indicators, update_folders).before(UiSystems::Content),
        );
    Ok(())
}

fn initialize(mut commands: Commands) {
    let root = commands.spawn_empty().id();
    let standalone = commands
        .spawn((
            WidgetryTreeNode,
            NavigatorEntry {
                target: Target::Group(NewChatTarget::Standalone),
                title: "单独聊天".into(),
            },
            ChildOf(root),
        ))
        .id();
    let source = commands.spawn(WidgetryTreeModel::new(root)).id();
    commands.insert_resource(Navigator {
        source,
        root,
        standalone,
        project_nodes: HashMap::new(),
        session_nodes: HashMap::new(),
    });
}

fn render(node: Entity, entry: &NavigatorEntry) -> impl SceneList + use<> {
    let title = entry.title.clone();
    let session = matches!(entry.target, Target::Session(_));
    let title_margin = if session {
        UiRect::left(px(-SESSION_TITLE_LEFT_SHIFT))
    } else {
        UiRect::ZERO
    };
    let (name, icon, actions): (_, Option<Box<dyn SceneList>>, Box<dyn SceneList>) = match entry
        .target
    {
        Target::Group(target) => {
            let (name, project) = match target {
                NewChatTarget::Project(id) => (format!("NavigatorProject-{id}"), Some(id)),
                NewChatTarget::Standalone => ("NavigatorStandaloneProject".into(), None),
            };
            let more = project.map(|id| bsn_list! { @crate::projects::more(id) });
            (
                name,
                Some(Box::new(bsn_list! {
                    @WidgetryIcon { @path: {SidebarIcon::ProjectFolder.path()}, @max_size: {Some(UVec2::splat(FOLDER_SIZE))} }
                    template(move |_| Ok(ProjectFolder(node)))
                    Pickable::IGNORE Node { width: px(FOLDER_SIZE as f32), height: px(FOLDER_SIZE as f32), margin: UiRect::left(px(-FOLDER_LEFT_OFFSET)), flex_shrink: 0.0 }
                })),
                Box::new(bsn_list! {
                    @crate::conversation::new_chat_button(target)--
                    {more}
                }),
            )
        }
        Target::Session(id) => (
            format!("NavigatorSession-{id}"),
            None,
            Box::new(bsn_list! {
                @indicator(id)--
                @crate::session_actions::more(id)
            }),
        ),
    };
    bsn_list! {
        Name(name)
        Node { width: percent(100), height: percent(100), min_width: px(0), align_items: AlignItems::Center, column_gap: px(6) }
        Children [
            {icon}--
            Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), margin: title_margin }
            Children [ @crate::text_overflow::text(title, session) ]--
            {actions}
        ]
    }
}

pub(crate) fn panel(world: &World) -> impl Scene + use<> {
    let source = world.resource::<Navigator>().source;
    bsn! {
        Name("LocalSessionArchives") Node { width: percent(100), min_width: px(0), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(CONTROL_GAP), flex_shrink: 0.0 }
        Children [
            @project_tree(source)--
            @crate::session_actions::panel()
        ]
    }
}

fn project_tree(source: Entity) -> impl Scene {
    bsn! {
        Name("NavigatorProjects")
        Node { width: percent(100), min_height: px(0), flex_basis: px(0), flex_grow: 1.0, flex_direction: FlexDirection::Column }
        Children [
            Node {
                width: percent(100), height: px(40), padding: UiRect::horizontal(px(8)),
                align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, flex_shrink: 0.0,
            }
            Children [
                Text("项目") WidgetryText Pickable::IGNORE--
                @crate::projects::create_button()
            ]--
            @crate::projects::status()--
            Text("") WidgetryText ArchiveStatus Name("SessionArchiveStatus")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter) Node { display: Display::None }--
            @WidgetryTreeView { @source: source, @item_height: ROW_HEIGHT, @indent_width: SESSION_INDENT, @colors: {project_tree_colors()} }
            ProjectTree
            Node { width: percent(100), flex_basis: px(0), min_height: px(0), flex_grow: 1.0 }
            on(select)
        ]
    }
}

fn project_tree_colors() -> WidgetryTreeColorOverrides {
    let container = WidgetryListViewContainerStateColorOverrides {
        background: Some(Color::NONE),
        border: Some(Color::NONE),
        ..default()
    };
    let state = WidgetryTreeExpanderStateColorOverrides {
        foreground: Some(Color::NONE),
        ..default()
    };
    let interaction = WidgetryTreeExpanderInteractionColorOverrides {
        normal: state.clone(),
        hovered: state.clone(),
        pressed: state.clone(),
        disabled: state,
    };
    WidgetryTreeColorOverrides {
        container: WidgetryListViewContainerColorOverrides {
            normal: container.clone(),
            focused: container.clone(),
            disabled: container,
        },
        expander: WidgetryTreeExpanderColorOverrides {
            collapsed: interaction.clone(),
            expanded: interaction,
        },
        ..default()
    }
}

fn update_folders(
    trees: Query<Ref<WidgetryTreeState>, With<ProjectTree>>,
    folders: Query<(Entity, Ref<ProjectFolder>, &WidgetryIcon)>,
    mut commands: Commands,
) {
    let Ok(tree) = trees.single() else {
        return;
    };
    for (entity, folder, icon) in &folders {
        if !tree.is_changed() && !folder.is_added() {
            continue;
        }
        let path = if tree.is_expanded(folder.0) {
            SidebarIcon::OpenProjectFolder.path()
        } else {
            SidebarIcon::ProjectFolder.path()
        };
        if icon.path() != Some(&path) {
            commands.queue(move |world: &mut World| -> Result {
                if world.get_entity(entity).is_err() {
                    return Ok(());
                }
                WidgetryIcon::set_svg_in_world(world, entity, path)
                    .map_err(|failure| failure.into_bevy_error())?;
                Ok(())
            });
        }
    }
}

pub(crate) fn reveal_button(icon: SidebarIcon) -> impl Scene {
    let icon_size = 12;
    let size = ROW_ACTION_SIZE as f32;
    bsn! {
        @AgentelierButton {
            @style: AgentelierButtonStyle::Icon,
            @children: bsn_list! {
                @WidgetryIcon { @path: {icon.path()}, @max_size: {Some(UVec2::splat(icon_size))} }
                Pickable::IGNORE
            },
        }
        RevealOnHover Visibility::Hidden Node { min_height: px(size), height: px(size) }
    }
}

pub(crate) fn block_row_click(mut event: On<PointerClick>) {
    event.propagate(false);
}

fn reveal_on_hover(
    hover: Res<HoverMap>,
    parents: Query<&ChildOf>,
    mut buttons: Query<(&ChildOf, Option<&Children>, &mut Visibility), With<RevealOnHover>>,
    menus: Query<(), With<crate::session_actions::SidebarMenu>>,
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
    let size = ROW_ACTION_SIZE as f32;
    let error_color = WidgetryIconStateColorOverrides {
        foreground: Some(Color::srgb_u8(0xB9, 0x19, 0x2E)),
    };
    bsn! {
        Name({format!("SessionActivity-{session}")})
        template(move |_| Ok(SessionIndicator(session)))
        Visibility::Hidden Pickable::IGNORE
        Node { width: px(size), height: px(size), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0 }
        Children [
            @crate::spinner::ring(SESSION_SPINNER_SIZE) Name("SessionBusy") Node { display: Display::None }--
            @WidgetryIcon {
                @path: {SidebarIcon::SessionError.path()},
                @max_size: {Some(UVec2::splat(ROW_ACTION_SIZE))},
                @colors: {WidgetryIconColorOverrides { normal: error_color.clone(), disabled: error_color }},
            }
            Name("SessionError") Pickable::IGNORE Node { display: Display::None }
        ]
    }
}

fn chat_activity(chat: &ChatState) -> SessionActivity {
    let Some(session) = &chat.session else {
        return SessionActivity::Idle;
    };
    use crate::conversation::ChatTurnStatus;
    if chat.error.is_some()
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
        })
    {
        SessionActivity::Error
    } else if chat.operation.is_some()
        || session
            .turn
            .as_ref()
            .is_some_and(|turn| matches!(turn.status, ChatTurnStatus::Running))
        || !session.queue_paused && !session.queue.is_empty()
    {
        SessionActivity::Busy
    } else {
        SessionActivity::Idle
    }
}

fn update_indicators(
    chat: Res<ChatState>,
    parked: Res<SessionUiStates>,
    archives: Res<ArchiveState>,
    tasks: Res<crate::global_tasks::GlobalTaskState>,
    mut indicators: Query<(&SessionIndicator, &Children, &mut Visibility)>,
    mut parts: Query<&mut Node>,
) {
    let current = chat.session.as_ref().map(|session| session.id);
    for (indicator, children, mut visibility) in &mut indicators {
        let id = indicator.0;
        let state = if current == Some(id) {
            Some(chat.as_ref())
        } else {
            parked.session(id)
        };
        let projected = state.map_or(SessionActivity::Idle, chat_activity);
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
            if let Ok(node) = parts.get_mut(child) {
                let display = if activity == state {
                    Display::Flex
                } else {
                    Display::None
                };
                node.map_unchanged(|node| &mut node.display)
                    .set_if_neq(display);
            }
        }
    }
}

fn select(event: On<WidgetryTreeEvent>, entries: Query<&NavigatorEntry>, mut commands: Commands) {
    let selected = match &event.kind {
        WidgetryTreeEventKind::SelectionChanged { selected, .. } => selected,
        WidgetryTreeEventKind::Expanded(_) | WidgetryTreeEventKind::Collapsed(_) => {
            commands.queue(sync_selection);
            return;
        }
        _ => return,
    };
    let target = selected
        .iter()
        .find_map(|entity| entries.get(*entity).ok())
        .map(|entry| entry.target);
    commands.queue(move |world: &mut World| -> Result {
        let session = match target {
            Some(Target::Group(group))
                if world.resource::<ChatState>().draft_target() == Some(group) =>
            {
                return Ok(());
            }
            Some(Target::Session(session)) => session,
            _ => return sync_selection(world),
        };
        if world.resource::<ArchiveState>().selected == Some(session) {
            return Ok(());
        }
        sync_selection(world)?;
        if world.resource::<ArchiveState>().busy() {
            return Ok(());
        }
        agentelier_agent_host::flush_scene_state(world);
        match world
            .resource::<AgentHost>()
            .request_conversation(ConversationRequest::Open { session })
        {
            Ok(id) => {
                let mut archives = world.resource_mut::<ArchiveState>();
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
    if state.startup == ArchiveStartup::Scanning {
        state.startup = if choices.iter().any(|entry| !entry.archived) {
            ArchiveStartup::Restoring
        } else {
            ArchiveStartup::Ready
        };
    }
    state.entries = choices;
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
                    target: Target::Group(NewChatTarget::Project(view.project.id)),
                    title: view.project.name.clone(),
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
                },
            )
        })
        .collect::<Vec<_>>();
    let active_moved = world.resource_scope(|world, mut nav: Mut<Navigator>| {
        let mut active_moved = false;
        let mut old_projects = std::mem::take(&mut nav.project_nodes);
        let mut old_sessions = std::mem::take(&mut nav.session_nodes);
        let mut project_order = Vec::with_capacity(projects.len() + 1);
        let mut sessions_by_parent = HashMap::<Entity, Vec<Entity>>::new();
        for (id, entry) in projects {
            let node = update_node(world, old_projects.remove(&id), entry);
            set_parent(world, node, nav.root);
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
        project_order.push(nav.standalone);
        order_children(world, nav.root, &project_order);
        for parent in project_order {
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

fn views(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, With<ProjectTree>>()
        .iter(world)
        .collect()
}

fn active_session_node(world: &World) -> Option<Entity> {
    let session = world.resource::<ArchiveState>().selected?;
    world
        .resource::<Navigator>()
        .session_nodes
        .get(&session)
        .copied()
}

pub(crate) fn reveal_selection(world: &mut World) -> Result {
    if let Some(node) = active_session_node(world)
        && let Some(parent) = world.get::<ChildOf>(node).map(ChildOf::parent)
    {
        for view in views(world) {
            WidgetryTreeView::expand(world, view, parent)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    project_selection(world, SelectionSync::Reveal)
}

pub(crate) fn sync_selection(world: &mut World) -> Result {
    project_selection(world, SelectionSync::Highlight)
}

#[derive(Clone, Copy)]
enum SelectionSync {
    Highlight,
    Reveal,
}

fn project_selection(world: &mut World, sync: SelectionSync) -> Result {
    let session = active_session_node(world);
    let parent = session.and_then(|node| world.get::<ChildOf>(node).map(ChildOf::parent));
    let navigator = world.resource::<Navigator>();
    let draft = match world.resource::<ChatState>().draft_target() {
        Some(NewChatTarget::Project(id)) => navigator.project_nodes.get(&id).copied(),
        Some(NewChatTarget::Standalone) => Some(navigator.standalone),
        None => None,
    };
    for view in views(world) {
        let target = if let Some(parent) = parent {
            let expanded = WidgetryTreeView::state(world, view)
                .map_err(|failure| failure.into_bevy_error())?
                .is_expanded(parent);
            session.filter(|_| expanded)
        } else {
            draft
        };
        match sync {
            SelectionSync::Highlight => {
                let selection = WidgetryTreeView::selection(world, view)
                    .map_err(|failure| failure.into_bevy_error())?;
                if selection.selected_id() != target {
                    WidgetryTreeView::replace_selection(world, view, target)
                        .map_err(|failure| failure.into_bevy_error())?;
                }
            }
            SelectionSync::Reveal => {
                WidgetryTreeView::select(world, view, target)
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
    }
    Ok(())
}

fn update(
    state: Res<ArchiveState>,
    selectors: Query<(Entity, Has<InteractionDisabled>), With<ProjectTree>>,
    mut texts: Query<(&mut Text, &mut Node), With<ArchiveStatus>>,
    mut commands: Commands,
) {
    for (entity, current) in &selectors {
        if state.opening.is_some() && !current {
            commands.entity(entity).insert(InteractionDisabled);
        } else if state.opening.is_none() && current {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
    let progress = match state.startup {
        ArchiveStartup::Scanning => Some("正在读取项目和会话…"),
        ArchiveStartup::Restoring => Some("正在读取上次会话…"),
        ArchiveStartup::Ready if state.opening.is_some() => Some("正在打开会话…"),
        ArchiveStartup::Ready => None,
    };
    let status = state
        .error
        .clone()
        .or_else(|| progress.map(str::to_owned))
        .unwrap_or_else(|| state.cleanup_errors.join("\n"));
    let display = if status.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    for (mut text, node) in &mut texts {
        text.set_if_neq(Text::new(status.clone()));
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
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
