use super::MAX_LABEL_BYTES;
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{
    input_focus::tab_navigation::TabIndex, prelude::*, ui::Checked, ui::UiSystems,
    ui_widgets::ValueChange,
};
use bevy_widgetry::{
    check_box::{WidgetryCheckBox, WidgetryCheckBoxPlugin},
    combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent},
    list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer},
    radio_group::{
        WidgetryRadioGroup, WidgetryRadioGroupHeadless, WidgetryRadioGroupPlugin,
        WidgetryRadioOption,
    },
    scene::spawn_scene,
    segmented::{
        WidgetrySegmented, WidgetrySegmentedHeadless, WidgetrySegmentedOption,
        WidgetrySegmentedPlugin,
    },
    text::WidgetryText,
    toggle_button::{
        WidgetryToggleButton, WidgetryToggleButtonChanged, WidgetryToggleButtonPlugin,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

const MAX_OPTIONS: usize = 24;
const MAX_OPTION_VALUE_BYTES: usize = 64;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChoiceProperties {
    label: String,
    presentation: ChoicePresentation,
    options: Vec<Choice>,
    value: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ChoicePresentation {
    ComboBox,
    RadioGroup,
    Segmented,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    value: String,
    label: String,
}

#[derive(Component, Clone, Reflect)]
#[reflect(Component)]
struct ChoiceValue(String);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BooleanProperties {
    label: String,
    presentation: BooleanPresentation,
    value: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BooleanPresentation {
    CheckBox,
    ToggleButton,
}

#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiscreteValue {
    Choice(Option<String>),
    Boolean(bool),
}

pub(crate) type DiscreteChange = ValueChange<DiscreteValue>;

#[derive(Component)]
enum InitialChoice {
    Combo(WidgetryListItemId),
    Radio(usize),
}

pub(super) fn install(app: &mut App) -> Result {
    app.add_plugins((
        WidgetryCheckBoxPlugin,
        WidgetryRadioGroupPlugin,
        WidgetrySegmentedPlugin,
        WidgetryToggleButtonPlugin,
    ))
    .register_type::<ChoiceValue>()
    .register_type::<DiscreteChange>()
    .add_systems(PostUpdate, initialize_choice.before(UiSystems::Content));
    app.register_widgetry_combo_box::<Choice>()
        .map_err(|error| error.into_bevy_error())?;
    Ok(())
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        "agentelier.choice",
        "Create a single-selection control with stable string values.",
        &[
            ("label", "Required nonblank group label, at most 256 UTF-8 bytes."),
            ("presentation", "Required: combo_box, radio_group, or segmented."),
            ("options", "Required array of 1 to 24 objects with a unique nonblank value (max 64 UTF-8 bytes) and a nonblank label (max 256 bytes)."),
            ("value", "Initial option value. Omit on update only to retain the current selection when it is still an option."),
        ],
        choice,
        None,
        Some(prepare_choice),
    )?
    .with_ecs_scene(
        "agentelier.boolean",
        "Create a checkbox or a two-state toggle button.",
        &[
            ("label", "Required nonblank visible label, at most 256 UTF-8 bytes."),
            ("presentation", "Required: check_box or toggle_button."),
            ("value", "Initial boolean. Omit on update to retain the current value of the mounted control."),
        ],
        boolean,
        None,
        Some(prepare_boolean),
    )
}

fn choice(
    world: &mut World,
    parent: Entity,
    properties: ChoiceProperties,
) -> std::result::Result<Entity, EcsError> {
    let initial = properties.initial().map_err(invalid_props)?;
    let ChoiceProperties {
        label,
        presentation,
        options,
        ..
    } = properties;
    match presentation {
        ChoicePresentation::ComboBox => combo(world, parent, label, options, initial),
        ChoicePresentation::RadioGroup | ChoicePresentation::Segmented => {
            let control = group(presentation, options, initial);
            spawn_scene(
                world,
                bsn! { @choice_row(label) ChildOf(parent) Children [@control] },
            )
            .map_err(construction_error)
        }
    }
}

impl ChoiceProperties {
    fn initial(&self) -> Result<usize> {
        validate_label(&self.label)?;
        if self.options.is_empty() || self.options.len() > MAX_OPTIONS {
            return Err(io::Error::other("options 必须包含 1 至 24 个选项").into());
        }
        let mut values = BTreeSet::new();
        for option in &self.options {
            validate_label(&option.label)?;
            if option.value.trim().is_empty() || option.value.len() > MAX_OPTION_VALUE_BYTES {
                return Err(io::Error::other("选项 value 必须非空且最多 64 字节").into());
            }
            if !values.insert(option.value.as_str()) {
                return Err(io::Error::other(format!("选项 value 重复：{}", option.value)).into());
            }
        }
        let value = self
            .value
            .as_ref()
            .ok_or_else(|| io::Error::other("创建或更新未挂载的选择控件时必须提供 value"))?;
        self.options
            .iter()
            .position(|option| &option.value == value)
            .ok_or_else(|| io::Error::other(format!("options 不包含 value {value}")).into())
    }
}

fn choice_row(label: String) -> impl Scene {
    bsn! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [Text(label) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)]
    }
}

fn group(presentation: ChoicePresentation, options: Vec<Choice>, initial: usize) -> Box<dyn Scene> {
    let radio = matches!(presentation, ChoicePresentation::RadioGroup);
    let options = options.into_iter().enumerate().map(move |(index, option)| {
        let Choice { value, label } = option;
        let shell: Box<dyn Scene> = if radio {
            Box::new(bsn! { @WidgetryRadioOption })
        } else {
            Box::new(bsn! { @WidgetrySegmentedOption })
        };
        let checked = (!radio && index == initial).then_some(bsn! { Checked });
        bsn! {
            @shell @{checked}
            template(move |_| Ok(ChoiceValue(value.clone())))
            Node { max_width: percent(100) }
            Children [Text(label) WidgetryText Pickable::IGNORE TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)]
        }
    }).collect::<Vec<_>>();
    let list = bevy::scene::RelatedScenes::<Children>::new(options);
    if radio {
        Box::new(bsn! {
            @WidgetryRadioGroup Name("NativeRadioGroup")
            Node { align_self: AlignSelf::Start, flex_direction: FlexDirection::Column, row_gap: px(4) }
            template(move |_| Ok(InitialChoice::Radio(initial)))
            on(group_changed) @list
        })
    } else {
        Box::new(bsn! {
            @WidgetrySegmented Name("NativeSegmentedControl")
            Node { align_self: AlignSelf::Start, max_width: percent(100), flex_wrap: FlexWrap::Wrap }
            on(group_changed) @list
        })
    }
}

fn combo(
    world: &mut World,
    parent: Entity,
    label: String,
    options: Vec<Choice>,
    initial: usize,
) -> std::result::Result<Entity, EcsError> {
    let mut model = WidgetryListModel::default();
    for option in options {
        model.push(option).map_err(construction_error)?;
    }
    let selected = model
        .id(initial)
        .ok_or_else(|| construction_error("缺少下拉框初始选项"))?;
    let source = spawn_scene(
        world,
        bsn! {
            @choice_row(label)
            ChildOf(parent)
        },
    )
    .map_err(construction_error)?;
    world.entity_mut(source).insert(model);
    let combo = spawn_scene(
        world,
        bsn! {
            @WidgetryComboBox::<Choice> {
                @source: source,
                @max_visible_items: 6_usize,
            @placeholder: {WidgetryComboBoxContent::new(|| {
                bsn_list! { Text("请选择") WidgetryText }
                })},
                @renderer: {WidgetryListViewRenderer::new(|option: &Choice| {
                    let label = option.label.clone();
                    let value = option.value.clone();
                    bsn_list! {
                        Text(label) WidgetryText TextLayout::no_wrap()
                    template(move |_| Ok(ChoiceValue(value.clone())))
                    }
                })},
            }
            Name("NativeChoiceComboBox") ChildOf(source)
            Node { width: px(240), max_width: percent(100) }
        template(move |_| Ok(InitialChoice::Combo(selected)))
            on(combo_changed)
        },
    );
    if let Err(error) = combo {
        world.despawn(source);
        return Err(construction_error(error));
    }
    Ok(source)
}

fn initialize_choice(pending: Query<(Entity, &InitialChoice)>, mut commands: Commands) {
    for (entity, initial) in &pending {
        match *initial {
            InitialChoice::Combo(id) => {
                WidgetryComboBox::<Choice>::select_id(&mut commands, entity, id)
            }
            InitialChoice::Radio(index) => {
                WidgetryRadioGroupHeadless::set_selected(&mut commands, entity, index)
            }
        }
        commands.entity(entity).remove::<InitialChoice>();
    }
}

fn boolean(properties: BooleanProperties) -> Result<Box<dyn Scene>> {
    validate_label(&properties.label)?;
    let BooleanProperties {
        label,
        presentation,
        value,
    } = properties;
    let value =
        value.ok_or_else(|| io::Error::other("创建或更新未挂载的布尔控件时必须提供 value"))?;
    let text = bsn_list! {
        Text(label) WidgetryText Pickable::IGNORE TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
    };
    Ok(match presentation {
        BooleanPresentation::CheckBox => {
            let checked = value.then_some(bsn! { Checked });
            Box::new(bsn! {
                @WidgetryCheckBox @{checked}
                Name("NativeCheckBox") TabIndex(0)
                Node { align_self: AlignSelf::Start, max_width: percent(100) }
                Children [{text}] on(check_changed)
            })
        }
        BooleanPresentation::ToggleButton => {
            let children = Box::new(text) as Box<dyn SceneList>;
            Box::new(bsn! {
                @WidgetryToggleButton { @selected: value, @children: {Some(children)} }
                Name("NativeToggleButton") TabIndex(0)
                Node { align_self: AlignSelf::Start, max_width: percent(100) }
                on(toggle_changed)
            })
        }
    })
}

fn prepare_choice(
    world: &World,
    root: Entity,
    mut properties: ChoiceProperties,
) -> Result<ChoiceProperties> {
    if properties.value.is_some() {
        return Ok(properties);
    }
    if let Some(model) = world.get::<WidgetryListModel<Choice>>(root) {
        let control = world
            .get::<Children>(root)
            .into_iter()
            .flatten()
            .copied()
            .find(|entity| world.get::<WidgetryComboBox<Choice>>(*entity).is_some())
            .ok_or_else(|| io::Error::other("原生选择场景缺少下拉控件"))?;
        let selected = match world.get::<InitialChoice>(control) {
            Some(InitialChoice::Combo(id)) => Some(*id),
            _ => WidgetryComboBox::<Choice>::selected_id(world, control)
                .map_err(|error| error.into_bevy_error())?,
        };
        properties.value = selected
            .and_then(|id| model.index_of(id))
            .and_then(|index| model.get(index))
            .map(|option| option.value.clone());
    } else {
        let control = world
            .get::<Children>(root)
            .into_iter()
            .flatten()
            .copied()
            .find(|entity| {
                world.get::<WidgetryRadioGroupHeadless>(*entity).is_some()
                    || world.get::<WidgetrySegmentedHeadless>(*entity).is_some()
            })
            .ok_or_else(|| io::Error::other("原生选择场景缺少选择组"))?;
        let children = world
            .get::<Children>(control)
            .ok_or_else(|| io::Error::other("原生选择组缺少选项"))?;
        let selected = match world.get::<InitialChoice>(control) {
            Some(InitialChoice::Radio(index)) => children.get(*index).copied(),
            _ => children
                .iter()
                .find(|entity| world.get::<Checked>(*entity).is_some()),
        };
        properties.value = selected
            .and_then(|entity| world.get::<ChoiceValue>(entity))
            .map(|value| value.0.clone());
    }
    Ok(properties)
}

fn prepare_boolean(
    world: &World,
    root: Entity,
    mut properties: BooleanProperties,
) -> Result<BooleanProperties> {
    properties
        .value
        .get_or_insert_with(|| world.get::<Checked>(root).is_some());
    Ok(properties)
}

pub(super) fn set_bound_boolean(world: &mut World, root: Entity, value: bool) -> Result {
    if world.get::<WidgetryToggleButton>(root).is_some() {
        bevy_widgetry::toggle_button::WidgetryToggleButtonHeadless::set_selected_in_world(
            world, root, value,
        )
        .map_err(|error| error.into_bevy_error())?;
    } else {
        let mut entity = world.get_entity_mut(root)?;
        if value {
            entity.insert(Checked);
        } else {
            entity.remove::<Checked>();
        }
    }
    Ok(())
}

pub(super) fn set_bound_choice(world: &mut World, root: Entity, value: &str) -> Result {
    let controls = world
        .get::<Children>(root)
        .ok_or_else(|| io::Error::other("原生选择场景缺少控件"))?;
    if let Some(model) = world.get::<WidgetryListModel<Choice>>(root) {
        let control = controls
            .iter()
            .find(|entity| world.get::<WidgetryComboBox<Choice>>(*entity).is_some())
            .ok_or_else(|| io::Error::other("原生选择场景缺少下拉控件"))?;
        let id = (0..model.len())
            .find(|index| {
                model
                    .get(*index)
                    .is_some_and(|option| option.value == value)
            })
            .and_then(|index| model.id(index))
            .ok_or_else(|| io::Error::other("下拉控件缺少绑定的选项"))?;
        WidgetryComboBox::<Choice>::select_id(&mut world.commands(), control, id);
    } else {
        let control = controls
            .iter()
            .find(|entity| {
                world.get::<WidgetryRadioGroupHeadless>(*entity).is_some()
                    || world.get::<WidgetrySegmentedHeadless>(*entity).is_some()
            })
            .ok_or_else(|| io::Error::other("原生选择场景缺少选择组"))?;
        let index = world
            .get::<Children>(control)
            .into_iter()
            .flatten()
            .position(|child| {
                world
                    .get::<ChoiceValue>(*child)
                    .is_some_and(|option| option.0 == value)
            })
            .ok_or_else(|| io::Error::other("选择组缺少绑定的选项"))?;
        if world.get::<WidgetryRadioGroupHeadless>(control).is_some() {
            WidgetryRadioGroupHeadless::set_selected(&mut world.commands(), control, index);
        } else {
            WidgetrySegmentedHeadless::set_selected(&mut world.commands(), control, index);
        }
    }
    Ok(())
}

fn group_changed(
    event: On<ValueChange<usize>>,
    groups: Query<&Children>,
    choices: Query<&ChoiceValue>,
    mut commands: Commands,
) {
    if let Ok(children) = groups.get(event.source)
        && let Some(option) = children.get(event.value)
        && let Ok(value) = choices.get(*option)
    {
        commands.trigger(DiscreteChange {
            source: event.source,
            value: DiscreteValue::Choice(Some(value.0.clone())),
            is_final: event.is_final,
        });
    }
}

fn combo_changed(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    controls: Query<&ChildOf>,
    models: Query<&WidgetryListModel<Choice>>,
    mut commands: Commands,
) {
    if let Ok(parent) = controls.get(event.source)
        && let Ok(model) = models.get(parent.parent())
    {
        let value = event
            .value
            .and_then(|id| model.index_of(id))
            .and_then(|index| model.get(index))
            .map(|option| option.value.clone());
        commands.trigger(DiscreteChange {
            source: event.source,
            value: DiscreteValue::Choice(value),
            is_final: event.is_final,
        });
    }
}

fn check_changed(event: On<ValueChange<bool>>, mut commands: Commands) {
    commands.trigger(DiscreteChange {
        source: event.source,
        value: DiscreteValue::Boolean(event.value),
        is_final: event.is_final,
    });
}

fn toggle_changed(event: On<WidgetryToggleButtonChanged>, mut commands: Commands) {
    commands.trigger(DiscreteChange {
        source: event.entity,
        value: DiscreteValue::Boolean(event.selected),
        is_final: true,
    });
}

fn validate_label(label: &str) -> Result {
    if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(io::Error::other("label 必须非空且最多 256 字节").into());
    }
    Ok(())
}

fn invalid_props(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}

fn construction_error(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
