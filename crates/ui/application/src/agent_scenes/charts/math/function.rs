use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        invalid,
        plot::{self, Content, Legend, LegendSymbol, Mark, Style},
    },
    Model, Variables,
    expression::Expression,
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

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.function_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    curves: Vec<Curve>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    points: Vec<PointSeries>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Curve {
    name: String,
    expression: String,
    color: Option<[u8; 3]>,
    domain: Option<[f64; 2]>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PointSeries {
    name: String,
    color: Option<[u8; 3]>,
    points: Vec<[f64; 2]>,
}

struct CurveProgram {
    legend: Legend,
    expression: Expression,
    domain: [f64; 2],
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    x_domain: [f64; 2],
    curves: Vec<CurveProgram>,
    points: Vec<(Legend, Vec<[f64; 2]>)>,
    variables: Variables,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Plot explicit mathematical functions with chart axes and native typeset formulas. Bind numeric composition parameters locally.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("curves", "One or more [{name,expression,domain?,color?}]. Expression uses x, numeric parameters, pi/tau/e, operators + - * / ^, parentheses and functions sin, cos, tan, sqrt, abs, exp, ln, log10, min, max and related elementary functions. Use explicit multiplication and no equals sign. Domain is a finite increasing [min,max]; it defaults to the X display range. color is [R,G,B]."),
            ("points", "Optional independent point series [{name,points:[[x,y],...],color?}]. There may be up to 8 total curves and point series."),
            ("parameters", "Optional map from names such as a/b/c to finite numeric values. Declare each variable used by the curves. A composition can bind a number with kind math_parameter, parameter, and variable."),
        ],
        build,
        None,
        None,
    )?
    .with_conditional_ecs_scene_update(TEMPLATE, update)
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

impl Model for Prepared {
    fn projection(&self, steps: usize) -> (Content, String) {
        self.project(steps)
    }
    fn variables_mut(&mut self) -> &mut Variables {
        &mut self.variables
    }
    fn style(&self) -> Style {
        Style::Function
    }
    fn formula_name(&self) -> &'static str {
        "FunctionFormula"
    }
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let Properties {
            title,
            x_axis,
            y_axis,
            curves,
            points,
            parameters,
        } = props;
        if curves.is_empty() || curves.len() + points.len() > plot::PALETTE.len() {
            return Err(
                io::Error::other("函数图需要至少一条表达式，曲线和散点总数不超过 8").into(),
            );
        }
        let variables = Variables::new(&["x"], parameters)?;
        let mut unique = BTreeSet::new();
        if curves
            .iter()
            .map(|curve| &curve.name)
            .chain(points.iter().map(|points| &points.name))
            .any(|name| name.trim().is_empty() || name.len() > 96 || !unique.insert(name.as_str()))
        {
            return Err(io::Error::other("曲线和散点系列需要唯一的非空名称，最多 96 字节").into());
        }
        let x_domain = x_axis
            .range
            .ok_or_else(|| io::Error::other("函数图需要显式 X 轴范围"))?;
        let y_domain = y_axis
            .range
            .ok_or_else(|| io::Error::other("函数图需要显式 Y 轴范围"))?;
        let x = numeric_axis(x_axis, x_domain)?;
        let y = numeric_axis(y_axis, y_domain)?;
        let curves = curves
            .into_iter()
            .enumerate()
            .map(|(index, curve)| {
                let domain = curve.domain.unwrap_or(x_domain);
                Range::new(Some(domain), [], AxisKind::Number)?;
                let expression = Expression::parse(&curve.expression, &variables.names)
                    .map_err(|error| io::Error::other(format!("曲线 {}：{error}", curve.name)))?;
                Ok(CurveProgram {
                    legend: named_color(curve.name, curve.color, index),
                    expression,
                    domain,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let start = curves.len();
        let points = points
            .into_iter()
            .enumerate()
            .map(|(index, point)| {
                if point
                    .points
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
                {
                    return Err(io::Error::other("散点坐标必须有限").into());
                }
                Ok((
                    named_color(point.name, point.color, start + index),
                    point.points,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title,
            x,
            y,
            x_domain,
            curves,
            points,
            variables,
        })
    }

    fn project(&self, steps: usize) -> (Content, String) {
        let mut values = self.variables.values.clone();
        let x = self.x.range;
        let y = self.y.range;
        let mut marks: Vec<_> = self
            .curves
            .iter()
            .map(|curve| {
                let [left, right] = [
                    curve.domain[0].max(self.x_domain[0]),
                    curve.domain[1].min(self.x_domain[1]),
                ];
                let mut commands = Vec::new();
                if left <= right {
                    trace(&mut commands, [left, right], steps, |sample| {
                        values[0] = sample;
                        let observed = curve.expression.evaluate(&values)?;
                        let point = DVec2::new(x.fraction(sample), y.fraction(observed));
                        point.is_finite().then_some(point)
                    });
                }
                Mark {
                    legend: curve.legend.clone(),
                    symbol: LegendSymbol::Line,
                    commands,
                    paint: plot::stroke_paint(curve.legend.color, plot::LINE_WIDTH),
                }
            })
            .collect();
        let has_curve = marks.iter().any(|mark| !mark.commands.is_empty());
        for (legend, points) in &self.points {
            let mut commands = Vec::new();
            for &[point_x, point_y] in points {
                plot::append_marker(
                    &mut commands,
                    DVec2::new(x.fraction(point_x), y.fraction(point_y)),
                );
            }
            marks.push(Mark {
                legend: legend.clone(),
                symbol: LegendSymbol::Point,
                commands,
                paint: plot::stroke_paint(legend.color, 7.0),
            });
        }
        let mut source = self.formula();
        if !has_curve {
            source.push_str("\n\n当前范围内没有可绘制的曲线。");
        }
        let content = Content {
            title: self.title.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            marks,
            baseline: None,
        };
        (content, source)
    }

    fn formula(&self) -> String {
        let slots = self.variables.tex();
        self.curves
            .iter()
            .enumerate()
            .map(|(index, curve)| {
                let expression = curve.expression.tex(&slots);
                let name = if self.curves.len() == 1 {
                    "f".into()
                } else {
                    format!("f_{{{}}}", index + 1)
                };
                format!("$${name}(x)={expression}$$")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
