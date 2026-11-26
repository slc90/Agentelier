use super::MAX_LABEL_BYTES;
use agentelier_agent_host::AgentHostPlugin;
use bevy::{prelude::*, ui_widgets::ValueChange};
use bevy_widgetry::{
    number_input::{WidgetryNumber, WidgetryNumberInput, WidgetryNumberInputPlugin},
    slider::{SliderDragEnded, WidgetrySlider, WidgetrySliderHeadless, WidgetrySliderPlugin},
    text::WidgetryText,
};
use serde::{Deserialize, Serialize};
use std::io;

const MAX_EXACT_SLIDER_INTEGER: i64 = 1 << 24;
const PARAMETERS: &[(&str, &str)] = &[
    (
        "label",
        "Required nonblank visible label, at most 256 UTF-8 bytes.",
    ),
    ("kind", "Required: integer or float."),
    ("min", "Required inclusive minimum, less than max."),
    ("max", "Required inclusive maximum."),
    (
        "step",
        "Required positive step of the requested numeric kind.",
    ),
    (
        "value",
        "Initial value in range. May be omitted when updating a mounted control of the same kind to preserve the current value if it is valid.",
    ),
];

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Properties<F> {
    Integer(Numeric<i64>),
    Float(Numeric<F>),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Numeric<T> {
    label: String,
    min: T,
    max: T,
    step: T,
    value: Option<T>,
}

#[derive(Reflect, Copy, Clone, Debug, PartialEq)]
pub(crate) enum NumericValue {
    Integer(i64),
    Float(f64),
}

trait EventValue {
    fn event_value(self) -> NumericValue;
}

impl EventValue for i64 {
    fn event_value(self) -> NumericValue {
        NumericValue::Integer(self)
    }
}

impl EventValue for f64 {
    fn event_value(self) -> NumericValue {
        NumericValue::Float(self)
    }
}

pub(crate) type NumericChange = ValueChange<NumericValue>;

#[derive(Component, Clone, Copy)]
enum Kind {
    Integer,
    Float,
}

#[derive(Component, Clone, Default)]
struct SliderValueText;

pub(super) fn install(app: &mut App) {
    app.add_plugins((WidgetrySliderPlugin, WidgetryNumberInputPlugin))
        .register_type::<NumericChange>();
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene(
        "agentelier.slider",
        "Create a stepped Widgetry slider. Floats use single precision. Integer sliders support values and spans up to 2^24. The value must be reachable by stepping from an endpoint.",
        PARAMETERS,
        slider,
        None,
        Some(prepare_slider),
    )?
    .with_ecs_scene(
        "agentelier.number_input",
        "Create an editable Widgetry Number Input with step buttons. Integers are signed 64-bit and floats use double precision.",
        PARAMETERS,
        number_input,
        None,
        Some(prepare_number),
    )
}

impl<T: WidgetryNumber> Numeric<T> {
    fn validated_value(&self) -> Result<T> {
        if self.label.trim().is_empty() || self.label.len() > MAX_LABEL_BYTES {
            return Err(io::Error::other("label 必须非空且最多 256 字节").into());
        }
        if !self.min.is_finite() || !self.max.is_finite() || self.min >= self.max {
            return Err(io::Error::other("min 和 max 必须有限且 min 小于 max").into());
        }
        if !self.step.is_finite() || self.step <= T::ZERO {
            return Err(io::Error::other("step 必须是有限正数").into());
        }
        let value = self.value.ok_or_else(|| {
            io::Error::other("创建控件或更改数值类型时必须提供 value，未挂载的控件也需要 value")
        })?;
        if !value.is_finite() || value < self.min || value > self.max {
            return Err(io::Error::other(format!(
                "value {value} 不在 [{}, {}] 范围内",
                self.min, self.max
            ))
            .into());
        }
        Ok(value)
    }
}

fn slider(properties: Properties<f32>) -> Result<impl Scene> {
    let (numeric, kind, value_text) = match properties {
        Properties::Integer(integer) => {
            let value = integer.validated_value()?;
            if integer.min < -MAX_EXACT_SLIDER_INTEGER
                || integer.max > MAX_EXACT_SLIDER_INTEGER
                || integer.max - integer.min > MAX_EXACT_SLIDER_INTEGER
                || integer.step > MAX_EXACT_SLIDER_INTEGER
            {
                return Err(io::Error::other("整数滑块的数值和跨度不得超过 2^24").into());
            }
            (
                Numeric {
                    label: integer.label,
                    min: integer.min as f32,
                    max: integer.max as f32,
                    step: integer.step as f32,
                    value: Some(value as f32),
                },
                Kind::Integer,
                value.to_string(),
            )
        }
        Properties::Float(float) => {
            let value = float.validated_value()?;
            (float, Kind::Float, value.to_string())
        }
    };
    let Numeric {
        label,
        min,
        max,
        step,
        value,
    } = numeric;
    if !(max - min).is_finite() || step > max - min || min + step == min || max - step == max {
        return Err(io::Error::other("step 必须能在滑块范围内产生可表示的步进").into());
    }
    let control = bsn! {
        @WidgetrySlider { @min: min, @max: max, @step: {Some(step)}, @value: value }
        Name("NativeNumericSlider") template(move |_| Ok(kind))
        Node { width: percent(100), max_width: px(320) }
        on(slider_changed)
        on(slider_ended)
    };
    Ok(bsn! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [
            Node { width: percent(100), min_width: px(0), max_width: px(320), justify_content: JustifyContent::SpaceBetween, column_gap: px(12) }
            Children [
                Text(label) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)--
                Text(value_text) WidgetryText SliderValueText Name("NativeSliderValue")
            ]--
            @control
        ]
    })
}

fn number_input(properties: Properties<f64>) -> Result<impl Scene> {
    let (label, control): (_, Box<dyn Scene>) = match properties {
        Properties::Integer(numeric) => {
            let value = numeric.validated_value()?;
            let scene = number_control(&numeric, value, Kind::Integer);
            (numeric.label, Box::new(scene))
        }
        Properties::Float(numeric) => {
            let value = numeric.validated_value()?;
            if numeric.min + numeric.step == numeric.min
                || numeric.max - numeric.step == numeric.max
            {
                return Err(io::Error::other("step 必须能产生可表示的数值变化").into());
            }
            let scene = number_control(&numeric, value, Kind::Float);
            (numeric.label, Box::new(scene))
        }
    };
    Ok(bsn! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [
            Text(label) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)--
            @control
        ]
    })
}

fn number_control<T: WidgetryNumber + EventValue>(
    numeric: &Numeric<T>,
    value: T,
    kind: Kind,
) -> impl Scene + use<T> {
    let Numeric { min, max, step, .. } = *numeric;
    bsn! {
        @WidgetryNumberInput<T> { @min: {Some(min)}, @max: {Some(max)}, @step: {step}, @value: {Some(value)} }
        Name("NativeNumberInput") template(move |_| Ok(kind))
        on(number_changed::<T>)
    }
}

fn prepare_slider(
    world: &World,
    root: Entity,
    properties: Properties<f32>,
) -> Result<Properties<f32>> {
    let control = numeric_control(world, root)?;
    let kind = world.get::<Kind>(control);
    Ok(match properties {
        Properties::Integer(mut numeric) => {
            if numeric.value.is_none() && matches!(kind, Some(Kind::Integer)) {
                let value = WidgetrySliderHeadless::value(world, control)
                    .map_err(|error| error.into_bevy_error())?;
                numeric.value = Some(value as i64);
            }
            Properties::Integer(numeric)
        }
        Properties::Float(mut numeric) => {
            if numeric.value.is_none() && matches!(kind, Some(Kind::Float)) {
                numeric.value = Some(
                    WidgetrySliderHeadless::value(world, control)
                        .map_err(|error| error.into_bevy_error())?,
                );
            }
            Properties::Float(numeric)
        }
    })
}

fn prepare_number(
    world: &World,
    root: Entity,
    properties: Properties<f64>,
) -> Result<Properties<f64>> {
    let control = numeric_control(world, root)?;
    let kind = world.get::<Kind>(control);
    Ok(match properties {
        Properties::Integer(mut numeric) => {
            if numeric.value.is_none() && matches!(kind, Some(Kind::Integer)) {
                numeric.value = Some(
                    WidgetryNumberInput::<i64>::value(world, control)
                        .map_err(|error| error.into_bevy_error())?,
                );
            }
            Properties::Integer(numeric)
        }
        Properties::Float(mut numeric) => {
            if numeric.value.is_none() && matches!(kind, Some(Kind::Float)) {
                numeric.value = Some(
                    WidgetryNumberInput::<f64>::value(world, control)
                        .map_err(|error| error.into_bevy_error())?,
                );
            }
            Properties::Float(numeric)
        }
    })
}

pub(super) fn numeric_control(world: &World, root: Entity) -> Result<Entity> {
    world
        .get::<Children>(root)
        .into_iter()
        .flatten()
        .copied()
        .find(|child| world.get::<Kind>(*child).is_some())
        .ok_or_else(|| io::Error::other("原生数值场景缺少编辑控件").into())
}

fn slider_changed(
    event: On<ValueChange<f32>>,
    controls: Query<(&Kind, &ChildOf)>,
    children: Query<&Children>,
    mut texts: Query<&mut Text, With<SliderValueText>>,
    mut commands: Commands,
) {
    let Ok((kind, parent)) = controls.get(event.source) else {
        return;
    };
    let (value, display) = slider_value(*kind, event.value);
    if let Some(text) = children
        .iter_descendants(parent.parent())
        .find(|child| texts.contains(*child))
        && let Ok(mut text) = texts.get_mut(text)
    {
        text.set_if_neq(Text(display));
    }
    commands.trigger(NumericChange {
        source: event.source,
        value,
        is_final: event.is_final,
    });
}

fn slider_ended(event: On<SliderDragEnded>, controls: Query<&Kind>, mut commands: Commands) {
    if let Ok(&kind) = controls.get(event.entity) {
        let (value, _) = slider_value(kind, event.value);
        commands.trigger(NumericChange {
            source: event.entity,
            value,
            is_final: true,
        });
    }
}

fn slider_value(kind: Kind, raw: f32) -> (NumericValue, String) {
    match kind {
        Kind::Integer => {
            let value = raw as i64;
            (NumericValue::Integer(value), value.to_string())
        }
        Kind::Float => {
            let display = raw.to_string();
            let value = display.parse::<f64>().unwrap_or(f64::from(raw));
            (NumericValue::Float(value), display)
        }
    }
}

fn number_changed<T: WidgetryNumber + EventValue>(
    event: On<ValueChange<T>>,
    mut commands: Commands,
) {
    commands.trigger(NumericChange {
        source: event.source,
        value: event.value.event_value(),
        is_final: event.is_final,
    });
}
