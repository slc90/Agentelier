use super::{
    construction, invalid,
    plot::{self, Legend},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::prelude::*;
use bevy_widgetry::{
    canvas::{CanvasDocument, CanvasPaint, CanvasPath, CanvasPathCommand},
    scene::spawn_scene,
    text::WidgetryText,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    f64::consts::{FRAC_PI_2, TAU},
};

const RADIUS: f32 = 113.0;
const MIN_SWEEP: f64 = 0.000_01;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    unit: Option<String>,
    parts: Vec<Part>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Part {
    name: String,
    value: Option<f64>,
    color: Option<[u8; 3]>,
}

#[derive(Clone, Copy)]
enum Total {
    Value(f64),
    Zero,
    Missing,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        "agentelier.pie_chart",
        "Display portions of a known nonnegative total as a native pie chart with percentages.",
        &[
            ("title", "Optional nonblank title."),
            ("unit", "Optional shared unit for the total and each part."),
            ("parts", "Required 1 to 8 unique named parts [{name, value, color?}] in display order. value is a nonnegative finite number or null for a missing observation. color is [R,G,B]. A missing observation or zero total prevents a complete pie."),
        ],
        build,
        None,
        None,
    )
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    let unit = props.unit.as_deref();
    if unit.is_some_and(|unit| !label_is_valid(unit, 32)) {
        return Err(invalid("unit 必须是非空单行文本，最多 32 字节"));
    }
    if !(1..=plot::PALETTE.len()).contains(&props.parts.len()) {
        return Err(invalid("parts 必须包含 1 至 8 个组成类别"));
    }
    let mut names = BTreeSet::new();
    for part in &props.parts {
        if !label_is_valid(&part.name, 96) || !names.insert(&part.name) {
            return Err(invalid("组成需要唯一的非空单行名称，最多 96 字节"));
        }
        if part
            .value
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err(invalid("组成值必须为非负有限数值，缺失值使用 null"));
        }
    }
    let total = total(&props.parts)?;
    let root = plot::root(world, parent, "NativePieChart", props.title.as_deref())?;
    if let Err(error) = decorate(world, root, props, total) {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

fn total(parts: &[Part]) -> std::result::Result<Total, EcsError> {
    if parts.iter().any(|part| part.value.is_none()) {
        return Ok(Total::Missing);
    }
    let mut sum = 0.0;
    let mut correction = 0.0;
    for value in parts.iter().filter_map(|part| part.value) {
        let adjusted = value - correction;
        let next = sum + adjusted;
        if !next.is_finite() {
            return Err(invalid("组成合计超过可表示的范围"));
        }
        correction = (next - sum) - adjusted;
        sum = next;
    }
    Ok(if sum == 0.0 {
        Total::Zero
    } else {
        Total::Value(sum)
    })
}

fn decorate(
    world: &mut World,
    root: Entity,
    props: Properties,
    total: Total,
) -> std::result::Result<(), EcsError> {
    let colors: Vec<_> = props
        .parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            part.color
                .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b))
        })
        .collect();
    let document = if let Total::Value(sum) = total {
        Some(draw(world, root, &props.parts, &colors, sum)?)
    } else {
        None
    };
    let viewport = document.map(plot::viewport);
    let title = props.title.map(plot::heading);
    let unit = props.unit.as_deref();
    let summary = match total {
        Total::Value(sum) => format!("合计 {}", amount(sum, unit)),
        Total::Zero => format!("合计 {}，无法计算比例", amount(0.0, unit)),
        Total::Missing => "组成数据有缺失，无法计算合计和比例".into(),
    };
    let legend: Vec<_> = props
        .parts
        .into_iter()
        .zip(colors)
        .map(|(part, color)| {
            let value = part
                .value
                .map_or_else(|| "缺失".into(), |value| amount(value, unit));
            let name = if let (Total::Value(sum), Some(number)) = (total, part.value) {
                format!("{}  {}  ({})", part.name, value, percentage(number, sum))
            } else {
                format!("{}  {}", part.name, value)
            };
            plot::legend(Legend { name, color }, 11.0, 11.0)
        })
        .collect();
    let legend = bevy::scene::RelatedScenes::<Children>::new(legend);
    spawn_scene(world, bsn! {
        ChildOf(root)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{title}--
            @{viewport}--
            Name("PieTotal") Text(summary) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)--
            Node { min_width: px(0), column_gap: px(16), row_gap: px(4), flex_wrap: FlexWrap::Wrap }
            @legend
        ]
    }).map_err(construction)?;
    Ok(())
}

fn draw(
    world: &mut World,
    root: Entity,
    parts: &[Part],
    colors: &[Color],
    sum: f64,
) -> std::result::Result<Entity, EcsError> {
    let document = plot::document(world, root)?;
    let final_part = parts
        .iter()
        .rposition(|part| part.value.is_some_and(|value| value > 0.0));
    let full = parts
        .iter()
        .filter(|part| part.value.is_some_and(|value| value > 0.0))
        .count()
        == 1;
    CanvasDocument::edit(world, document, |world, token| {
        let mut start = -FRAC_PI_2;
        let mut labels = Vec::new();
        for (index, (part, color)) in parts.iter().zip(colors).enumerate() {
            let Some(value) = part.value.filter(|value| *value > 0.0) else {
                continue;
            };
            let end = if Some(index) == final_part {
                TAU - FRAC_PI_2
            } else {
                (start + TAU * (value / sum)).min(TAU - FRAC_PI_2)
            };
            if end - start >= MIN_SWEEP {
                let entity = CanvasDocument::create_path(
                    world,
                    token,
                    sector(start, end, full),
                    Transform::default(),
                    CanvasPaint {
                        fill: Some((*color).into()),
                        ..default()
                    },
                )?;
                world
                    .entity_mut(entity)
                    .insert(Name::new(format!("PieSector-{index}")));
                if value / sum >= 0.1 {
                    let point = if full {
                        Vec2::ZERO
                    } else {
                        direction((start + end) * 0.5) * 73.0
                    };
                    labels.push((percentage(value, sum), point, plot::contrast(*color)));
                }
            }
            start = end;
        }
        for (text, point, color) in labels {
            plot::label(world, token, &text, point, color, None)?;
        }
        Ok(())
    })
    .map_err(construction)?;
    Ok(document)
}

fn sector(start: f64, end: f64, full: bool) -> CanvasPath {
    use CanvasPathCommand::{ArcTo, Close, LineTo, MoveTo};
    let outer = direction(start) * RADIUS;
    let mut commands = if full {
        vec![MoveTo(outer)]
    } else {
        vec![MoveTo(Vec2::ZERO), LineTo(outer)]
    };
    let distance = end - start;
    let steps = (distance / FRAC_PI_2).ceil().clamp(1.0, 4.0) as u8;
    for step in 1..=steps {
        let angle = start + distance * f64::from(step) / f64::from(steps);
        let to = if full && step == steps {
            outer
        } else {
            direction(angle) * RADIUS
        };
        commands.push(ArcTo {
            radii: Vec2::splat(RADIUS),
            rotation: 0.0,
            large_arc: false,
            sweep: true,
            to,
        });
    }
    commands.push(Close);
    CanvasPath { commands }
}

fn direction(angle: f64) -> Vec2 {
    Vec2::new(angle.cos() as f32, angle.sin() as f32)
}

fn label_is_valid(label: &str, limit: usize) -> bool {
    !label.trim().is_empty()
        && label.len() <= limit
        && label.chars().all(super::super::text_input::single_line)
}

fn amount(value: f64, unit: Option<&str>) -> String {
    let label = plot::display_number(value);
    match unit {
        Some(unit) => format!("{label} {unit}"),
        None => label,
    }
}

fn percentage(value: f64, sum: f64) -> String {
    let percent = value / sum * 100.0;
    if value > 0.0 && percent < 0.005 {
        return "<0.01%".into();
    }
    let format = format!("{percent:.2}");
    format!("{}%", format.trim_end_matches('0').trim_end_matches('.'))
}
