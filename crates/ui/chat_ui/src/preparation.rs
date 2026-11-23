use crate::conversation::ChatState;
use crate::reasoning::{self, ReasoningSetting, ReasoningSource};
use agentelier_agent_host::{AgentHost, HostActivity, HostFailed, ModelEvent, ReceiveAgentEvents};
use agentelier_controls::{AgentelierButton, AgentelierButtonLoading, AgentelierButtonStyle};
use agentelier_llm_model_bridge::{ModelEventKind, ModelRequest, ModelResult};
use agentelier_login_ui::AccountState;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use std::io;
use tracing::{error, info};

const PANEL_GAP: f32 = 12.0;
const BUTTON_WIDTH: f32 = 168.0;
const BUTTON_HEIGHT: f32 = 40.0;
const MODEL_SELECTOR_WIDTH: f32 = 360.0;

#[derive(Clone, Reflect)]
pub struct ModelChoice {
    pub slug: String,
    pub display_name: String,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
    pub image_input: Option<bool>,
    pub context_window: Option<u64>,
    pub effective_window: Option<u64>,
    pub auto_compact_limit: Option<u64>,
    pub comp_hash: Option<String>,
}

#[derive(Clone, Copy, Default, Reflect, PartialEq, Eq)]
pub enum ModelPhase {
    #[default]
    Idle,
    Catalog,
    Verify,
}

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct ModelState {
    pub models: Vec<ModelChoice>,
    pub selected: Option<ModelChoice>,
    pub reasoning: ReasoningSetting,
    pub used_effort: Option<String>,
    pub operation: Option<u64>,
    pub phase: ModelPhase,
    pub catalog_loaded: bool,
    pub catalog_error: Option<String>,
    pub model: Option<String>,
    pub used_slug: Option<String>,
    pub text: Option<String>,
    pub error: Option<String>,
    pub account_identity: Option<(String, String)>,
}

impl ModelState {
    pub(crate) fn can_request(&self, account: &AccountState) -> bool {
        self.operation.is_none()
            && !account.runtime_failed
            && account.model_unavailable_reason().is_none()
    }
    pub(crate) fn selected_model(&self) -> Option<&ModelChoice> {
        self.selected
            .as_ref()
            .and_then(|selected| self.models.iter().find(|model| model.slug == selected.slug))
    }
    pub(crate) fn can_verify(&self, account: &AccountState) -> bool {
        self.can_request(account)
            && self.catalog_loaded
            && self.catalog_error.is_none()
            && self.selected_model().is_some()
            && reasoning::choices(self.selected_model()).contains(&self.reasoning)
    }
    pub(crate) fn can_create_session(&self, account: &AccountState) -> bool {
        self.can_request(account) && self.catalog_loaded && self.catalog_error.is_none()
    }
    pub(crate) fn clear_reply(&mut self) {
        self.error = None;
        self.text = None;
        self.model = None;
        self.used_slug = None;
        self.used_effort = None;
    }
}

#[derive(Component, Clone, Default)]
pub struct ChatUiHost;
#[derive(Resource)]
struct ModelSource(Entity);
#[derive(Component, Clone, Copy)]
enum ModelAction {
    Refresh,
    Verify,
}
#[derive(Component, Clone, Default)]
struct ModelSelector;
#[derive(Component, Clone, Default)]
struct SessionModelSelector;
#[derive(Component, Clone, Default)]
struct SessionModelLabel;
#[derive(Component, Clone, Default)]
struct ModelStatus;
#[derive(Component, Clone, Default)]
struct ModelText;
#[derive(Component, Clone, Default)]
struct ModelTextPreview;

pub fn install(
    app: &mut App,
    model_defaults: impl Fn() -> io::Result<crate::SessionModelDefaults> + Send + Sync + 'static,
) -> Result {
    crate::archives::install(app)?;
    crate::projects::install(app)?;
    app.add_plugins(agentelier_markdown::MarkdownPlugin);
    crate::reading::install(app);
    reasoning::install(app)?;
    app.insert_resource(crate::conversation::SessionDefaultsSource(Box::new(
        model_defaults,
    )));
    crate::conversation::install(app);
    crate::global_tasks::install(app);
    crate::task_panel::install(app)?;
    crate::task_toast::install(app);
    crate::working_directory::install(app);
    crate::prompts::install(app)?;
    crate::mcp::install(app);
    crate::memory::install(app);
    crate::workspace::install(app);
    app.register_widgetry_combo_box::<ModelChoice>()
        .map_err(|failure| failure.into_bevy_error())?;
    app.init_resource::<ModelState>()
        .register_type::<ModelState>()
        .add_systems(Startup, initialize)
        .add_observer(mount)
        .add_systems(Update, restore_selection)
        .add_systems(Update, receive.after(ReceiveAgentEvents))
        .add_systems(Update, auto_catalog.after(receive))
        .add_systems(
            PostUpdate,
            (update, update_session_model).before(UiSystems::Content),
        );
    Ok(())
}

fn initialize(mut commands: Commands) {
    let source = commands
        .spawn(WidgetryListModel::<ModelChoice>::default())
        .id();
    commands.insert_resource(ModelSource(source));
}

fn mount(event: On<Add<ChatUiHost>>, mut commands: Commands) {
    let host = event.entity;
    commands.queue(move |world: &mut World| {
        let state = world.resource::<ModelState>().clone();
        let account = world.resource::<AccountState>();
        let source = world.resource::<ModelSource>().0;
        let reasoning_source = world.resource::<ReasoningSource>().0;
        let summary = status(&state, account);
        let raw_text = state.text.clone().unwrap_or_default();
        let preview_display = if raw_text.is_empty() { Display::None } else { Display::Flex };
        let chat = world.resource::<crate::conversation::ChatState>().clone();
        let content = bsn_list! {
            Name("ModelPreparation")
            Node { width: percent(100), height: percent(100), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(PANEL_GAP) }
            Children [
                Node { column_gap: px(PANEL_GAP), flex_wrap: FlexWrap::Wrap, row_gap: px(PANEL_GAP), flex_shrink: 0.0 }
                Children [
                    @action_button("刷新模型目录", "RefreshModelCatalog", ModelAction::Refresh)--
                    @WidgetryComboBox::<ModelChoice> {
                        @source: source,
                        @placeholder: {WidgetryComboBoxContent::new(|| bsn_list! { Text("请选择模型") WidgetryText })},
                        @renderer: {WidgetryListViewRenderer::new(|model: &ModelChoice| {
                            let label = model.display_name.clone();
                            bsn_list! { Text(label) WidgetryText }
                        })},
                    }
                    Name("ModelSelector") ModelSelector
                    Node { width: px(MODEL_SELECTOR_WIDTH), flex_shrink: 0.0 }
                    on(select_model)--
                    @reasoning::selector(reasoning_source)--
                    @action_button("验证模型连接", "VerifyModelConnection", ModelAction::Verify)--
                    @crate::prompts::entry()
                ]--
                crate::prompts::NormalContent
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(PANEL_GAP) }
                Children [
                Text(summary) WidgetryText ModelStatus Name("ModelConnectionStatus")
                TextLayout::linebreak(LineBreak::AnyCharacter)--
                @WidgetryScrollArea {
                    @children: {Some(Box::new(bsn_list! {
                        Text(raw_text) WidgetryText ModelText Name("ModelConnectionText")
                        TextLayout::linebreak(LineBreak::AnyCharacter)
                        Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
                    }) as Box<dyn SceneList>)}
                }
                ModelTextPreview
                Node { width: percent(100), height: px(64), flex_shrink: 0.0, display: preview_display }--
                @crate::working_directory::panel(world.resource::<crate::working_directory::DirectoryState>())--
                @crate::conversation::panel(&chat, account, source)
                ]--
                crate::prompts::PromptWorkspace Name("PromptWorkspace")
                Node { display: Display::None, width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column }
            ]
        };
        let archives = bsn_list! { @crate::archives::panel(world) };
        let tasks = crate::task_panel::panel(world);
        let layout = world.resource::<crate::WorkspaceLayout>().ratios;
        world.commands().spawn_scene_with_error_handler(bsn! {
            @crate::workspace::scene(content, archives, tasks, layout)
            ChildOf(host)
        });
    });
}

pub(crate) fn session_model_selector(source: Entity) -> impl Scene {
    bsn! {
        @WidgetryComboBox::<ModelChoice> {
            @source: source,
            @placeholder: {WidgetryComboBoxContent::new(|| bsn_list! { Text::default() WidgetryText SessionModelLabel })},
            @renderer: {WidgetryListViewRenderer::new(|model: &ModelChoice| {
                let name = model.display_name.clone();
                bsn_list! { Text(name) WidgetryText }
            })},
        }
        Name("ChatSessionModel") SessionModelSelector
        Node { width: px(MODEL_SELECTOR_WIDTH), max_width: percent(70), min_width: px(0) }
        on(select_session_model)
    }
}

fn select_session_model(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    source: Res<ModelSource>,
    models: Query<&WidgetryListModel<ModelChoice>>,
    mut commands: Commands,
) -> Result {
    let Some(id) = event.value else {
        return Ok(());
    };
    let slug = models
        .get(source.0)?
        .get_by_id(id)
        .ok_or_else(|| io::Error::other("所选模型条目已不存在"))?
        .slug
        .clone();
    commands.queue(move |world: &mut World| crate::conversation::choose_session_model(world, slug));
    WidgetryComboBox::<ModelChoice>::clear_selection(&mut commands, event.source);
    Ok(())
}

fn update_session_model(
    state: Res<ChatState>,
    model: Res<ModelState>,
    account: Res<AccountState>,
    selectors: Query<(Entity, Has<InteractionDisabled>), With<SessionModelSelector>>,
    mut labels: Query<&mut Text, With<SessionModelLabel>>,
    mut commands: Commands,
) {
    let enabled = state
        .session
        .as_ref()
        .is_some_and(|session| !session.execution_blocked && session.matches_account(&account))
        && state.model_change.is_none()
        && state.replacement.is_none()
        && model.catalog_loaded
        && model.operation.is_none()
        && model.catalog_error.is_none()
        && !account.runtime_failed;
    let label = if let Some(session) = &state.session {
        if state.model_change.is_some_and(|(_, id)| id == session.id) {
            format!("{} · 等待保存模型", session.display_name)
        } else {
            session.display_name.clone()
        }
    } else {
        "当前会话模型".to_owned()
    };
    for mut text in &mut labels {
        text.set_if_neq(Text(label.clone()));
    }
    for (entity, disabled) in &selectors {
        if enabled && disabled {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else if !enabled && !disabled {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
}

fn action_button(label: &'static str, name: &'static str, action: ModelAction) -> impl Scene {
    let style = match action {
        ModelAction::Refresh => AgentelierButtonStyle::Primary,
        ModelAction::Verify => AgentelierButtonStyle::Secondary,
    };
    bsn! {
        @AgentelierButton { @style: style, @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
        Name(name) template(move |_| Ok(action))
        Node { min_width: px(BUTTON_WIDTH), height: px(BUTTON_HEIGHT), flex_shrink: 0.0 }
        on(activate)
    }
}

fn restore_selection(
    selectors: Query<Entity, Added<ModelSelector>>,
    state: Res<ModelState>,
    mut commands: Commands,
) {
    if let Some(index) = state.selected_model().and_then(|selected| {
        state
            .models
            .iter()
            .position(|model| model.slug == selected.slug)
    }) {
        for entity in &selectors {
            WidgetryComboBox::<ModelChoice>::select_index(&mut commands, entity, index);
        }
    }
}

fn select_model(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    selectors: Query<(), With<ModelSelector>>,
    source: Res<ModelSource>,
    models: Query<&WidgetryListModel<ModelChoice>>,
    mut state: ResMut<ModelState>,
    mut commands: Commands,
) -> Result {
    if !selectors.contains(event.source) || state.operation.is_some() {
        return Ok(());
    }
    let Some(id) = event.value else {
        return Ok(());
    };
    let choice = models
        .get(source.0)?
        .get_by_id(id)
        .ok_or_else(|| io::Error::other("所选模型条目已不存在"))?
        .clone();
    if state
        .selected
        .as_ref()
        .is_none_or(|selected| selected.slug != choice.slug)
    {
        info!(model = choice.slug, "用户选择模型");
        state.reasoning = reasoning::initial(&choice);
        state.selected = Some(choice);
        state.clear_reply();
        commands.queue(reasoning::sync);
    }
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&ModelAction>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| submit(world, action));
    Ok(())
}

fn auto_catalog(world: &mut World) -> Result {
    let state = world.resource::<ModelState>();
    if !world.resource::<crate::ArchiveState>().loading
        && !state.catalog_loaded
        && state.catalog_error.is_none()
        && state.error.is_none()
    {
        submit(world, ModelAction::Refresh)?;
    }
    Ok(())
}

fn submit(world: &mut World, action: ModelAction) -> Result {
    let state = world.resource::<ModelState>();
    let account = world.resource::<AccountState>();
    if !state.can_request(account)
        || world.resource::<AgentHost>().model_busy()?
        || matches!(action, ModelAction::Verify) && !state.can_verify(account)
    {
        return Ok(());
    }
    let request = match action {
        ModelAction::Refresh => ModelRequest::Catalog,
        ModelAction::Verify => {
            let choice = state
                .selected_model()
                .ok_or_else(|| io::Error::other("没有可验证的模型"))?;
            ModelRequest::Verify {
                slug: choice.slug.clone(),
                display_name: choice.display_name.clone(),
                effort: state.reasoning.effort(),
            }
        }
    };
    let result = world.resource::<AgentHost>().request_model(request);
    let mut state = world.resource_mut::<ModelState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.phase = match action {
                ModelAction::Refresh => ModelPhase::Catalog,
                ModelAction::Verify => ModelPhase::Verify,
            };
            state.error = None;
            if matches!(action, ModelAction::Refresh) {
                state.catalog_error = None;
            }
            info!(operation_id = id, "开始模型操作");
        }
        Err(failure) => {
            state.error = Some(failure.to_string());
            error!(error = %failure, "无法开始模型操作");
        }
    }
    Ok(())
}

pub(crate) fn receive(world: &mut World, mut failures: Local<MessageCursor<HostFailed>>) -> Result {
    let account = world.resource::<AccountState>();
    let identity = account
        .model_identity()
        .map(|(subject, workspace)| (subject.to_owned(), workspace.to_owned()));
    if world.resource::<ModelState>().account_identity != identity
        || identity.is_none() && world.resource::<ModelState>().catalog_loaded
    {
        *world.resource_mut::<ModelState>() = ModelState {
            account_identity: identity,
            ..default()
        };
        let source = world.resource::<ModelSource>().0;
        world
            .get_mut::<WidgetryListModel<ModelChoice>>(source)
            .ok_or_else(|| io::Error::other("模型选择数据源不存在"))?
            .clear();
        reasoning::sync(world)?;
    }
    let events: Vec<_> = world
        .resource_mut::<Messages<ModelEvent>>()
        .drain()
        .collect();
    for event in events {
        if world.resource::<ModelState>().operation != Some(event.operation_id) {
            continue;
        }
        let mut state = world.resource_mut::<ModelState>();
        let phase = state.phase;
        state.operation = None;
        state.phase = ModelPhase::Idle;
        match event.kind {
            ModelEventKind::Completed(Ok(ModelResult::Catalog(models))) => {
                state.models = models
                    .into_iter()
                    .map(|model| ModelChoice {
                        slug: model.slug,
                        display_name: model.display_name,
                        supported_reasoning_levels: model.supported_reasoning_levels,
                        default_reasoning_level: model.default_reasoning_level,
                        image_input: model.image_input,
                        context_window: model.context_limits.window,
                        effective_window: model.context_limits.effective,
                        auto_compact_limit: model.context_limits.automatic,
                        comp_hash: model.context_limits.comp_hash,
                    })
                    .collect();
                state.catalog_loaded = true;
                state.catalog_error = None;
                let choices = state.models.clone();
                let selected = state.selected.as_ref().map(|model| model.slug.clone());
                let source = world.resource::<ModelSource>().0;
                let mut model = world
                    .get_mut::<WidgetryListModel<ModelChoice>>(source)
                    .ok_or_else(|| io::Error::other("模型选择数据源不存在"))?;
                model.clear();
                for choice in &choices {
                    model
                        .push(choice.clone())
                        .map_err(|failure| failure.into_bevy_error())?;
                }
                let mut selectors = world.query_filtered::<Entity, With<ModelSelector>>();
                let entities: Vec<_> = selectors.iter(world).collect();
                for entity in entities {
                    if let Some(index) = choices
                        .iter()
                        .position(|choice| Some(&choice.slug) == selected.as_ref())
                    {
                        WidgetryComboBox::<ModelChoice>::select_index(
                            &mut world.commands(),
                            entity,
                            index,
                        );
                    }
                }
                reasoning::sync(world)?;
                info!(model_count = choices.len(), "模型目录读取完成");
            }
            ModelEventKind::Completed(Ok(ModelResult::Verified(reply))) => {
                info!(
                    model = reply.slug,
                    operation_id = event.operation_id,
                    "指定模型连接验证完成"
                );
                state.model = Some(reply.model);
                state.used_slug = Some(reply.slug);
                state.used_effort = reply.effort;
                state.text = Some(if reply.text.is_empty() {
                    "请求已完成，但没有文本内容".into()
                } else {
                    reply.text
                });
            }
            ModelEventKind::Completed(Err(failure)) => {
                error!(
                    operation_id = event.operation_id,
                    status = failure.status,
                    code = failure.code,
                    "模型操作失败"
                );
                if phase == ModelPhase::Catalog {
                    state.catalog_error = Some(failure.to_string());
                } else {
                    state.error = Some(failure.to_string());
                }
            }
            ModelEventKind::Cancelled => {
                state.error = Some("模型操作已停止".into());
            }
        }
    }
    if let Some(failure) = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|failure| failure.reason.clone())
    {
        let mut state = world.resource_mut::<ModelState>();
        state.operation = None;
        state.phase = ModelPhase::Idle;
        state.error = Some(failure);
    }
    Ok(())
}

fn status(state: &ModelState, account: &AccountState) -> String {
    match state.phase {
        ModelPhase::Catalog => return "正在读取模型目录…".into(),
        ModelPhase::Verify => return "正在验证所选模型连接…".into(),
        ModelPhase::Idle => {}
    }
    if let Some(error) = state.catalog_error.as_ref().or(state.error.as_ref()) {
        return error.clone();
    }
    if let Some(reason) = account.model_unavailable_reason() {
        return reason.into();
    }
    if let Some(selected) = &state.selected
        && state.selected_model().is_none()
    {
        return format!(
            "所选模型不在当前目录中：{} · {}",
            selected.display_name, selected.slug
        );
    }
    if state.selected_model().is_some()
        && !reasoning::choices(state.selected_model()).contains(&state.reasoning)
    {
        return format!(
            "推理档位 {} 已不在支持范围内，请重新选择",
            state.reasoning.label()
        );
    }
    if let Some(model) = &state.model {
        return format!(
            "连接完成 · {model} · {} · 推理：{}",
            state.used_slug.as_deref().unwrap_or_default(),
            state.used_effort.as_deref().unwrap_or("服务默认")
        );
    }
    if state.catalog_loaded && state.models.is_empty() {
        return "当前账号没有可显示的模型".into();
    }
    state
        .selected
        .as_ref()
        .map(|model| {
            format!(
                "已选择：{} · {} · 推理：{}",
                model.display_name,
                model.slug,
                state.reasoning.label()
            )
        })
        .unwrap_or_else(|| "刷新模型目录，然后选择要使用的模型".into())
}

fn update(
    state: Res<ModelState>,
    account: Res<AccountState>,
    activity: Res<HostActivity>,
    mounted: Query<(), Added<ModelAction>>,
    mut buttons: Query<(Entity, &ModelAction)>,
    selectors: Query<Entity, With<ModelSelector>>,
    mut texts: Query<(&mut Text, Has<ModelStatus>, Has<ModelText>)>,
    mut preview: Query<&mut Node, With<ModelTextPreview>>,
    mut commands: Commands,
) {
    if !state.is_changed() && !account.is_changed() && !activity.is_changed() && mounted.is_empty()
    {
        return;
    }
    for (entity, action) in &mut buttons {
        let enabled = match action {
            ModelAction::Refresh => state.can_request(&account),
            ModelAction::Verify => state.can_verify(&account),
        };
        if enabled && !activity.model_busy {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            commands.entity(entity).insert(InteractionDisabled);
        }
        let loading = matches!(
            (action, state.phase),
            (ModelAction::Refresh, ModelPhase::Catalog) | (ModelAction::Verify, ModelPhase::Verify)
        );
        AgentelierButtonLoading::set(&mut commands, entity, loading);
    }
    for entity in &selectors {
        if state.can_request(&account) && !state.models.is_empty() {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
    for (mut text, is_status, is_text) in &mut texts {
        if is_status {
            text.0 = status(&state, &account);
        } else if is_text {
            text.0 = state.text.clone().unwrap_or_default();
        }
    }
    for mut node in &mut preview {
        node.display = if state.text.as_deref().is_some_and(|text| !text.is_empty()) {
            Display::Flex
        } else {
            Display::None
        };
    }
}
