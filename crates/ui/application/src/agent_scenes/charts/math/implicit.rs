use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        invalid,
        plot::{self, Content, Legend, LegendSymbol, Mark, Style},
    },
    Model, Variables,
    expression::{Expression, tex_number},
    level_grid::Grid,
    named_color, numeric_axis,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::CanvasPathCommand;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(super) const TEMPLATE: &str = "agentelier.implicit_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    curves: Vec<Curve>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Curve {
    name: String,
    expression: String,
    #[serde(default)]
    level: f64,
    color: Option<[u8; 3]>,
    domain: Option<Domain>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Domain {
    pub x: [f64; 2],
    pub y: [f64; 2],
}

struct ImplicitCurve {
    legend: Legend,
    expression: Expression,
    level: f64,
    bounds: Option<[DVec2; 2]>,
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    display: Domain,
    curves: Vec<ImplicitCurve>,
    variables: Variables,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw native contours for implicit expressions in two variables without solving for an explicit branch.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("curves", "1 to 8 unique named curves [{name,expression,level?,domain?,color?}]. Draw expression(x,y)=level, default 0. Use the same numeric grammar as function_plot with x and y, without an equals sign. Optional domain is {x:[min,max],y:[min,max]}. color is [R,G,B]."),
            ("parameters", "Optional map of finite numeric variables other than x and y. A composition can use math_parameter to bind a number to a declared variable."),
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
        let mut values = self.variables.values.clone();
        let slots = self.variables.tex();
        let mut formula = Vec::new();
        let columns = (steps / 3).clamp(40, 160);
        let rows = (columns * 2 / 3).max(28);
        let marks: Vec<_> = self
            .curves
            .iter()
            .map(|curve| {
                let expression = curve.expression.tex(&slots);
                formula.push(format!("$${expression}={}$$", tex_number(curve.level, 0)));
                let mut commands = Vec::new();
                if let Some(bounds) = curve.bounds {
                    let mut evaluate = |point| {
                        let actual = self.display.position(point);
                        values[0] = actual.x;
                        values[1] = actual.y;
                        let residual = curve.expression.evaluate(&values)? - curve.level;
                        residual.is_finite().then_some(residual)
                    };
                    let grid = Grid::sample(bounds, columns, rows, &mut evaluate);
                    for path in grid.contours(evaluate) {
                        append_contour(&mut commands, &path);
                    }
                }
                Mark {
                    legend: curve.legend.clone(),
                    symbol: LegendSymbol::Line,
                    commands,
                    paint: plot::stroke_paint(curve.legend.color, plot::LINE_WIDTH),
                }
            })
            .collect();
        if marks.iter().all(|mark| mark.commands.is_empty()) {
            formula.push("当前范围内没有可绘制的等值曲线。".into());
        }
        (
            Content {
                title: self.title.clone(),
                x: self.x.clone(),
                y: self.y.clone(),
                marks,
                baseline: None,
            },
            formula.join("\n\n"),
        )
    }

    fn variables_mut(&mut self) -> &mut Variables {
        &mut self.variables
    }
    fn style(&self) -> Style {
        Style::Implicit
    }
    fn formula_name(&self) -> &'static str {
        "ImplicitFormula"
    }
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let Properties {
            title,
            x_axis,
            y_axis,
            curves,
            parameters,
        } = props;
        let mut unique = BTreeSet::new();
        if !(1..=plot::PALETTE.len()).contains(&curves.len())
            || curves.iter().any(|curve| {
                curve.name.trim().is_empty()
                    || curve.name.len() > 96
                    || !unique.insert(curve.name.as_str())
            })
        {
            return Err(io::Error::other("隐式方程需要 1 至 8 条唯一命名的曲线").into());
        }
        let display = Domain::from_axes(&x_axis, &y_axis)?;
        let x = numeric_axis(x_axis, display.x)?;
        let y = numeric_axis(y_axis, display.y)?;
        let variables = Variables::new(&["x", "y"], parameters)?;
        let curves = curves
            .into_iter()
            .enumerate()
            .map(|(index, curve)| {
                if !curve.level.is_finite() {
                    return Err(io::Error::other("等值条件必须有限").into());
                }
                let expression = Expression::parse(&curve.expression, &variables.names)
                    .map_err(|error| io::Error::other(format!("曲线 {}：{error}", curve.name)))?;
                let bounds = curve
                    .domain
                    .unwrap_or(display)
                    .clip(display, x.range, y.range)?;
                Ok(ImplicitCurve {
                    legend: named_color(curve.name, curve.color, index),
                    expression,
                    level: curve.level,
                    bounds,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title,
            x,
            y,
            display,
            curves,
            variables,
        })
    }
}

impl Domain {
    pub(super) fn from_axes(x: &AxisSpec<f64>, y: &AxisSpec<f64>) -> Result<Self> {
        Ok(Self {
            x: x.range
                .ok_or_else(|| io::Error::other("数学绘图需要显式 X 轴范围"))?,
            y: y.range
                .ok_or_else(|| io::Error::other("数学绘图需要显式 Y 轴范围"))?,
        })
    }

    pub(super) fn clip(self, display: Self, x: Range, y: Range) -> Result<Option<[DVec2; 2]>> {
        Range::new(Some(self.x), [], AxisKind::Number)?;
        Range::new(Some(self.y), [], AxisKind::Number)?;
        let low = DVec2::new(self.x[0].max(display.x[0]), self.y[0].max(display.y[0]));
        let high = DVec2::new(self.x[1].min(display.x[1]), self.y[1].min(display.y[1]));
        Ok((low.x < high.x && low.y < high.y).then(|| {
            [
                DVec2::new(x.fraction(low.x), y.fraction(low.y)),
                DVec2::new(x.fraction(high.x), y.fraction(high.y)),
            ]
        }))
    }

    pub(super) fn position(self, point: DVec2) -> DVec2 {
        DVec2::new(self.x[0], self.y[0]) * (DVec2::ONE - point)
            + DVec2::new(self.x[1], self.y[1]) * point
    }
}

pub(super) fn append_contour(commands: &mut Vec<CanvasPathCommand>, points: &[DVec2]) {
    if points.len() >= 4 && points.first() == points.last() {
        let mut mapped: Vec<_> = points[..points.len() - 1]
            .iter()
            .copied()
            .map(plot::map)
            .collect();
        mapped.dedup();
        if let [first, rest @ ..] = mapped.as_slice()
            && rest.len() >= 2
        {
            commands.push(CanvasPathCommand::MoveTo(*first));
            commands.extend(rest.iter().copied().map(CanvasPathCommand::LineTo));
            commands.push(CanvasPathCommand::Close);
        }
    } else {
        plot::append_line(commands, points);
    }
}
