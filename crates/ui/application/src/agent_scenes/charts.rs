mod area;
mod axis;
pub(super) mod bars;
pub(super) mod boxplot;
pub(super) mod combined;
pub(super) mod heatmap;
pub(super) mod histogram;
pub(super) mod math;
mod pie;
mod plot;
pub(super) mod scatter;

use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::{math::DVec2, prelude::*};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

use axis::{Axis, AxisKind, AxisSpec, Coordinate, Range};

pub(super) const LINE_CHART: &str = "agentelier.line_chart";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LineProperties {
    title: Option<String>,
    #[serde(default)]
    x_kind: AxisKind,
    x_axis: AxisSpec<Coordinate>,
    y_axis: AxisSpec<f64>,
    series: Vec<LineSeries>,
    selected_series: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LineSeries {
    name: String,
    color: Option<[u8; 3]>,
    points: Vec<(Coordinate, Option<f64>)>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        LINE_CHART,
        "Display observed data as a native multi-series line chart. Null Y values break lines.",
        &[
            ("title", "Optional nonblank title."),
            ("x_kind", "number (default) or time. Time points and ranges use RFC 3339 timestamps including a time zone."),
            ("x_axis", "Required {label, unit?, range?}. range has two increasing values of the X kind. Omit range to fit observed X coordinates."),
            ("y_axis", "Required {label, unit?, range?}. range has two increasing numbers. Omit range to fit measured Y values."),
            ("series", "Required 1 to 8 named series [{name, color?, points}]. color is [R,G,B]. Each point is [x,y] and y is a number or null. Series are sorted by x, preserving input order of ties."),
            ("selected_series", "Optional series name. Null or omitted shows all. A composition can use series_filter."),
        ],
        line,
        None,
        None,
    )?
    .with_conditional_ecs_scene_update(LINE_CHART, update_line)
    .and_then(scatter::register)
    .and_then(area::register)
    .and_then(bars::register)
    .and_then(pie::register)
    .and_then(heatmap::register)
    .and_then(histogram::register)
    .and_then(boxplot::register)
    .and_then(combined::register)
    .and_then(math::register)
}

pub(super) fn install(app: &mut App) {
    plot::install(app);
    math::install(app);
}

pub(super) fn select_series(world: &mut World, root: Entity, selected: Option<&str>) -> Result {
    plot::RetainedPlot::select_series(world, root, selected)?;
    if world.get::<boxplot::BoxPlot>(root).is_some() {
        boxplot::select_detail(world, root, selected)?;
    }
    Ok(())
}

struct Series {
    legend: plot::Legend,
    points: Vec<(f64, Option<f64>)>,
}

impl Series {
    fn runs(&self, x: Range, y: Range) -> impl Iterator<Item = Vec<DVec2>> + '_ {
        observed_runs(&self.points, x, y)
    }
}

fn observed_runs(
    points: &[(f64, Option<f64>)],
    x: Range,
    y: Range,
) -> impl Iterator<Item = Vec<DVec2>> + '_ {
    points.split(|(_, y)| y.is_none()).map(move |run| {
        run.iter()
            .filter_map(|(x0, y0)| y0.map(|y0| DVec2::new(x.fraction(*x0), y.fraction(y0))))
            .collect()
    })
}

fn line(
    world: &mut World,
    parent: Entity,
    props: LineProperties,
) -> std::result::Result<Entity, EcsError> {
    let selection = props.selected_series.clone();
    let content = prepare_line(props).map_err(invalid)?;
    let root = plot::RetainedPlot::build(world, parent, content, plot::Style::Line)?;
    if let Err(error) = plot::RetainedPlot::select_series(world, root, selection.as_deref()) {
        world.despawn(root);
        return Err(construction(error));
    }
    Ok(root)
}

fn update_line(world: &mut World, root: Entity, props: LineProperties) -> Result<bool> {
    let selection = props.selected_series.clone();
    let content = prepare_line(props)?;
    if !plot::RetainedPlot::update(world, root, &content)? {
        return Ok(false);
    }
    plot::RetainedPlot::select_series(world, root, selection.as_deref())?;
    Ok(true)
}

fn prepare_line(props: LineProperties) -> Result<plot::Content> {
    let LineProperties {
        title,
        x_kind,
        x_axis,
        y_axis,
        series,
        selected_series,
    } = props;
    if selected_series
        .as_ref()
        .is_some_and(|name| !series.iter().any(|series| &series.name == name))
    {
        return Err(io::Error::other("selected_series 缺少对应折线系列").into());
    }
    let (x, y, series) = prepare_continuous(x_kind, x_axis, y_axis, series, None)?;
    let plots = series
        .into_iter()
        .map(|series| {
            let mut commands = Vec::new();
            for points in series.runs(x.range, y.range) {
                plot::append_line(&mut commands, &points);
            }
            plot::Mark {
                paint: plot::stroke_paint(series.legend.color, plot::LINE_WIDTH),
                legend: series.legend,
                symbol: plot::LegendSymbol::Line,
                commands,
            }
        })
        .collect();
    Ok(plot::Content {
        title,
        x,
        y,
        marks: plots,
        baseline: None,
    })
}

pub(super) fn validate_line_choices<'a>(
    scene: &SceneDefinition,
    values: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
) -> Result {
    let properties: LineProperties =
        serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = properties
        .series
        .iter()
        .map(|series| series.name.as_str())
        .collect();
    if all.is_some_and(|name| names.contains(name))
        || values
            .into_iter()
            .any(|name| Some(name) != all && !names.contains(name))
    {
        return Err(io::Error::other("折线筛选需要匹配系列名称，全部选项不能与系列重名").into());
    }
    Ok(())
}

fn prepare_continuous(
    kind: AxisKind,
    x_spec: AxisSpec<Coordinate>,
    y_spec: AxisSpec<f64>,
    series: Vec<LineSeries>,
    baseline: Option<f64>,
) -> Result<(Axis, Axis, Vec<Series>)> {
    if !(1..=plot::PALETTE.len()).contains(&series.len()) {
        return Err(io::Error::other("series 必须包含 1 至 8 个观测序列").into());
    }
    let mut names = BTreeSet::new();
    let mut materialized = Vec::with_capacity(series.len());
    for (index, series) in series.into_iter().enumerate() {
        if series.name.trim().is_empty()
            || series.name.len() > 96
            || !names.insert(series.name.clone())
            || series.points.is_empty()
        {
            return Err(io::Error::other("各序列需要唯一的非空名称和至少一个观测点").into());
        }
        let points = sorted_points(kind, series.points)?;
        let color = series
            .color
            .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b));
        materialized.push(Series {
            legend: plot::Legend {
                name: series.name,
                color,
            },
            points,
        });
    }
    let range = x_spec
        .range
        .map(|[lo, hi]| -> Result<_> { Ok([kind.value(lo)?, kind.value(hi)?]) })
        .transpose()?;
    let values = || {
        materialized
            .iter()
            .flat_map(|series| series.points.iter().copied())
    };
    let x_range = Range::new(range, values().map(|(x, _)| x), kind)?;
    let y_range = Range::new(
        y_spec.range,
        values().filter_map(|(_, y)| y).chain(baseline),
        AxisKind::Number,
    )?;
    if values().any(|(x, y)| {
        !x_range.fraction(x).is_finite() || y.is_some_and(|y| !y_range.fraction(y).is_finite())
    }) {
        return Err(io::Error::other("观测位置无法按轴范围建立有限比例").into());
    }
    let x = Axis::new(x_spec.label, x_spec.unit, x_range, kind, 5)?;
    let y = Axis::new(y_spec.label, y_spec.unit, y_range, AxisKind::Number, 5)?;
    Ok((x, y, materialized))
}

fn sorted_points(
    kind: AxisKind,
    points: Vec<(Coordinate, Option<f64>)>,
) -> Result<Vec<(f64, Option<f64>)>> {
    let mut points = points
        .into_iter()
        .map(|(x, y)| Ok((kind.value(x)?, y.filter(|y| y.is_finite()))))
        .collect::<Result<Vec<_>>>()?;
    points.sort_by(|(a, _), (b, _)| a.total_cmp(b));
    Ok(points)
}

fn invalid(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
