mod area;
mod axis;
pub(super) mod bars;
mod plot;
pub(super) mod scatter;

use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

use axis::{Axis, AxisKind, AxisSpec, Coordinate, Range};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LineProperties {
    title: Option<String>,
    #[serde(default)]
    x_kind: AxisKind,
    x_axis: AxisSpec<Coordinate>,
    y_axis: AxisSpec<f64>,
    series: Vec<LineSeries>,
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
        "agentelier.line_chart",
        "Display observed data as a native multi-series line chart. Null Y values break lines.",
        &[
            ("title", "Optional nonblank title."),
            ("x_kind", "number (default) or time. Time points and ranges use RFC 3339 timestamps including a time zone."),
            ("x_axis", "Required {label, unit?, range?}. range has two increasing values of the X kind. Omit range to fit observed X coordinates."),
            ("y_axis", "Required {label, unit?, range?}. range has two increasing numbers. Omit range to fit measured Y values."),
            ("series", "Required 1 to 8 named series [{name, color?, points}]. color is [R,G,B]. Each point is [x,y] and y is a number or null. Series are sorted by x, preserving input order of ties."),
        ],
        line,
        None,
        None,
    )
    .and_then(scatter::register)
    .and_then(area::register)
    .and_then(bars::register)
}

pub(super) fn install(app: &mut App) {
    plot::install(app);
}

pub(super) fn select_series(world: &mut World, root: Entity, selected: Option<&str>) -> Result {
    plot::RetainedPlot::select_series(world, root, selected)
}

struct Series {
    legend: plot::Legend,
    points: Vec<(f64, Option<f64>)>,
}

impl Series {
    fn runs(&self, x: Range, y: Range) -> impl Iterator<Item = Vec<DVec2>> + '_ {
        self.points.split(|(_, y)| y.is_none()).map(move |run| {
            run.iter()
                .filter_map(|(x0, y0)| y0.map(|y0| DVec2::new(x.fraction(*x0), y.fraction(y0))))
                .collect()
        })
    }
}

fn line(
    world: &mut World,
    parent: Entity,
    props: LineProperties,
) -> std::result::Result<Entity, EcsError> {
    let LineProperties {
        title,
        x_kind,
        x_axis,
        y_axis,
        series,
    } = props;
    let valid = prepare_continuous(x_kind, x_axis, y_axis, series, None).map_err(invalid)?;
    let (x, y, series) = valid;
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
                commands,
            }
        })
        .collect();
    plot::mount(
        world,
        parent,
        plot::Content {
            title,
            x,
            y,
            marks: plots,
            baseline: None,
        },
        plot::Style::Line,
    )
    .map(|plot| plot.root)
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
        let mut points = series
            .points
            .into_iter()
            .map(|(x, y)| Ok((kind.value(x)?, y.filter(|y| y.is_finite()))))
            .collect::<Result<Vec<_>>>()?;
        points.sort_by(|(a, _), (b, _)| a.total_cmp(b));
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

fn invalid(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
