use super::{
    area,
    axis::{Axis, AxisKind, AxisSpec, Coordinate, Range},
    construction, invalid, observed_runs,
    plot::{self, Content, Legend, LegendSymbol, Mark, RetainedPlot, Style},
    sorted_points,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::CanvasPaint;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.combined_chart";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    #[serde(default)]
    x_kind: AxisKind,
    x_axis: AxisSpec<Coordinate>,
    y_axis: AxisSpec<f64>,
    series: Vec<Series>,
    selected_series: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Series {
    name: String,
    color: Option<[u8; 3]>,
    x: Option<AxisReference>,
    y: Option<AxisReference>,
    data: Data,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AxisReference {
    axis: String,
    unit: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Data {
    Line {
        points: Vec<(Coordinate, Option<f64>)>,
    },
    Scatter {
        points: Vec<(Option<Coordinate>, Option<f64>)>,
        radius: Option<f32>,
    },
    Area {
        points: Vec<(Coordinate, Option<f64>)>,
        baseline: f64,
    },
    Bar {
        intervals: Vec<Interval>,
        baseline: f64,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    bounds: [Coordinate; 2],
    value: Option<f64>,
}

type Observations = Vec<(f64, Option<f64>)>;

enum Geometry {
    Line(Observations),
    Scatter {
        points: Observations,
        radius: f32,
    },
    Area {
        points: Observations,
        baseline: f64,
    },
    Bar {
        intervals: Vec<([f64; 2], Option<f64>)>,
        baseline: f64,
    },
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw differently marked series in a single native chart with a common X/Y coordinate system and compatible units.",
        &[
            ("title", "Optional nonblank title."),
            ("x_kind", "number (default) or time (RFC 3339 timestamps including a time zone)."),
            ("x_axis", "Required {label,unit?,range?}. Optional increasing axis range in the X coordinate kind."),
            ("y_axis", "Required {label,unit?,range?}. Optional increasing numeric axis range."),
            ("series", "Required 1 to 8 unique named series in drawing order [{name,color?,x?,y?,data}]. A provided axis reference must have {axis:'x' or 'y',unit?} matching the shared axis. data kinds: line {points:[[x,y],...]}; scatter {points:[[x,y],...],radius?}; area {points:[[x,y],...],baseline}; bar {intervals:[{bounds:[left,right],value}],baseline}. Null observations leave a gap or an empty mark."),
            ("selected_series", "Optional series name; null or omitted shows all. A composition can use series_filter."),
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
    let selection = props.selected_series.clone();
    let content = prepare(props).map_err(invalid)?;
    let root = RetainedPlot::build(world, parent, content, Style::Combined)?;
    if let Err(error) = RetainedPlot::select_series(world, root, selection.as_deref()) {
        world.despawn(root);
        return Err(construction(error));
    }
    Ok(root)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let selection = props.selected_series.clone();
    let content = prepare(props)?;
    if !RetainedPlot::update(world, root, &content)? {
        return Ok(false);
    }
    RetainedPlot::select_series(world, root, selection.as_deref())?;
    Ok(true)
}

fn prepare(props: Properties) -> Result<Content> {
    let Properties {
        title,
        x_kind,
        x_axis,
        y_axis,
        series,
        selected_series,
    } = props;
    let mut names = BTreeSet::new();
    if !(1..=plot::PALETTE.len()).contains(&series.len())
        || series.iter().any(|series| {
            series.name.trim().is_empty()
                || series.name.len() > 96
                || !names.insert(series.name.as_str())
                || !series
                    .name
                    .chars()
                    .all(super::super::text_input::single_line)
        })
    {
        return Err(io::Error::other("series 需要 1 至 8 个唯一的非空单行名称").into());
    }
    if selected_series
        .as_deref()
        .is_some_and(|name| !names.contains(name))
    {
        return Err(io::Error::other("selected_series 缺少对应图形系列").into());
    }
    let mut x_values = Vec::new();
    let mut y_values = Vec::new();
    let mut shapes = Vec::new();
    for (index, series) in series.into_iter().enumerate() {
        check_axis("x", x_axis.unit.as_deref(), series.x.as_ref(), &series.name)?;
        check_axis("y", y_axis.unit.as_deref(), series.y.as_ref(), &series.name)?;
        let shape = Geometry::new(series.data, x_kind)?;
        shape.extents(&mut x_values, &mut y_values);
        let color = series
            .color
            .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b));
        shapes.push((
            Legend {
                name: series.name,
                color,
            },
            shape,
        ));
    }
    let x_explicit = x_axis
        .range
        .map(|[left, right]| -> Result<_> { Ok([x_kind.value(left)?, x_kind.value(right)?]) })
        .transpose()?;
    let x_range = Range::new(x_explicit, x_values.iter().copied(), x_kind)?;
    let y_range = Range::new(y_axis.range, y_values.iter().copied(), AxisKind::Number)?;
    if x_values.iter().any(|&x| !x_range.fraction(x).is_finite())
        || y_values.iter().any(|&y| !y_range.fraction(y).is_finite())
    {
        return Err(io::Error::other("数据位置无法按轴范围建立有限比例").into());
    }
    let x = Axis::new(x_axis.label, x_axis.unit, x_range, x_kind, 5)?;
    let y = Axis::new(y_axis.label, y_axis.unit, y_range, AxisKind::Number, 5)?;
    let marks = shapes
        .into_iter()
        .map(|(legend, shape)| shape.draw(legend, x_range, y_range))
        .collect();
    Ok(Content {
        title,
        x,
        y,
        marks,
        baseline: None,
    })
}

fn check_axis(
    expected: &str,
    unit: Option<&str>,
    reference: Option<&AxisReference>,
    name: &str,
) -> Result {
    if let Some(reference) = reference {
        if reference.axis != expected {
            return Err(
                io::Error::other(format!("系列 {name} 引用的轴无效，必须为 {expected}")).into(),
            );
        }
        if reference
            .unit
            .as_deref()
            .is_some_and(|requested| Some(requested) != unit)
        {
            return Err(
                io::Error::other(format!("系列 {name} 的单位与共享 {expected} 轴不兼容")).into(),
            );
        }
    }
    Ok(())
}

impl Geometry {
    fn new(data: Data, x: AxisKind) -> Result<Self> {
        match data {
            Data::Line { points } => Ok(Self::Line(sorted_points(x, points)?)),
            Data::Scatter { points, radius } => {
                let radius = radius.unwrap_or(4.0);
                if !(1.0..=16.0).contains(&radius) {
                    return Err(io::Error::other("散点半径必须在 1 至 16 之间").into());
                }
                let points = points
                    .into_iter()
                    .filter_map(|(x, y)| Some((x?, y.filter(|y| y.is_finite())?)))
                    .map(|(coordinate, value)| Ok((x.value(coordinate)?, Some(value))))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Self::Scatter { points, radius })
            }
            Data::Area { points, baseline } => Ok(Self::Area {
                points: sorted_points(x, points)?,
                baseline: validate_baseline(baseline)?,
            }),
            Data::Bar {
                intervals,
                baseline,
            } => {
                let intervals = intervals
                    .into_iter()
                    .map(|interval| {
                        let [left, right] = interval.bounds;
                        let bounds = [x.value(left)?, x.value(right)?];
                        if bounds[0] >= bounds[1] {
                            return Err(io::Error::other("柱形的 bounds 必须从小到大").into());
                        }
                        Ok((bounds, interval.value.filter(|value| value.is_finite())))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(Self::Bar {
                    intervals,
                    baseline: validate_baseline(baseline)?,
                })
            }
        }
    }

    fn extents(&self, x: &mut Vec<f64>, y: &mut Vec<f64>) {
        match self {
            Self::Line(points) | Self::Scatter { points, .. } | Self::Area { points, .. } => {
                x.extend(points.iter().map(|(x, _)| *x));
                y.extend(points.iter().filter_map(|(_, y)| *y));
            }
            Self::Bar { intervals, .. } => {
                x.extend(intervals.iter().flat_map(|(bounds, _)| *bounds));
                y.extend(intervals.iter().filter_map(|(_, value)| *value));
            }
        }
        if let Self::Area { baseline, .. } | Self::Bar { baseline, .. } = self {
            y.push(*baseline);
        }
    }

    fn draw(self, legend: Legend, x: Range, y: Range) -> Mark {
        let color = legend.color;
        let mut commands = Vec::new();
        let (symbol, paint) = match self {
            Self::Line(points) => {
                for run in observed_runs(&points, x, y) {
                    plot::append_line(&mut commands, &run);
                }
                (
                    LegendSymbol::Line,
                    plot::stroke_paint(color, plot::LINE_WIDTH),
                )
            }
            Self::Scatter { points, radius } => {
                for (x0, y0) in points
                    .into_iter()
                    .filter_map(|(x, y)| y.map(|value| (x, value)))
                {
                    plot::append_marker(&mut commands, DVec2::new(x.fraction(x0), y.fraction(y0)));
                }
                (LegendSymbol::Point, plot::stroke_paint(color, 2.0 * radius))
            }
            Self::Area { points, baseline } => {
                for run in observed_runs(&points, x, y) {
                    area::append_area(&mut commands, &run, y.fraction(baseline));
                }
                (
                    LegendSymbol::Fill,
                    CanvasPaint {
                        fill: Some(color.with_alpha(0.42).into()),
                        ..default()
                    },
                )
            }
            Self::Bar {
                intervals,
                baseline,
            } => {
                for ([left, right], value) in intervals {
                    if let Some(value) = value {
                        plot::append_bar(
                            &mut commands,
                            x.fraction(left),
                            x.fraction(right),
                            y.fraction(baseline),
                            y.fraction(value),
                        );
                    }
                }
                let mut paint = plot::stroke_paint(color, 1.4);
                paint.fill = Some(color.with_alpha(0.72).into());
                (LegendSymbol::Column, paint)
            }
        };
        Mark {
            legend,
            symbol,
            commands,
            paint,
        }
    }
}

fn validate_baseline(baseline: f64) -> Result<f64> {
    if baseline.is_finite() {
        Ok(baseline)
    } else {
        Err(io::Error::other("baseline 必须为有限值").into())
    }
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    values: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
) -> Result {
    let props: Properties = serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = props
        .series
        .iter()
        .map(|series| series.name.as_str())
        .collect();
    if all.is_some_and(|name| names.contains(name))
        || values
            .into_iter()
            .any(|name| Some(name) != all && !names.contains(name))
    {
        return Err(
            io::Error::other("组合图筛选需要匹配各系列名称，全部选项不能与系列重名").into(),
        );
    }
    Ok(())
}
