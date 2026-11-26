use super::{
    axis::{Axis, AxisKind, AxisSpec, Range},
    invalid,
    plot::{self, Content, Legend, Mark, Rendered, Style},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::{CanvasDocument, CanvasGeometry, CanvasPaint, CanvasPathCommand};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.scatter_chart";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    series: Vec<Series>,
    selected_series: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Series {
    name: String,
    color: Option<[u8; 3]>,
    radius: Option<f32>,
    points: Vec<[Option<f64>; 2]>,
}

#[derive(Component)]
struct Scatter {
    plot: Rendered,
    title: Option<String>,
    x: Axis,
    y: Axis,
    legends: Vec<Legend>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Display independent numeric observations in a native scatter chart.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label, unit?, range?}; range has two increasing numeric values. Omit range to fit the valid observations."),
            ("y_axis", "Required {label, unit?, range?}; range has two increasing numeric values. Omit range to fit the valid observations."),
            ("series", "1 to 8 unique named series [{name, color?, radius?, points}]. color is [R,G,B]; radius is 1 to 16 in chart units (default 4). points is [[x,y],...]. Observations with a null coordinate are skipped."),
            ("selected_series", "Optional series name. Null or omitted shows all series. A composition can set this using a series_filter binding."),
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
    properties: Properties,
) -> std::result::Result<Entity, EcsError> {
    let selected = properties.selected_series.clone();
    let content = prepare(properties).map_err(invalid)?;
    let title = content.title.clone();
    let (x, y) = (content.x.clone(), content.y.clone());
    let legends = content
        .marks
        .iter()
        .map(|mark| mark.legend.clone())
        .collect();
    let plot = plot::mount(world, parent, content, Style::Scatter)?;
    let root = plot.root;
    world.entity_mut(root).insert(Scatter {
        plot,
        title,
        x,
        y,
        legends,
    });
    if let Err(error) = filter(world, root, selected.as_deref()) {
        world.despawn(root);
        return Err(super::construction(error));
    }
    Ok(root)
}

fn update(world: &mut World, root: Entity, properties: Properties) -> Result<bool> {
    let selected = properties.selected_series.clone();
    let content = prepare(properties)?;
    let chart = chart(world, root)?;
    if (chart.title.as_ref(), &chart.x, &chart.y)
        != (content.title.as_ref(), &content.x, &content.y)
        || !chart
            .legends
            .iter()
            .eq(content.marks.iter().map(|mark| &mark.legend))
    {
        return Ok(false);
    }
    let document = chart.plot.document;
    let mut changed = Vec::new();
    for (layer, mark) in chart.plot.layers.iter().zip(&content.marks) {
        let (path, paint) = mark.geometry();
        if !matches!(world.get::<CanvasGeometry>(layer.path), Some(CanvasGeometry::Path(current)) if current == &path)
            || world.get::<CanvasPaint>(layer.path) != Some(&paint)
        {
            path.validate(&Transform::default(), &paint)?;
            changed.push((layer.path, path, paint));
        }
    }
    if !changed.is_empty() {
        CanvasDocument::edit(world, document, |world, token| {
            for (entity, path, paint) in changed {
                CanvasDocument::update_path(
                    world,
                    token,
                    entity,
                    path,
                    Transform::default(),
                    paint,
                )?;
            }
            Ok(())
        })?;
    }
    filter(world, root, selected.as_deref())?;
    Ok(true)
}

fn prepare(properties: Properties) -> Result<Content> {
    let Properties {
        title,
        x_axis,
        y_axis,
        series,
        selected_series,
    } = properties;
    if !(1..=plot::PALETTE.len()).contains(&series.len()) {
        return Err(io::Error::other("series 必须包含 1 至 8 个观测序列").into());
    }
    let mut names = BTreeSet::new();
    for item in &series {
        if item.name.trim().is_empty() || item.name.len() > 96 || !names.insert(item.name.as_str())
        {
            return Err(io::Error::other("各序列需要唯一的非空名称，最多 96 字节").into());
        }
        if !(1.0..=16.0).contains(&item.radius.unwrap_or(4.0)) {
            return Err(io::Error::other("点半径必须在 1 至 16 之间").into());
        }
    }
    if selected_series
        .as_deref()
        .is_some_and(|name| !names.contains(name))
    {
        return Err(io::Error::other("selected_series 缺少对应数据系列").into());
    }
    let positions = || {
        series
            .iter()
            .flat_map(|item| item.points.iter().filter_map(position))
    };
    let x_range = Range::new(x_axis.range, positions().map(|p| p.x), AxisKind::Number)?;
    let y_range = Range::new(y_axis.range, positions().map(|p| p.y), AxisKind::Number)?;
    let x = Axis::new(x_axis.label, x_axis.unit, x_range, AxisKind::Number, 5)?;
    let y = Axis::new(y_axis.label, y_axis.unit, y_range, AxisKind::Number, 5)?;
    let marks = series
        .into_iter()
        .enumerate()
        .map(|(index, series)| {
            let mut commands = Vec::new();
            for point in series.points.iter().filter_map(position) {
                let normal = DVec2::new(x_range.fraction(point.x), y_range.fraction(point.y));
                if (0.0..=1.0).contains(&normal.x) && (0.0..=1.0).contains(&normal.y) {
                    let at = plot::map(normal);
                    // A tiny round-capped segment puts the marker center at the exact observation.
                    commands.extend([
                        CanvasPathCommand::MoveTo(at - Vec2::new(0.01, 0.0)),
                        CanvasPathCommand::LineTo(at + Vec2::new(0.01, 0.0)),
                    ]);
                }
            }
            Mark {
                legend: Legend {
                    name: series.name,
                    color: series
                        .color
                        .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b)),
                },
                commands,
                width: 2.0 * series.radius.unwrap_or(4.0),
            }
        })
        .collect();
    Ok(Content { title, x, y, marks })
}

fn position(point: &[Option<f64>; 2]) -> Option<DVec2> {
    let at = DVec2::new(point[0]?, point[1]?);
    at.is_finite().then_some(at)
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    choices: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
) -> Result {
    let properties: Properties =
        serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = properties
        .series
        .iter()
        .map(|series| series.name.as_str())
        .collect();
    if all.is_some_and(|all| names.contains(all))
        || choices
            .into_iter()
            .any(|value| Some(value) != all && !names.contains(value))
    {
        return Err(
            io::Error::other("系列筛选选项必须对应数据系列，全部选项不能与系列重名").into(),
        );
    }
    Ok(())
}

pub(in crate::agent_scenes) fn filter(
    world: &mut World,
    root: Entity,
    selected: Option<&str>,
) -> Result {
    let chart = chart(world, root)?;
    let document = chart.plot.document;
    let visibility: Vec<_> = chart
        .plot
        .layers
        .iter()
        .zip(&chart.legends)
        .map(|(layer, legend)| (layer.id, selected.is_none_or(|name| name == legend.name)))
        .collect();
    CanvasDocument::edit(world, document, |world, token| {
        for (layer, visible) in visibility {
            CanvasDocument::set_layer_visible(world, token, layer, visible)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn chart(world: &World, root: Entity) -> Result<&Scatter> {
    world
        .get::<Scatter>(root)
        .ok_or_else(|| io::Error::other("散点图缺少绘图状态").into())
}
