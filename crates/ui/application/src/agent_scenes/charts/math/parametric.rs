use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        invalid,
        plot::{self, Content, Legend, LegendSymbol, Mark, Style},
    },
    Model, Variables,
    expression::{Expression, tex_number},
    implicit::Domain,
    named_color, numeric_axis,
    trace::trace,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::CanvasPathCommand;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(super) const TEMPLATE: &str = "agentelier.parametric_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    curves: Vec<Curve>,
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Curve {
    name: String,
    x: String,
    y: String,
    domain: [f64; 2],
    color: Option<[u8; 3]>,
    samples: Option<usize>,
}

struct Program {
    legend: Legend,
    x: Expression,
    y: Expression,
    domain: [f64; 2],
    samples: usize,
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    curves: Vec<Program>,
    cursor: Option<usize>,
    variables: Variables,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw native ordered parametric curves x(t), y(t), with an optional local cursor indicating the same t on each curve.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("curves", "1 to 8 unique named curves {name,x,y,domain:[min,max],samples?,color?}. x and y are numerical expressions in the same variable t and declared parameters. Domain is finite and increasing. Optional samples is a minimum from 24 to 2048, otherwise sampling adapts to the viewport. color is [R,G,B]."),
            ("parameters", "Optional map of finite numeric variables other than t. A composition can bind them via math_parameter."),
            ("cursor", "Optional name of a declared parameter whose value is interpreted as t and used to mark a point on each defined, visible curve. Bind that parameter via math_parameter to scrub a curve."),
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
            return Err(io::Error::other("参数图需要 1 至 8 条唯一命名的曲线").into());
        }
        let display = Domain::from_axes(&props.x_axis, &props.y_axis)?;
        let x = numeric_axis(props.x_axis, display.x)?;
        let y = numeric_axis(props.y_axis, display.y)?;
        let variables = Variables::new(&["t"], props.parameters)?;
        let cursor = props
            .cursor
            .as_ref()
            .map(|name| {
                variables
                    .names
                    .iter()
                    .enumerate()
                    .skip(1)
                    .find_map(|(index, variable)| (variable == name).then_some(index))
                    .ok_or_else(|| io::Error::other(format!("定位游标引用了未声明的参数 {name}")))
            })
            .transpose()?;
        let curves = props
            .curves
            .into_iter()
            .enumerate()
            .map(|(index, curve)| {
                Range::new(Some(curve.domain), [], AxisKind::Number)?;
                let samples = curve.samples.unwrap_or(24);
                if !(24..=2048).contains(&samples) {
                    return Err(io::Error::other("参数曲线采样数必须在 24 至 2048 之间").into());
                }
                let parse = |source: &str, axis| {
                    Expression::parse(source, &variables.names).map_err(|error| {
                        io::Error::other(format!("曲线 {} 的 {axis}(t)：{error}", curve.name))
                    })
                };
                let x = parse(&curve.x, "x")?;
                let y = parse(&curve.y, "y")?;
                Ok(Program {
                    legend: named_color(curve.name, curve.color, index),
                    x,
                    y,
                    domain: curve.domain,
                    samples,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title: props.title,
            x,
            y,
            curves,
            cursor,
            variables,
        })
    }
}

impl Model for Prepared {
    fn projection(&self, steps: usize) -> (Content, String) {
        let mut values = self.variables.values.clone();
        let slots = self.variables.tex();
        let cursor = self.cursor.map(|index| values[index]);
        let mut marks = Vec::new();
        let mut formulas = Vec::new();
        for curve in &self.curves {
            let mut commands = Vec::new();
            let mut evaluate = |t| {
                values[0] = t;
                let point = DVec2::new(
                    self.x.range.fraction(curve.x.evaluate(&values)?),
                    self.y.range.fraction(curve.y.evaluate(&values)?),
                );
                point.is_finite().then_some(point)
            };
            let domain = curve.domain.map(|bound| tex_number(bound, 0));
            let x = curve.x.tex(&slots);
            let y = curve.y.tex(&slots);
            formulas.push(format!(
                r"$$x(t)={x},\quad y(t)={y},\quad t\in[{},{}]$$",
                domain[0], domain[1]
            ));
            trace(
                &mut commands,
                curve.domain,
                steps.max(curve.samples),
                &mut evaluate,
            );
            if commands.is_empty() {
                formulas.push(format!(
                    "{} 在当前范围内没有可绘制的曲线。",
                    curve.legend.name
                ));
            } else if let (Some(first), Some(last)) =
                (evaluate(curve.domain[0]), evaluate(curve.domain[1]))
                && first.distance(last) <= 1e-12
            {
                close_visible(&mut commands, first);
            }
            let color = curve.legend.color;
            marks.push(Mark {
                legend: curve.legend.clone(),
                symbol: LegendSymbol::Line,
                commands,
                paint: plot::stroke_paint(color, plot::LINE_WIDTH),
            });
            if let Some(t) = cursor {
                let mut commands = Vec::new();
                if (curve.domain[0]..=curve.domain[1]).contains(&t)
                    && let Some(point) = evaluate(t)
                {
                    plot::append_marker(&mut commands, point);
                }
                marks.push(Mark {
                    legend: Legend {
                        name: format!("{} · t", curve.legend.name),
                        color,
                    },
                    symbol: LegendSymbol::Hidden,
                    commands,
                    paint: plot::stroke_paint(color, 8.0),
                });
            }
        }
        if let Some(t) = cursor {
            formulas.push(format!("$$t={}$$", tex_number(t, 0)));
        }
        (
            Content {
                title: self.title.clone(),
                x: self.x.clone(),
                y: self.y.clone(),
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
        Style::Parametric
    }
    fn formula_name(&self) -> &'static str {
        "ParametricFormula"
    }
}

fn close_visible(commands: &mut [CanvasPathCommand], point: DVec2) {
    use CanvasPathCommand::{Close, LineTo, MoveTo};
    if commands.len() < 4 {
        return;
    }
    let Some((last, prefix)) = commands.split_last_mut() else {
        return;
    };
    let mapped = plot::map(point);
    if let ([MoveTo(a), rest @ ..], LineTo(b)) = (prefix, &*last)
        && a.distance(mapped) < 0.001
        && b.distance(mapped) < 0.001
        && rest.iter().all(|step| matches!(step, LineTo(_)))
    {
        *last = Close;
    }
}
