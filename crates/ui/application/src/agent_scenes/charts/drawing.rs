use super::{
    axis::{AxisKind, Range},
    construction, invalid, plot,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::{
    canvas::{
        CanvasDocument, CanvasEditToken, CanvasFill, CanvasPaint, CanvasPath, CanvasPathCommand,
        CanvasTextStyle,
    },
    job::JobFailure,
    scene::spawn_scene,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.script_canvas";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    x: [f64; 2],
    y: [f64; 2],
    items: Vec<Item>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Item {
    id: String,
    shape: Shape,
    style: Option<Style>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Shape {
    Point {
        at: [f64; 2],
        size: Option<f32>,
    },
    Line {
        from: [f64; 2],
        to: [f64; 2],
    },
    Path {
        points: Vec<[f64; 2]>,
        #[serde(default)]
        closed: bool,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    Ellipse {
        center: [f64; 2],
        radii: [f64; 2],
    },
    Rectangle {
        min: [f64; 2],
        max: [f64; 2],
    },
    Text {
        at: [f64; 2],
        value: String,
        size: Option<f32>,
    },
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Style {
    stroke: Option<[u8; 4]>,
    fill: Option<[u8; 4]>,
    stroke_width: Option<f32>,
}

struct Board {
    ranges: [Range; 2],
}

enum Rendered {
    Path { fill: Path, stroke: Path },
    Text { text: CanvasTextStyle, at: Vec2 },
}

struct Path {
    geometry: CanvasPath,
    paint: CanvasPaint,
}

#[derive(Clone, Copy)]
enum Native {
    Path { fill: Entity, stroke: Entity },
    Text(Entity),
}

#[derive(Component)]
struct ScriptDrawing {
    document: Entity,
    items: Vec<(String, Native)>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Render a batched native Canvas drawing from mathematical coordinates and stable item IDs. Suitable for Rhai scripted_widget parts.",
        &[
            ("x", "Required finite increasing [left,right] coordinate range."),
            ("y", "Required finite increasing [bottom,top] coordinate range, increasing upward."),
            ("items", "Up to 128 {id,shape,style?}. shape is {kind:'point',at,size?}, {kind:'line',from,to}, {kind:'path',points,closed?}, {kind:'circle',center,radius}, {kind:'ellipse',center,radii}, {kind:'rectangle',min,max}, or {kind:'text',at,value,size?}. Coordinates are [x,y]; point and text sizes are logical pixels. path uses 2 to 512 ordered vertices."),
            ("style", "Per-item style optionally includes stroke or fill as [R,G,B,A] integers and stroke_width. By default shapes have a visible outline; text or points use fill or stroke color. Closed shapes may fill and stroke separately."),
        ],
        build, None, None,
    )?.with_conditional_ecs_scene_update(TEMPLATE, update)
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let painting = project(props).map_err(invalid)?;
    let root = plot::root(world, parent, "NativeScriptCanvas", None)?;
    let result = (|| {
        let document = plot::document(world, root)?;
        let view = plot::viewport(document);
        spawn_scene(world, bsn! { ChildOf(root) @view }).map_err(construction)?;
        let items = CanvasDocument::edit(world, document, |world, token| {
            painting
                .into_iter()
                .map(|(id, item)| {
                    let native = item.create(world, token, &id)?;
                    Ok((id, native))
                })
                .collect::<Result<Vec<_>, JobFailure>>()
        })
        .map_err(construction)?;
        world
            .entity_mut(root)
            .insert(ScriptDrawing { document, items });
        Ok(root)
    })();
    if result.is_err() {
        world.despawn(root);
    }
    result
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let painting = project(props)?;
    let previous = world
        .get::<ScriptDrawing>(root)
        .ok_or_else(|| io::Error::other("脚本画布缺少图元状态"))?;
    if painting.len() != previous.items.len()
        || !painting
            .iter()
            .zip(&previous.items)
            .all(|((name, item), (id, native))| {
                name == id
                    && matches!(
                        (item, native),
                        (Rendered::Path { .. }, Native::Path { .. })
                            | (Rendered::Text { .. }, Native::Text(_))
                    )
            })
    {
        return Ok(false);
    }
    let items: Vec<_> = previous.items.iter().map(|(_, native)| *native).collect();
    let document = previous.document;
    CanvasDocument::edit(world, document, |world, token| {
        for ((_, item), native) in painting.into_iter().zip(items) {
            item.update(world, token, native)?;
        }
        Ok(())
    })?;
    Ok(true)
}

fn project(props: Properties) -> Result<Vec<(String, Rendered)>> {
    let ranges = [
        Range::new(Some(props.x), [], AxisKind::Number)?,
        Range::new(Some(props.y), [], AxisKind::Number)?,
    ];
    let board = Board { ranges };
    let mut unique = BTreeSet::new();
    if props.items.len() > 128
        || props.items.iter().any(|item| {
            item.id.trim().is_empty() || item.id.len() > 64 || !unique.insert(item.id.as_str())
        })
    {
        return Err(io::Error::other("脚本画布最多支持 128 个具有唯一 ID 的图元").into());
    }
    props
        .items
        .into_iter()
        .map(|item| {
            Ok((
                item.id,
                board.shape(item.shape, item.style.unwrap_or_default())?,
            ))
        })
        .collect()
}

impl Board {
    fn normalize(&self, at: [f64; 2]) -> Result<DVec2> {
        let result = DVec2::new(
            self.ranges[0].fraction(at[0]),
            self.ranges[1].fraction(at[1]),
        );
        if !result.is_finite() {
            return Err(io::Error::other("画布坐标必须有限且可计算").into());
        }
        Ok(result)
    }

    fn shape(&self, shape: Shape, style: Style) -> Result<Rendered> {
        let color = style.stroke.map_or_else(|| plot::palette(0), rgba);
        let fill = style.fill.map(rgba);
        let width = optional_size(style.stroke_width, 2.0, 0.25..=32.0)?;
        let stroke = plot::stroke_paint(color, width);
        let (points, closed) = match shape {
            Shape::Point { at, size } => {
                let mut commands = Vec::new();
                plot::append_marker(&mut commands, self.normalize(at)?);
                let size = optional_size(size, 6.0, 1.0..=32.0)?;
                return Ok(Rendered::paths(
                    Vec::new(),
                    commands,
                    None,
                    plot::stroke_paint(fill.unwrap_or(color), size),
                ));
            }
            Shape::Text { at, value, size } => {
                if value.trim().is_empty() || value.len() > 512 {
                    return Err(io::Error::other("画布文字须非空且最多 512 字节").into());
                }
                let font_size = optional_size(size, 15.0, 6.0..=64.0)?;
                let at = self.normalize(at)?;
                let visible = at.cmpge(DVec2::ZERO).all() && at.cmple(DVec2::ONE).all();
                let color = if visible {
                    fill.unwrap_or(color)
                } else {
                    Color::NONE
                };
                return Ok(Rendered::Text {
                    text: CanvasTextStyle {
                        content: value,
                        color,
                        font_size,
                        ..default()
                    },
                    at: plot::map(at.clamp(DVec2::ZERO, DVec2::ONE)),
                });
            }
            Shape::Line { from, to } => (vec![from, to], false),
            Shape::Path { points, closed } => {
                let minimum = if closed { 3 } else { 2 };
                if !(minimum..=512).contains(&points.len()) {
                    return Err(io::Error::other("画布路径的点数无效").into());
                }
                (points, closed)
            }
            Shape::Circle { center, radius } => (ellipse(center, [radius; 2])?, true),
            Shape::Ellipse { center, radii } => (ellipse(center, radii)?, true),
            Shape::Rectangle {
                min: [x, y],
                max: [right, top],
            } => {
                if !(x < right && y < top) {
                    return Err(io::Error::other("画布矩形需提供严格递增的坐标范围").into());
                }
                (vec![[x, y], [right, y], [right, top], [x, top]], true)
            }
        };
        if !closed && fill.is_some() {
            return Err(io::Error::other("没有闭合的画布路径不能填充").into());
        }
        let mut points = points
            .into_iter()
            .map(|at| self.normalize(at))
            .collect::<Result<Vec<_>>>()?;
        let mut interior = Vec::new();
        if closed {
            if fill.is_some() {
                plot::append_polygon(&mut interior, points.clone());
            }
            points.push(points[0]);
        }
        let mut outline = Vec::new();
        if style.stroke.is_some() || fill.is_none() {
            plot::append_line(&mut outline, &points);
        }
        Ok(Rendered::paths(interior, outline, fill, stroke))
    }
}

fn optional_size(
    size: Option<f32>,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Result<f32> {
    let size = size.unwrap_or(default);
    if !range.contains(&size) {
        return Err(io::Error::other("画布字号或笔划大小超出支持范围").into());
    }
    Ok(size)
}

fn ellipse(center: [f64; 2], radii: [f64; 2]) -> Result<Vec<[f64; 2]>> {
    if radii
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(io::Error::other("画布圆或椭圆的半径必须有限且为正").into());
    }
    let center = DVec2::from_array(center);
    let radii = DVec2::from_array(radii);
    Ok((0..72)
        .map(|index| {
            let theta = f64::from(index) * std::f64::consts::TAU / 72.0;
            (center + DVec2::from_angle(theta) * radii).to_array()
        })
        .collect())
}

fn rgba([r, g, b, a]: [u8; 4]) -> Color {
    Color::srgba_u8(r, g, b, a)
}

impl Rendered {
    fn paths(
        fill: Vec<CanvasPathCommand>,
        stroke: Vec<CanvasPathCommand>,
        ink: Option<Color>,
        outline: CanvasPaint,
    ) -> Self {
        let fill = Path::new(
            fill,
            CanvasPaint {
                fill: ink.map(CanvasFill::Solid),
                ..default()
            },
        );
        Self::Path {
            fill,
            stroke: Path::new(stroke, outline),
        }
    }

    fn create(
        self,
        world: &mut World,
        token: CanvasEditToken,
        id: &str,
    ) -> Result<Native, JobFailure> {
        let name = format!("ScriptCanvasItem-{id}");
        match self {
            Self::Path { fill, stroke } => {
                let fill = CanvasDocument::create_path(
                    world,
                    token,
                    fill.geometry,
                    Transform::default(),
                    fill.paint,
                )?;
                let stroke = CanvasDocument::create_path(
                    world,
                    token,
                    stroke.geometry,
                    Transform::default(),
                    stroke.paint,
                )?;
                world.entity_mut(stroke).insert(Name::new(name));
                Ok(Native::Path { fill, stroke })
            }
            Self::Text { text, at } => {
                let entity = CanvasDocument::create_text(
                    world,
                    token,
                    text,
                    Transform::from_translation(at.extend(0.0)),
                )?;
                world.entity_mut(entity).insert(Name::new(name));
                Ok(Native::Text(entity))
            }
        }
    }

    fn update(
        self,
        world: &mut World,
        token: CanvasEditToken,
        native: Native,
    ) -> Result<(), JobFailure> {
        match (self, native) {
            (Self::Path { fill, stroke }, Native::Path { fill: a, stroke: b }) => {
                CanvasDocument::update_path(
                    world,
                    token,
                    a,
                    fill.geometry,
                    Transform::default(),
                    fill.paint,
                )?;
                CanvasDocument::update_path(
                    world,
                    token,
                    b,
                    stroke.geometry,
                    Transform::default(),
                    stroke.paint,
                )
            }
            (Self::Text { text, at }, Native::Text(entity)) => {
                CanvasDocument::update_text(world, token, entity, text)?;
                CanvasDocument::update_transform(
                    world,
                    token,
                    entity,
                    Transform::from_translation(at.extend(0.0)),
                )
            }
            _ => Ok(()),
        }
    }
}

impl Path {
    fn new(mut commands: Vec<CanvasPathCommand>, mut paint: CanvasPaint) -> Self {
        if commands.is_empty() {
            commands.extend([
                CanvasPathCommand::MoveTo(Vec2::ZERO),
                CanvasPathCommand::LineTo(Vec2::X),
            ]);
            paint = default();
        }
        Self {
            geometry: CanvasPath { commands },
            paint,
        }
    }
}
