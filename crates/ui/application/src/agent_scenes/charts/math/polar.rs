use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        invalid,
        plot::{self, AngleUnit, Content, Legend, LegendSymbol, Mark, Style},
    },
    Model, Variables,
    expression::{Expression, tex_number},
    named_color, numeric_axis,
    trace::trace,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(super) const TEMPLATE: &str = "agentelier.polar_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    radial_axis: RadialAxis,
    angle_unit: AngleUnit,
    curves: Vec<Curve>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RadialAxis {
    label: String,
    unit: Option<String>,
    max: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Curve {
    name: String,
    expression: String,
    domain: [f64; 2],
    color: Option<[u8; 3]>,
}

struct PolarCurve {
    legend: Legend,
    expression: Expression,
    domain: [f64; 2],
    turns: f64,
}

struct Prepared {
    title: Option<String>,
    radius: Axis,
    angle: Axis,
    maximum: f64,
    unit: AngleUnit,
    curves: Vec<PolarCurve>,
    variables: Variables,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw native polar functions r(theta) with radial circles, labeled angle spokes and signed radii.",
        &[
            ("title", "Optional nonblank title."),
            ("radial_axis", "Required {label,unit?,max}, with finite positive max radial distance."),
            ("angle_unit", "Required radians or degrees. theta in each expression uses this unit. The numeric functions sin/cos/tan take radians, so in degree mode use e.g. cos(theta*pi/180)."),
            ("curves", "1 to 8 unique named curves [{name,expression,domain:[start,end],color?}]. Expressions use theta, declared parameters and the function_plot numeric grammar. Domain increases in the given unit, up to 32 turns. A negative radius plots in the opposite direction. color is [R,G,B]."),
            ("parameters", "Optional named finite numbers, reserving theta. A composition may bind them using math_parameter."),
        ],
        build, None, None,
    )?.with_conditional_ecs_scene_update(TEMPLATE, update)
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    super::mount(world, parent, Prepared::new(props).map_err(invalid)?)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    super::update(world, root, Prepared::new(props)?)
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let mut unique = BTreeSet::new();
        if !(1..=plot::PALETTE.len()).contains(&props.curves.len())
            || props.curves.iter().any(|curve| {
                curve.name.trim().is_empty()
                    || curve.name.len() > 96
                    || !unique.insert(curve.name.as_str())
            })
        {
            return Err(io::Error::other("极坐标图需要 1 至 8 条唯一命名的曲线").into());
        }
        let unit = props.angle_unit;
        let maximum = props.radial_axis.max;
        let radius = numeric_axis(
            AxisSpec {
                label: props.radial_axis.label,
                unit: props.radial_axis.unit,
                range: Some([0.0, maximum]),
            },
            [0.0, maximum],
        )?;
        let angle = numeric_axis(
            AxisSpec {
                label: "角度".into(),
                unit: Some(unit.name().into()),
                range: Some([0.0, unit.period()]),
            },
            [0.0, unit.period()],
        )?;
        let variables = Variables::new(&["theta"], props.parameters)?;
        let curves = props
            .curves
            .into_iter()
            .enumerate()
            .map(|(index, curve)| {
                Range::new(Some(curve.domain), [], AxisKind::Number)?;
                let turns = (curve.domain[1] - curve.domain[0]) / unit.period();
                if turns > 32.0 {
                    return Err(io::Error::other("极坐标角域最多跨越 32 周").into());
                }
                let expression = Expression::parse(&curve.expression, &variables.names)
                    .map_err(|error| io::Error::other(format!("曲线 {}：{error}", curve.name)))?;
                Ok(PolarCurve {
                    legend: named_color(curve.name, curve.color, index),
                    expression,
                    domain: curve.domain,
                    turns,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title: props.title,
            radius,
            angle,
            maximum,
            unit,
            curves,
            variables,
        })
    }
}

impl Model for Prepared {
    fn projection(&self, steps: usize) -> (Content, String) {
        let mut values = self.variables.values.clone();
        let slots = self.variables.tex();
        let mut formulas = Vec::new();
        let marks: Vec<_> = self
            .curves
            .iter()
            .enumerate()
            .map(|(index, curve)| {
                let mut commands = Vec::new();
                let steps = steps.max((curve.turns * 96.0).ceil() as usize);
                trace(&mut commands, curve.domain, steps, |theta| {
                    values[0] = theta;
                    let ratio = curve.expression.evaluate(&values)? / self.maximum;
                    if ratio.abs() > 1.0 {
                        return None;
                    }
                    Some(plot::polar_point(
                        DVec2::from_angle(self.unit.radians(theta)) * ratio,
                    ))
                });
                let symbol = if self.curves.len() == 1 {
                    "r".into()
                } else {
                    format!("r_{{{}}}", index + 1)
                };
                let domain = curve.domain.map(|bound| tex_number(bound, 0));
                let expression = curve.expression.tex(&slots);
                formulas.push(format!(
                    r"$${symbol}(\theta)={expression},\quad \theta\in[{},{}]$$",
                    domain[0], domain[1]
                ));
                Mark {
                    legend: curve.legend.clone(),
                    symbol: LegendSymbol::Line,
                    commands,
                    paint: plot::stroke_paint(curve.legend.color, plot::LINE_WIDTH),
                }
            })
            .collect();
        if marks.iter().all(|mark| mark.commands.is_empty()) {
            formulas.push("当前显示范围内没有可绘制的极坐标曲线。".into());
        }
        (
            Content {
                title: self.title.clone(),
                x: self.radius.clone(),
                y: self.angle.clone(),
                marks,
                baseline: None,
            },
            formulas.join("\n\n"),
        )
    }

    fn variables_mut(&mut self) -> &mut Variables {
        &mut self.variables
    }
    fn style(&self) -> Style {
        Style::Polar(self.unit)
    }
    fn formula_name(&self) -> &'static str {
        "PolarFormula"
    }
}
