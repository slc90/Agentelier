use super::{
    LineSeries,
    axis::{AxisKind, AxisSpec, Coordinate},
    invalid,
    plot::{self, Baseline, Content, Mark, RetainedPlot, Style},
    prepare_continuous,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::{CanvasPaint, CanvasPathCommand};
use serde::{Deserialize, Serialize};
use std::io;

const TEMPLATE: &str = "agentelier.area_chart";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    #[serde(default)]
    x_kind: AxisKind,
    x_axis: AxisSpec<Coordinate>,
    y_axis: AxisSpec<f64>,
    baseline: f64,
    series: Vec<LineSeries>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Fill the area between observed series and an explicit baseline, with separate areas for distinct series.",
        &[
            ("title", "Optional nonblank title."),
            ("x_kind", "number (default) or time. Time points and ranges use RFC 3339 timestamps including a time zone."),
            ("x_axis", "Required {label, unit?, range?}. range has two increasing numbers or RFC 3339 timestamps of the X kind."),
            ("y_axis", "Required {label, unit?, range?}. range has two increasing numbers. Automatic ranges cover observations and the baseline."),
            ("baseline", "Required finite number in the units of the Y axis."),
            ("series", "1 to 8 named series [{name, color?, points}]. color is [R,G,B], points are [x,y]. Null Y breaks the area. Each series is filled independently."),
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
    RetainedPlot::build(world, parent, prepare(props).map_err(invalid)?, Style::Area)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let content = prepare(props)?;
    RetainedPlot::update(world, root, &content)
}

fn prepare(props: Properties) -> Result<Content> {
    let Properties {
        title,
        x_kind,
        x_axis,
        y_axis,
        baseline,
        series,
    } = props;
    let (x, y, series) = prepare_continuous(x_kind, x_axis, y_axis, series, Some(baseline))?;
    let fraction = y.range.fraction(baseline);
    if !fraction.is_finite() {
        return Err(io::Error::other("基线无法按 Y 轴范围建立有限比例").into());
    }
    let marks = series
        .into_iter()
        .map(|series| {
            let mut commands = Vec::new();
            for run in series.runs(x.range, y.range) {
                append_area(&mut commands, &run, fraction);
            }
            Mark {
                paint: CanvasPaint {
                    fill: Some(series.legend.color.with_alpha(0.42).into()),
                    ..default()
                },
                legend: series.legend,
                commands,
            }
        })
        .collect();
    Ok(Content {
        title,
        x,
        y,
        marks,
        baseline: Some(Baseline {
            fraction,
            value: baseline,
        }),
    })
}

fn append_area(commands: &mut Vec<CanvasPathCommand>, points: &[DVec2], baseline: f64) {
    let [first, ..] = points else { return };
    let mut lobe = vec![*first];
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        if (from.y < baseline && to.y > baseline) || (from.y > baseline && to.y < baseline) {
            let crossing = plot::intersection(from, to, 1, baseline);
            lobe.push(crossing);
            fill_lobe(commands, &lobe, baseline);
            lobe.clear();
            lobe.push(crossing);
        } else if from.y == baseline && lobe.len() > 1 {
            fill_lobe(commands, &lobe, baseline);
            lobe.clear();
            lobe.push(from);
        }
        lobe.push(to);
    }
    fill_lobe(commands, &lobe, baseline);
}

fn fill_lobe(commands: &mut Vec<CanvasPathCommand>, points: &[DVec2], baseline: f64) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    if first.x == last.x {
        return;
    }
    let polygon = points
        .iter()
        .copied()
        .chain([DVec2::new(last.x, baseline), DVec2::new(first.x, baseline)])
        .collect();
    plot::append_polygon(commands, polygon);
}
