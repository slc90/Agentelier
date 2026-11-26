use agentelier_agent_host::{MountedScene, SceneDefinition, SceneState, SceneStateWrites};
use bevy::{prelude::*, ui_widgets::Activate};
use bevy_widgetry::{
    number_input::WidgetryNumberInput, slider::WidgetrySliderHeadless,
    text_field::WidgetryTextFieldHeadless,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, io};

use super::{
    charts::{bars, heatmap, scatter, select_series},
    discrete::{DiscreteChange, DiscreteValue},
    numeric::{NumericChange, NumericValue},
    text_input::TextChange,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Definition {
    domain: Domain,
    default: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<Json>,
}

impl Definition {
    pub(super) fn restore_current(&mut self, name: &str, value: &Json) -> Result {
        if self.domain.decode(value).is_none() {
            return Err(parameter_error(name, "保存的当前值超出定义"));
        }
        self.current = Some(value.clone());
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Domain {
    Integer { min: i64, max: i64, step: i64 },
    Float { min: f64, max: f64, step: f64 },
    Boolean,
    Choice { options: Vec<Choice> },
    Text { max_length: usize },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    value: String,
    label: String,
}

#[derive(Reflect, Clone, Debug, PartialEq)]
pub(super) enum Value {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Choice(String),
    Text(String),
}

impl Value {
    fn json(&self) -> Json {
        match self {
            Self::Integer(value) => json!(value),
            Self::Float(value) => json!(value),
            Self::Boolean(value) => json!(value),
            Self::Choice(value) | Self::Text(value) => json!(value),
        }
    }

    fn display(&self, format: TextFormat) -> String {
        match (self, format) {
            (Self::Integer(value), TextFormat::Square) => (i128::from(*value).pow(2)).to_string(),
            (Self::Float(value), TextFormat::Square) => (value * value).to_string(),
            (Self::Integer(value), TextFormat::Value) => value.to_string(),
            (Self::Float(value), TextFormat::Value) => value.to_string(),
            (Self::Boolean(value), TextFormat::Value) => value.to_string(),
            (Self::Choice(value) | Self::Text(value), TextFormat::Value) => value.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Binding {
    Input {
        parameter: String,
    },
    Text {
        parameter: String,
        #[serde(default)]
        format: TextFormat,
    },
    Visible {
        parameter: String,
    },
    SeriesFilter {
        parameter: String,
        all: Option<String>,
    },
    CategoryFilter {
        parameter: String,
        all: Option<String>,
    },
    Reset,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum TextFormat {
    #[default]
    Value,
    Square,
}

#[derive(Clone)]
struct Parameter {
    domain: Domain,
    default: Value,
    current: Value,
}

#[derive(Component, Default)]
pub(super) struct Parameters {
    values: BTreeMap<String, Parameter>,
    parts: Vec<Entity>,
}

#[derive(Component)]
pub(super) struct BoundPart {
    owner: Entity,
    bindings: Vec<Binding>,
}

#[derive(Clone, EntityEvent, Reflect)]
#[reflect(Event)]
pub(crate) struct ParameterChanged {
    pub entity: Entity,
    pub name: String,
    pub value: Value,
}

impl BoundPart {
    pub(super) fn new(owner: Entity, bindings: Vec<Binding>) -> Self {
        Self { owner, bindings }
    }

    fn input(&self) -> Option<&str> {
        self.bindings.iter().find_map(|binding| match binding {
            Binding::Input { parameter } => Some(parameter.as_str()),
            _ => None,
        })
    }
}

impl Parameters {
    pub(super) fn new(definitions: BTreeMap<String, Definition>) -> Result<Self> {
        let mut parameters = BTreeMap::new();
        for (name, definition) in definitions {
            if name.trim().is_empty() || name.len() > 64 {
                return Err(parameter_error(&name, "名称必须非空且最多 64 字节"));
            }
            let default = definition
                .domain
                .decode(&definition.default)
                .ok_or_else(|| parameter_error(&name, "default 无效"))?;
            let current = match definition.current {
                Some(current) => definition
                    .domain
                    .decode(&current)
                    .ok_or_else(|| parameter_error(&name, "current 无效"))?,
                None => default.clone(),
            };
            parameters.insert(
                name,
                Parameter {
                    domain: definition.domain,
                    default,
                    current,
                },
            );
        }
        Ok(Self {
            values: parameters,
            parts: Vec::new(),
        })
    }

    pub(super) fn retain(&self, next: &mut BTreeMap<String, Definition>) -> Result {
        for (name, definition) in next {
            if let Some(previous) = self.values.get(name)
                && definition.current.is_none()
            {
                if !definition.domain.accepts(&previous.current) {
                    return Err(parameter_error(
                        name,
                        "旧值超出新定义，必须同时提供有效 current",
                    ));
                }
                definition.current = Some(previous.current.json());
            }
        }
        Ok(())
    }

    fn get(&self, name: &str) -> Result<&Parameter> {
        self.values
            .get(name)
            .ok_or_else(|| parameter_error(name, "未定义"))
    }

    pub(super) fn can_reuse(&self, previous: &Self, bindings: &[Binding]) -> bool {
        bindings.iter().all(|binding| match binding {
            Binding::Input { parameter }
            | Binding::Text { parameter, .. }
            | Binding::SeriesFilter { parameter, .. }
            | Binding::CategoryFilter { parameter, .. } => {
                self.values.get(parameter).map(|v| &v.current)
                    == previous.values.get(parameter).map(|v| &v.current)
            }
            _ => true,
        })
    }

    pub(super) fn specialize(&self, scene: &mut SceneDefinition, bindings: &[Binding]) -> Result {
        let mut input = false;
        let mut text = false;
        let mut visible = false;
        let mut series_filter = false;
        let mut category_filter = false;
        let mut reset = false;
        for binding in bindings {
            match binding {
                Binding::Input { parameter } if !input => {
                    input = true;
                    self.get(parameter)?.bind_input(parameter, scene)?;
                }
                Binding::Text { parameter, format }
                    if !text && scene.template == "agentelier.text" =>
                {
                    text = true;
                    let value = &self.get(parameter)?.current;
                    if matches!(format, TextFormat::Square)
                        && !matches!(value, Value::Integer(_) | Value::Float(_))
                    {
                        return Err(parameter_error(parameter, "平方输出需要数值"));
                    }
                    scene
                        .props
                        .insert("text".into(), json!(text_value(parameter, value, *format)));
                }
                Binding::Visible { parameter } if !visible => {
                    visible = true;
                    if !matches!(self.get(parameter)?.current, Value::Boolean(_)) {
                        return Err(parameter_error(parameter, "显隐需要布尔值"));
                    }
                }
                Binding::SeriesFilter { parameter, all }
                | Binding::CategoryFilter { parameter, all } => {
                    let by_category = matches!(binding, Binding::CategoryFilter { .. });
                    let (seen, property) = if by_category {
                        let property = if scene.template == heatmap::TEMPLATE {
                            "selected_row"
                        } else {
                            "selected_category"
                        };
                        (&mut category_filter, property)
                    } else {
                        (&mut series_filter, "selected_series")
                    };
                    if *seen {
                        return Err(parameter_error(parameter, "筛选绑定重复"));
                    }
                    *seen = true;
                    let Parameter {
                        domain: Domain::Choice { options },
                        current: Value::Choice(current),
                        ..
                    } = self.get(parameter)?
                    else {
                        return Err(parameter_error(parameter, "图表筛选需要选择值"));
                    };
                    let selected = (all.as_ref() != Some(current)).then_some(current);
                    let values = options.iter().map(|option| option.value.as_str());
                    scene.props.insert(property.into(), json!(selected));
                    match scene.template.as_str() {
                        bars::TEMPLATE => {
                            bars::validate_choices(scene, values, all.as_deref(), by_category)?
                        }
                        heatmap::TEMPLATE if by_category => {
                            heatmap::validate_choices(scene, values, all.as_deref())?
                        }
                        scatter::TEMPLATE if !by_category => {
                            scatter::validate_choices(scene, values, all.as_deref())?
                        }
                        _ => return Err(parameter_error(parameter, "图表模板不支持该筛选")),
                    }
                }
                Binding::Reset if !reset && scene.template == "agentelier.button" => reset = true,
                _ => {
                    return Err(io::Error::other(format!(
                        "模板 {} 的参数绑定不匹配或重复",
                        scene.template
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn attach(
        mut self,
        world: &mut World,
        owner: Entity,
        parts: impl IntoIterator<Item = (Entity, Vec<Binding>)>,
    ) {
        for (root, bindings) in parts {
            for binding in &bindings {
                if let Binding::Visible { parameter } = binding
                    && let Some(Parameter {
                        current: Value::Boolean(value),
                        ..
                    }) = self.values.get(parameter)
                {
                    set_visible(world, root, *value);
                }
            }
            self.parts.push(root);
            world
                .entity_mut(root)
                .insert(BoundPart::new(owner, bindings));
        }
        world.entity_mut(owner).insert(self);
    }
}

impl Parameter {
    fn bind_input(&self, name: &str, scene: &mut SceneDefinition) -> Result {
        let config = match (&self.domain, scene.template.as_str()) {
            (
                Domain::Integer { min, max, step },
                "agentelier.slider" | "agentelier.number_input",
            ) => [
                ("kind", json!("integer")),
                ("min", json!(min)),
                ("max", json!(max)),
                ("step", json!(step)),
            ]
            .to_vec(),
            (Domain::Float { min, max, step }, "agentelier.slider" | "agentelier.number_input") => {
                [
                    ("kind", json!("float")),
                    ("min", json!(min)),
                    ("max", json!(max)),
                    ("step", json!(step)),
                ]
                .to_vec()
            }
            (Domain::Boolean, "agentelier.boolean") => Vec::new(),
            (Domain::Choice { options }, "agentelier.choice") => vec![("options", json!(options))],
            (Domain::Text { max_length }, "agentelier.text_input") => {
                vec![("max_length", json!(max_length))]
            }
            _ => return Err(parameter_error(name, "类型与输入控件不匹配")),
        };
        scene
            .props
            .extend(config.into_iter().map(|(key, value)| (key.into(), value)));
        scene.props.insert("value".into(), self.current.json());
        Ok(())
    }
}

fn text_value(name: &str, value: &Value, format: TextFormat) -> String {
    let suffix = if matches!(format, TextFormat::Square) {
        "²"
    } else {
        ""
    };
    format!("{name}{suffix} = {}", value.display(format))
}

impl Domain {
    fn decode(&self, value: &Json) -> Option<Value> {
        let value = match self {
            Self::Integer { .. } => value.as_i64().map(Value::Integer),
            Self::Float { .. } => value.as_f64().map(Value::Float),
            Self::Boolean => value.as_bool().map(Value::Boolean),
            Self::Choice { .. } => value.as_str().map(|v| Value::Choice(v.to_owned())),
            Self::Text { .. } => value.as_str().map(|v| Value::Text(v.to_owned())),
        }?;
        self.accepts(&value).then_some(value)
    }

    fn accepts(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Integer { min, max, step }, Value::Integer(value)) => {
                *step > 0
                    && min < max
                    && value >= min
                    && value <= max
                    && ((i128::from(*value) - i128::from(*min)) % i128::from(*step) == 0
                        || (i128::from(*max) - i128::from(*value)) % i128::from(*step) == 0)
            }
            (Self::Float { min, max, step }, Value::Float(value)) => {
                min.is_finite()
                    && max.is_finite()
                    && step.is_finite()
                    && value.is_finite()
                    && min < max
                    && *step > 0.0
                    && value >= min
                    && value <= max
                    && (reachable(*value, *min, *step) || reachable(*value, *max, *step))
            }
            (Self::Boolean, Value::Boolean(_)) => true,
            (Self::Choice { options }, Value::Choice(value)) => {
                options.iter().any(|option| option.value == *value)
            }
            (Self::Text { max_length }, Value::Text(value)) => {
                (1..=4096).contains(max_length)
                    && value.chars().all(super::text_input::single_line)
                    && value.chars().take(max_length + 1).count() <= *max_length
            }
            _ => false,
        }
    }
}

fn reachable(value: f64, endpoint: f64, step: f64) -> bool {
    let distance = (value - endpoint).abs();
    let count = (distance / step).round();
    (distance - count * step).abs()
        <= (f64::from(f32::EPSILON) * distance.max(step) * 4.0).min(step * 0.001)
}

fn parameter_error(name: &str, details: &str) -> BevyError {
    io::Error::other(format!("参数 {name}：{details}")).into()
}

pub(super) fn install(app: &mut App) {
    app.register_type::<ParameterChanged>()
        .add_observer(on_number)
        .add_observer(on_discrete)
        .add_observer(on_text)
        .add_observer(on_reset)
        .add_observer(schedule_save);
}

fn schedule_save(
    event: On<ParameterChanged>,
    mounted: Query<(&MountedScene, &Parameters)>,
    mut writes: ResMut<SceneStateWrites>,
) {
    let Ok((mount, parameters)) = mounted.get(event.entity) else {
        return;
    };
    let state = SceneState {
        revision: mount.revision,
        values: parameters
            .values
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.current.json()))
            .collect(),
    };
    writes.schedule(event.entity, *mount, state);
}

fn on_number(event: On<NumericChange>, mut commands: Commands) {
    let value = match event.value {
        NumericValue::Integer(value) => Value::Integer(value),
        NumericValue::Float(value) => Value::Float(value),
    };
    queue_change(&mut commands, event.source, value);
}

fn on_discrete(event: On<DiscreteChange>, mut commands: Commands) {
    let value = match &event.value {
        DiscreteValue::Boolean(value) => Value::Boolean(*value),
        DiscreteValue::Choice(Some(value)) => Value::Choice(value.clone()),
        DiscreteValue::Choice(None) => return,
    };
    queue_change(&mut commands, event.source, value);
}

fn on_text(event: On<TextChange>, mut commands: Commands) {
    queue_change(
        &mut commands,
        event.source,
        Value::Text(event.value.clone()),
    );
}

fn queue_change(commands: &mut Commands, source: Entity, value: Value) {
    commands.queue(move |world: &mut World| -> Result {
        let Some((root, part)) = owning_part(world, source) else {
            return Ok(());
        };
        let Some(name) = part.input().map(str::to_owned) else {
            return Ok(());
        };
        let owner = part.owner;
        change(world, owner, &name, value, Some(root))
    });
}

fn on_reset(event: On<Activate>, mut commands: Commands) {
    let source = event.entity;
    commands.queue(move |world: &mut World| -> Result {
        let Some((_, part)) = owning_part(world, source) else {
            return Ok(());
        };
        if !part
            .bindings
            .iter()
            .any(|binding| matches!(binding, Binding::Reset))
        {
            return Ok(());
        }
        let owner = part.owner;
        let Some(parameters) = world.get::<Parameters>(owner) else {
            return Ok(());
        };
        let defaults: Vec<_> = parameters
            .values
            .iter()
            .filter(|(_, parameter)| parameter.default != parameter.current)
            .map(|(name, parameter)| (name.clone(), parameter.default.clone()))
            .collect();
        for (name, value) in defaults {
            change(world, owner, &name, value, None)?;
        }
        Ok(())
    });
}

fn owning_part(world: &World, mut entity: Entity) -> Option<(Entity, &BoundPart)> {
    loop {
        if let Some(part) = world.get::<BoundPart>(entity) {
            return Some((entity, part));
        }
        entity = world.get::<ChildOf>(entity)?.parent();
    }
}

fn change(
    world: &mut World,
    owner: Entity,
    name: &str,
    value: Value,
    source: Option<Entity>,
) -> Result {
    let source_is_slider = source
        .filter(|_| matches!(value, Value::Float(_)))
        .and_then(|entity| super::numeric::numeric_control(world, entity).ok())
        .is_some_and(|entity| world.get::<WidgetrySliderHeadless>(entity).is_some());
    let Some(mut parameters) = world.get_mut::<Parameters>(owner) else {
        return Ok(());
    };
    let Some(parameter) = parameters.values.get_mut(name) else {
        return Ok(());
    };
    if value == parameter.current
        || source_is_slider
            && matches!((&value, &parameter.current), (Value::Float(new), Value::Float(current)) if *new as f32 == *current as f32)
    {
        return Ok(());
    }
    if !parameter.domain.accepts(&value) {
        if let Some(source) = source {
            let current = parameter.current.clone();
            project_input(world, source, &current)?;
            return Ok(());
        }
        return Err(parameter_error(name, "当前值不符合参数定义"));
    }
    let previous = std::mem::replace(&mut parameter.current, value.clone());
    let parts = parameters.parts.clone();
    if let Err(error) = project(world, &parts, name, &value, source) {
        if let Some(mut parameters) = world.get_mut::<Parameters>(owner)
            && let Some(parameter) = parameters.values.get_mut(name)
        {
            parameter.current = previous.clone();
        }
        project(world, &parts, name, &previous, None)?;
        return Err(error);
    }
    world.trigger(ParameterChanged {
        entity: owner,
        name: name.to_owned(),
        value,
    });
    Ok(())
}

fn project(
    world: &mut World,
    parts: &[Entity],
    name: &str,
    value: &Value,
    source: Option<Entity>,
) -> Result {
    for &root in parts {
        let Some(part) = world.get::<BoundPart>(root) else {
            continue;
        };
        let bindings = part.bindings.clone();
        for binding in bindings {
            match binding {
                Binding::Input { parameter } if parameter == name && source != Some(root) => {
                    project_input(world, root, value)?;
                }
                Binding::Text { parameter, format } if parameter == name => {
                    if let Some(mut text) = world.get_mut::<Text>(root) {
                        text.set_if_neq(Text(text_value(name, value, format)));
                    }
                }
                Binding::Visible { parameter } if parameter == name => {
                    if let Value::Boolean(value) = value {
                        set_visible(world, root, *value);
                    }
                }
                Binding::SeriesFilter { parameter, all } if parameter == name => {
                    if let Value::Choice(value) = value {
                        let selected = (all.as_ref() != Some(value)).then_some(value.as_str());
                        select_series(world, root, selected)?;
                    }
                }
                Binding::CategoryFilter { parameter, all } if parameter == name => {
                    if let Value::Choice(value) = value {
                        let selected = (all.as_ref() != Some(value)).then_some(value.as_str());
                        if world.get::<heatmap::Heatmap>(root).is_some() {
                            heatmap::filter_row(world, root, selected)?;
                        } else {
                            bars::filter_category(world, root, selected)?;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn project_input(world: &mut World, root: Entity, value: &Value) -> Result {
    match value {
        Value::Integer(value) => {
            let control = super::numeric::numeric_control(world, root)?;
            if world.get::<WidgetrySliderHeadless>(control).is_some() {
                WidgetrySliderHeadless::set_value_in_world(world, control, *value as f32)
                    .map_err(|error| error.into_bevy_error())?;
            } else {
                WidgetryNumberInput::<i64>::set_value_in_world(world, control, *value)
                    .map_err(|error| error.into_bevy_error())?;
            }
        }
        Value::Float(value) => {
            let control = super::numeric::numeric_control(world, root)?;
            if world.get::<WidgetrySliderHeadless>(control).is_some() {
                WidgetrySliderHeadless::set_value_in_world(world, control, *value as f32)
                    .map_err(|error| error.into_bevy_error())?;
            } else {
                WidgetryNumberInput::<f64>::set_value_in_world(world, control, *value)
                    .map_err(|error| error.into_bevy_error())?;
            }
        }
        Value::Boolean(value) => super::discrete::set_bound_boolean(world, root, *value)?,
        Value::Choice(value) => super::discrete::set_bound_choice(world, root, value)?,
        Value::Text(value) => {
            let control = super::text_input::control(world, root)?;
            WidgetryTextFieldHeadless::set_text_in_world(world, control, value)
                .map_err(|error| error.into_bevy_error())?;
        }
    }
    Ok(())
}

fn set_visible(world: &mut World, root: Entity, show: bool) {
    if let Some(mut visibility) = world.get_mut::<Visibility>(root) {
        visibility.set_if_neq(if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}
