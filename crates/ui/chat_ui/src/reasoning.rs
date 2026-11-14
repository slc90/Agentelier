use crate::preparation::{ModelChoice, ModelState};
use agentelier_login_ui::AccountState;
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::ValueChange;
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer};
use bevy_widgetry::text::WidgetryText;
use std::io;
use tracing::info;

const SELECTOR_WIDTH: f32 = 220.0;

#[derive(Clone, Default, Reflect, PartialEq, Eq)]
pub enum ReasoningSetting {
    #[default]
    ServiceDefault,
    Effort(String),
}

impl ReasoningSetting {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::ServiceDefault => "服务默认".into(),
            Self::Effort(effort) => effort.clone(),
        }
    }
    pub(crate) fn effort(&self) -> Option<String> {
        match self {
            Self::ServiceDefault => None,
            Self::Effort(effort) => Some(effort.clone()),
        }
    }
}

pub(crate) fn initial(model: &ModelChoice) -> ReasoningSetting {
    match &model.default_reasoning_level {
        Some(default) if model.supported_reasoning_levels.contains(default) => {
            ReasoningSetting::Effort(default.clone())
        }
        _ => ReasoningSetting::ServiceDefault,
    }
}

pub(crate) fn choices(model: Option<&ModelChoice>) -> Vec<ReasoningSetting> {
    let mut choices = Vec::new();
    if let Some(model) = model {
        if model.supported_reasoning_levels.is_empty() || model.default_reasoning_level.is_none() {
            choices.push(ReasoningSetting::ServiceDefault);
        }
        choices.extend(
            model
                .supported_reasoning_levels
                .iter()
                .cloned()
                .map(ReasoningSetting::Effort),
        );
    }
    choices
}

#[derive(Resource)]
pub(crate) struct ReasoningSource(pub(crate) Entity);
#[derive(Component, Clone, Default)]
struct ReasoningSelector;

pub(crate) fn install(app: &mut App) -> Result {
    app.register_widgetry_combo_box::<ReasoningSetting>()
        .map_err(|failure| failure.into_bevy_error())?;
    app.add_systems(Startup, initialize)
        .add_observer(restore_selection)
        .add_systems(PostUpdate, update.before(UiSystems::Content));
    Ok(())
}

fn initialize(mut commands: Commands) {
    let source = commands
        .spawn(WidgetryListModel::<ReasoningSetting>::default())
        .id();
    commands.insert_resource(ReasoningSource(source));
}

pub(crate) fn selector(source: Entity) -> impl Scene {
    bsn! {
        @WidgetryComboBox::<ReasoningSetting> {
            @source: source,
            @placeholder: {WidgetryComboBoxContent::new(|| bsn_list! { Text("服务默认") WidgetryText })},
            @renderer: {WidgetryListViewRenderer::new(|setting: &ReasoningSetting| {
                let label = setting.label();
                bsn_list! { Text(label) WidgetryText }
            })},
        }
        Name("ReasoningSelector") ReasoningSelector
        Node { width: px(SELECTOR_WIDTH), flex_shrink: 0.0 }
        on(select)
    }
}

pub(crate) fn sync(world: &mut World) -> Result {
    let state = world.resource::<ModelState>();
    let options = choices(state.selected_model());
    let selected = options.iter().position(|option| *option == state.reasoning);
    let source = world.resource::<ReasoningSource>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ReasoningSetting>>(source)
        .ok_or_else(|| io::Error::other("推理选项数据源不存在"))?;
    model.clear();
    for option in options {
        model
            .push(option)
            .map_err(|failure| failure.into_bevy_error())?;
    }
    let mut selectors = world.query_filtered::<Entity, With<ReasoningSelector>>();
    let entities: Vec<_> = selectors.iter(world).collect();
    if let Some(index) = selected {
        for entity in entities {
            WidgetryComboBox::<ReasoningSetting>::select_index(
                &mut world.commands(),
                entity,
                index,
            );
        }
    }
    Ok(())
}

fn restore_selection(
    event: On<Add<ReasoningSelector>>,
    state: Res<ModelState>,
    mut commands: Commands,
) {
    if let Some(index) = choices(state.selected_model())
        .iter()
        .position(|option| *option == state.reasoning)
    {
        WidgetryComboBox::<ReasoningSetting>::select_index(&mut commands, event.entity, index);
    }
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    selectors: Query<(), With<ReasoningSelector>>,
    source: Res<ReasoningSource>,
    sources: Query<&WidgetryListModel<ReasoningSetting>>,
    mut state: ResMut<ModelState>,
) -> Result {
    if !selectors.contains(event.source) || state.operation.is_some() {
        return Ok(());
    }
    let Some(id) = event.value else {
        return Ok(());
    };
    let setting = sources
        .get(source.0)?
        .get_by_id(id)
        .ok_or_else(|| io::Error::other("所选推理档位已不存在"))?
        .clone();
    if state.reasoning != setting {
        info!(effort = setting.effort(), "用户选择推理设置");
        state.reasoning = setting;
        state.clear_reply();
    }
    Ok(())
}

fn update(
    state: Res<ModelState>,
    account: Res<AccountState>,
    selectors: Query<Entity, With<ReasoningSelector>>,
    mut commands: Commands,
) {
    if !state.is_changed() && !account.is_changed() {
        return;
    }
    for entity in &selectors {
        if state.can_request(&account) && state.selected_model().is_some() {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
}
