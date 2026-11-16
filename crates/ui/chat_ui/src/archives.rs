use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostActivity};
use agentelier_conversation_bridge::{ArchiveEntry, ConversationRequest, SessionId};
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer};
use bevy_widgetry::text::WidgetryText;
use std::io;

const SELECTOR_WIDTH: f32 = 360.0;
const CONTROL_HEIGHT: f32 = 40.0;
const CONTROL_GAP: f32 = 12.0;

#[derive(Clone, Reflect)]
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
}

impl Default for ArchiveState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            loading: true,
            refreshing: true,
            error: None,
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
    app.register_widgetry_combo_box::<ArchiveChoice>()
        .map_err(|failure| failure.into_bevy_error())?;
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
        Name("LocalSessionArchives") Node { width: percent(100), column_gap: px(CONTROL_GAP), row_gap: px(CONTROL_GAP), flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, flex_shrink: 0.0 }
        Children [
            Text("本地会话") WidgetryText--
            @WidgetryComboBox::<ArchiveChoice> {
                @source: source,
                @placeholder: {WidgetryComboBoxContent::new(|| bsn_list! { Text("打开已保存的会话") WidgetryText })},
                @renderer: {WidgetryListViewRenderer::new(|choice: &ArchiveChoice| { let title = choice.title.clone(); bsn_list! { Text(title) WidgetryText } })},
            }
            Name("LocalSessionSelector") ArchiveSelector Node { width: px(SELECTOR_WIDTH), flex_shrink: 0.0 } on(select)--
            @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text("刷新本地会话") WidgetryText }) as Box<dyn SceneList>)} }
            Name("RefreshSessionArchives") Node { height: px(CONTROL_HEIGHT) } on(refresh)--
            Text("") WidgetryText ArchiveStatus Name("SessionArchiveStatus")
        ]
    }
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    selectors: Query<(), With<ArchiveSelector>>,
    source: Res<ArchiveSource>,
    models: Query<&WidgetryListModel<ArchiveChoice>>,
    archives: Res<ArchiveState>,
    mut commands: Commands,
) -> Result {
    if !selectors.contains(event.source) || archives.loading {
        return Ok(());
    }
    let Some(id) = event.value else {
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
        if world.resource::<ChatState>().operation.is_some()
            || world.resource::<ArchiveState>().loading
            || world.resource::<AgentHost>().model_busy()?
        {
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
    let choices = entries
        .into_iter()
        .map(|entry| ArchiveChoice {
            id: entry.id,
            title: entry.title,
            created_at_ms: entry.created_at_ms,
            last_activity_ms: entry.last_activity_ms,
            last_sequence: entry.last_sequence,
            error: entry.error,
        })
        .collect::<Vec<_>>();
    let source = world.resource::<ArchiveSource>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ArchiveChoice>>(source)
        .ok_or_else(|| io::Error::other("本地会话选择数据源不存在"))?;
    model.clear();
    for choice in &choices {
        model
            .push(choice.clone())
            .map_err(|failure| failure.into_bevy_error())?;
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
    if state.loading {
        return Ok(());
    }
    let index = state
        .selected
        .and_then(|selected| state.entries.iter().position(|entry| entry.id == selected));
    if let Some(index) = index {
        let mut query = world.query_filtered::<Entity, With<ArchiveSelector>>();
        let entities = query.iter(world).collect::<Vec<_>>();
        for entity in entities {
            WidgetryComboBox::<ArchiveChoice>::select_index(&mut world.commands(), entity, index);
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
    let disabled = state.loading || activity.model_busy || chat.replacement.is_some();
    for (entity, current) in &mut selectors {
        if disabled && !current {
            commands.entity(entity).insert(InteractionDisabled);
        } else if !disabled && current {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
    let status = if let Some(error) = &state.error {
        error.clone()
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
