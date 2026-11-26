use super::{
    super::{
        axis::{Axis, AxisSpec},
        invalid,
        plot::{self, Content, Legend, LegendSymbol, Mark, Style},
    },
    Model, Variables,
    expression::{Expression, tex_number},
    implicit::{Domain, append_contour},
    level_grid::Grid,
    named_color, numeric_axis, region_mesh,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::{CanvasFill, CanvasPaint, CanvasPathCommand};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(super) const TEMPLATE: &str = "agentelier.inequality_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    regions: Vec<Region>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Region {
    name: String,
    domain: Option<Domain>,
    color: Option<[u8; 3]>,
    conditions: Vec<Condition>,
    combine: Option<Combine>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    expression: String,
    comparison: Comparison,
    #[serde(default)]
    level: f64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Comparison {
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Combine {
    All,
    Any,
}

struct Constraint {
    expression: Expression,
    comparison: Comparison,
    level: f64,
}

struct PreparedRegion {
    legend: Legend,
    bounds: Option<[DVec2; 2]>,
    constraints: Vec<Constraint>,
    combine: Combine,
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    display: Domain,
    regions: Vec<PreparedRegion>,
    variables: Variables,
}

#[derive(Clone, Copy)]
struct Score {
    value: f64,
    strict: bool,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Fill native mathematical inequality regions and draw their included or excluded boundaries.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("regions", "1 to 4 unique named regions {name,domain?,color?,conditions,combine?}. Optional domain is {x:[min,max],y:[min,max]}. color is [R,G,B]. Each condition {expression,comparison,level?} compares a numeric expression in x and y to a finite level (default 0). comparison is lt, le, gt or ge. For 2 to 4 conditions explicitly supply combine: all or any. Undefined values are excluded."),
            ("parameters", "Optional map of finite numeric variables other than x and y. A composition can bind a number to a declared variable with math_parameter."),
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

impl Model for Prepared {
    fn projection(&self, steps: usize) -> (Content, String) {
        let mut values = self.variables.values.clone();
        let slots = self.variables.tex();
        let columns = (steps / 4).clamp(32, 112);
        let rows = (columns * 2 / 3).max(26);
        let mut formula = Vec::new();
        let mut marks = Vec::new();
        for region in &self.regions {
            formula.push(region.formula(&slots));
            let mut fill = Vec::new();
            let mut boundary = Vec::new();
            if let Some(bounds) = region.bounds {
                let evaluate = |point, values: &mut [f64]| {
                    let actual = self.display.position(point);
                    values[0] = actual.x;
                    values[1] = actual.y;
                    region.score(values)
                };
                let grid = Grid::sample(bounds, columns, rows, |point| {
                    evaluate(point, &mut values).map(Score::level)
                });
                for path in region_mesh::filled_paths(&grid, |point| {
                    evaluate(point, &mut values).map(Score::level)
                }) {
                    append_contour(&mut fill, &path);
                }
                for path in grid.contours(|point| evaluate(point, &mut values).map(Score::level)) {
                    append_boundary(&mut boundary, &path, |point| {
                        evaluate(point, &mut values).is_some_and(|score| score.strict)
                    });
                }
            }
            let color = region.legend.color;
            marks.push(Mark {
                legend: region.legend.clone(),
                symbol: LegendSymbol::Fill,
                commands: fill,
                paint: CanvasPaint {
                    fill: Some(CanvasFill::Solid(color.with_alpha(0.42))),
                    ..default()
                },
            });
            marks.push(Mark {
                legend: Legend {
                    name: format!("{} · 边界", region.legend.name),
                    color,
                },
                symbol: LegendSymbol::Hidden,
                commands: boundary,
                paint: plot::stroke_paint(color, plot::LINE_WIDTH),
            });
        }
        if marks.iter().all(|mark| mark.commands.is_empty()) {
            formula.push("当前范围内没有可绘制的不等式区域。".into());
        } else if marks
            .iter()
            .any(|mark| matches!(mark.symbol, LegendSymbol::Hidden) && !mark.commands.is_empty())
        {
            formula.push("实线包含边界，虚线排除边界。".into());
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
        Style::Region
    }
    fn formula_name(&self) -> &'static str {
        "InequalityFormula"
    }
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let mut unique = BTreeSet::new();
        if !(1..=4).contains(&props.regions.len())
            || props.regions.iter().any(|region| {
                region.name.trim().is_empty()
                    || region.name.len() > 96
                    || !unique.insert(region.name.as_str())
            })
        {
            return Err(io::Error::other("不等式图需要 1 至 4 个唯一命名的区域").into());
        }
        let display = Domain::from_axes(&props.x_axis, &props.y_axis)?;
        let x = numeric_axis(props.x_axis, display.x)?;
        let y = numeric_axis(props.y_axis, display.y)?;
        let variables = Variables::new(&["x", "y"], props.parameters)?;
        let regions = props
            .regions
            .into_iter()
            .enumerate()
            .map(|(index, region)| {
                if !(1..=4).contains(&region.conditions.len())
                    || (region.conditions.len() > 1 && region.combine.is_none())
                {
                    return Err(io::Error::other(format!(
                        "区域 {} 需要 1 至 4 个条件，多条件时须声明 combine",
                        region.name
                    ))
                    .into());
                }
                let bounds = region
                    .domain
                    .unwrap_or(display)
                    .clip(display, x.range, y.range)?;
                let constraints = region
                    .conditions
                    .into_iter()
                    .map(|condition| {
                        if !condition.level.is_finite() {
                            return Err(io::Error::other("不等式比较值必须有限").into());
                        }
                        let expression = Expression::parse(&condition.expression, &variables.names)
                            .map_err(|error| {
                                io::Error::other(format!("区域 {}：{error}", region.name))
                            })?;
                        Ok(Constraint {
                            expression,
                            comparison: condition.comparison,
                            level: condition.level,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(PreparedRegion {
                    legend: named_color(region.name, region.color, index),
                    bounds,
                    constraints,
                    combine: region.combine.unwrap_or(Combine::All),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title: props.title,
            x,
            y,
            display,
            regions,
            variables,
        })
    }
}

impl PreparedRegion {
    fn score(&self, slots: &[f64]) -> Option<Score> {
        let mut scores = self
            .constraints
            .iter()
            .map(|constraint| constraint.score(slots));
        let initial = scores.next()??;
        scores.try_fold(initial, |left, right| {
            let right = right?;
            Some(match self.combine {
                Combine::All if left.value > right.value => left,
                Combine::Any if left.value < right.value => left,
                mode if left.value == right.value => Score {
                    value: left.value,
                    strict: match mode {
                        Combine::All => left.strict || right.strict,
                        Combine::Any => left.strict && right.strict,
                    },
                },
                _ => right,
            })
        })
    }

    fn formula(&self, slots: &[Option<f64>]) -> String {
        let relation = match self.combine {
            Combine::All => r"\land ",
            Combine::Any => r"\lor ",
        };
        let formula = self
            .constraints
            .iter()
            .map(|constraint| {
                format!(
                    "{}{}{}",
                    constraint.expression.tex(slots),
                    constraint.comparison.tex(),
                    tex_number(constraint.level, 0)
                )
            })
            .collect::<Vec<_>>()
            .join(relation);
        format!("$${formula}$$")
    }
}

impl Constraint {
    fn score(&self, slots: &[f64]) -> Option<Score> {
        let actual = self.expression.evaluate(slots)?;
        let value = match self.comparison {
            Comparison::Lt | Comparison::Le => actual - self.level,
            Comparison::Gt | Comparison::Ge => self.level - actual,
        };
        value.is_finite().then_some(Score {
            value,
            strict: matches!(self.comparison, Comparison::Lt | Comparison::Gt),
        })
    }
}

impl Comparison {
    fn tex(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => r"\leq ",
            Self::Gt => ">",
            Self::Ge => r"\geq ",
        }
    }
}

impl Score {
    fn level(self) -> f64 {
        if self.strict && self.value == 0.0 {
            f64::MIN_POSITIVE
        } else {
            self.value
        }
    }
}

fn append_boundary(
    commands: &mut Vec<CanvasPathCommand>,
    path: &[DVec2],
    mut strict: impl FnMut(DVec2) -> bool,
) {
    const DASH: f32 = 6.0;
    const CYCLE: f32 = 10.0;
    let mut phase = 0.0;
    let mut pen = None;
    for pair in path.windows(2) {
        let [a, b] = [plot::map(pair[0]), plot::map(pair[1])];
        if !strict(pair[0] * 0.5 + pair[1] * 0.5) {
            append_segment(commands, &mut pen, a, b);
            phase = 0.0;
            continue;
        }
        let length = a.distance(b);
        let mut from = 0.0;
        while from < length {
            let ink = phase < DASH;
            let left = if ink { DASH - phase } else { CYCLE - phase };
            let to = (from + left).min(length);
            if ink {
                let start = if from == 0.0 {
                    a
                } else {
                    a.lerp(b, from / length)
                };
                let end = if to == length {
                    b
                } else {
                    a.lerp(b, to / length)
                };
                append_segment(commands, &mut pen, start, end);
            }
            phase = if to < length {
                if ink { DASH } else { 0.0 }
            } else {
                (phase + to - from) % CYCLE
            };
            from = to;
        }
    }
}

fn append_segment(commands: &mut Vec<CanvasPathCommand>, pen: &mut Option<Vec2>, a: Vec2, b: Vec2) {
    if a == b {
        return;
    }
    if *pen != Some(a) {
        commands.push(CanvasPathCommand::MoveTo(a));
    }
    commands.push(CanvasPathCommand::LineTo(b));
    *pen = Some(b);
}
