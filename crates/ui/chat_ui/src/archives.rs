use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostActivity};
use agentelier_conversation_bridge::{ArchiveEntry, ConversationRequest, SessionId};
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::list_view::{
    WidgetryListModel, WidgetryListSelectionChanged, WidgetryListSelectionMode, WidgetryListView,
    WidgetryListViewAppExt, WidgetryListViewRenderer,
};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{TooltipContentFactory, WidgetryTooltip, WidgetryTooltipPlugin};
use std::io;

const SIDEBAR_WIDTH: f32 = 240.0;
const SIDEBAR_MIN_WIDTH: f32 = 168.0;
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
struct ArchiveSource(Entity);
#[derive(Component, Clone, Default)]
struct ArchiveSelector;
#[derive(Component, Clone, Default)]
struct ArchiveStatus;

pub(crate) fn install(app: &mut App) -> Result {
    if !app.is_plugin_added::<WidgetryTooltipPlugin>() {
        app.add_plugins(WidgetryTooltipPlugin);
    }
    app.register_widgetry_list_view::<ArchiveChoice>()
        .map_err(|failure| failure.into_bevy_error())?;
    crate::session_actions::install(app);
    app.init_resource::<ArchiveState>()
        .register_type::<ArchiveState>()
        .add_systems(Startup, initialize)
        .add_systems(PostUpdate, update.before(UiSystems::Content));
    Ok(())
}

fn initialize(mut commands: Commands) {
    let source = commands
        .spawn(WidgetryListModel::<ArchiveChoice>::default())
        .id();
    commands.insert_resource(ArchiveSource(source));
}

pub(crate) fn panel(world: &World) -> impl Scene {
    let source = world.resource::<ArchiveSource>().0;
    bsn! {
        Name("LocalSessionArchives") Node { width: px(SIDEBAR_WIDTH), min_width: px(SIDEBAR_MIN_WIDTH), max_width: percent(30), height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(CONTROL_GAP), flex_shrink: 0.0 }
        Children [
            Text("Agentelier") WidgetryText--
            @crate::conversation::create_button()--
            @WidgetryListView::<ArchiveChoice> {
                @source: source,
                @item_height: ROW_HEIGHT,
                @selection_mode: WidgetryListSelectionMode::Single,
                @renderer: {WidgetryListViewRenderer::new(|choice: &ArchiveChoice| {
                    let session = choice.id;
                    let full = match &choice.error { Some(error) => format!("{}\n{}", choice.title, error), None => choice.title.clone() };
                    let mut preview = choice.title.chars().take(TITLE_PREVIEW_CHARACTERS).collect::<String>();
                    if choice.title.chars().count() > TITLE_PREVIEW_CHARACTERS { preview.push('…'); }
                    bsn_list! {
                        @WidgetryTooltip { @content: {TooltipContentFactory::new(move || { let title = full.clone(); bsn_list! { Text(title) WidgetryText } })} }
                        Node { width: percent(100), min_width: px(0), overflow: Overflow::clip() }
                        Children [
                            Text(preview) WidgetryText TextLayout::no_wrap() Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }--
                            @crate::session_actions::more(session)
                        ]
                    }
                })},
            }
            Name("LocalSessionSelector") ArchiveSelector Node { width: percent(100), height: px(0), flex_basis: px(0), min_height: px(0), flex_grow: 1.0 } on(select)--
            @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text("刷新本地会话") WidgetryText }) as Box<dyn SceneList>)} }
            Name("RefreshSessionArchives") Node { height: px(CONTROL_HEIGHT) } on(refresh)--
            @crate::session_actions::panel()--
            Text("") WidgetryText ArchiveStatus Name("SessionArchiveStatus")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
    }
}

fn select(
    event: On<WidgetryListSelectionChanged>,
    selectors: Query<(), With<ArchiveSelector>>,
    source: Res<ArchiveSource>,
    models: Query<&WidgetryListModel<ArchiveChoice>>,
    archives: Res<ArchiveState>,
    mut commands: Commands,
) -> Result {
    if !selectors.contains(event.source) {
        return Ok(());
    }
    let Some(id) = event.selected.iter().next().copied() else {
        return Ok(());
    };
    let choice = models
        .get(source.0)?
        .get_by_id(id)
        .ok_or_else(|| io::Error::other("会话条目已变化"))?;
    if archives.selected == Some(choice.id) {
        return Ok(());
    }
    let session = choice.id;
    commands.queue(move |world: &mut World| -> Result {
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
    Ok(())
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
    let source = world.resource::<ArchiveSource>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ArchiveChoice>>(source)
        .ok_or_else(|| io::Error::other("本地会话选择数据源不存在"))?;
    for index in (0..model.len()).rev() {
        if model
            .get(index)
            .is_some_and(|old| !choices.iter().any(|entry| entry.id == old.id))
        {
            model.remove(index);
        }
    }
    for (index, choice) in choices.iter().enumerate() {
        if let Some(previous) =
            (0..model.len()).find(|&old| model.get(old).is_some_and(|entry| entry.id == choice.id))
        {
            model.move_item(previous, index);
            if model.get(index) != Some(choice)
                && let Some(value) = model
                    .get_mut(index)
                    .map_err(|failure| failure.into_bevy_error())?
            {
                *value = choice.clone();
            }
        } else {
            model
                .insert(index, choice.clone())
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    let mut state = world.resource_mut::<ArchiveState>();
    if choices.is_empty() {
        state.loading = false;
    }
    state.entries = choices;
    state.refreshing = false;
    state.error = None;
    sync_selection(world)
}

pub(crate) fn sync_selection(world: &mut World) -> Result {
    let state = world.resource::<ArchiveState>();
    let index = state
        .selected
        .and_then(|selected| state.entries.iter().position(|entry| entry.id == selected));
    if let Some(index) = index {
        let mut query = world.query_filtered::<Entity, With<ArchiveSelector>>();
        let entities = query.iter(world).collect::<Vec<_>>();
        for entity in entities {
            WidgetryListView::<ArchiveChoice>::select_index(&mut world.commands(), entity, index);
        }
    } else {
        let entities = world
            .query_filtered::<Entity, With<ArchiveSelector>>()
            .iter(world)
            .collect::<Vec<_>>();
        for entity in entities {
            WidgetryListView::<ArchiveChoice>::clear_selection(&mut world.commands(), entity);
        }
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
    let source = world.resource::<ArchiveSource>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ArchiveChoice>>(source)
        .ok_or_else(|| io::Error::other("会话数据源不存在"))?;
    if let Some(index) = (0..model.len()).find(|&index| {
        model.get(index).is_some_and(|entry| {
            entry.id == choice.id && entry.last_sequence <= choice.last_sequence
        })
    }) && let Some(value) = model
        .get_mut(index)
        .map_err(|failure| failure.into_bevy_error())?
    {
        *value = choice.clone();
    }
    if let Some(value) = world
        .resource_mut::<ArchiveState>()
        .entries
        .iter_mut()
        .find(|entry| entry.id == choice.id && entry.last_sequence <= choice.last_sequence)
    {
        *value = choice;
    }
    Ok(())
}

pub(crate) fn remove_entry(
    world: &mut World,
    session: SessionId,
    cleanup_error: Option<String>,
) -> Result {
    let source = world.resource::<ArchiveSource>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ArchiveChoice>>(source)
        .ok_or_else(|| io::Error::other("会话列表数据源不存在"))?;
    if let Some(index) =
        (0..model.len()).find(|&index| model.get(index).is_some_and(|entry| entry.id == session))
    {
        model.remove(index);
    }
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
    sync_selection(world)
}
