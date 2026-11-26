pub(in crate::agent_scenes) mod geometry;

use super::{construction, plot};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::prelude::*;
use bevy_widgetry::{
    canvas::{
        CanvasDocument, CanvasPaint, CanvasPath, CanvasPathCommand, CanvasTextStyle, CanvasView,
    },
    scene::spawn_scene,
    theme::WidgetryTheme,
};

#[derive(Component)]
struct BoardInput {
    root: Entity,
    canvas: Entity,
}

#[derive(Clone)]
struct Board {
    document: Entity,
    paths: Vec<Entity>,
    labels: Vec<Entity>,
    ink: Color,
}

#[derive(Default)]
struct Drawing {
    paths: Vec<(Vec<CanvasPathCommand>, CanvasPaint)>,
    labels: Vec<(String, Vec2)>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    geometry::register(host)
}

pub(super) fn install(app: &mut App) {
    geometry::install(app);
}

impl Board {
    fn mount(
        world: &mut World,
        root: Entity,
        title: Option<String>,
        drawing: Drawing,
        interactive: bool,
    ) -> Result<Self, EcsError> {
        let document = plot::document(world, root)?;
        let color = world.resource::<WidgetryTheme>().text.normal.foreground;
        let heading = title.map(plot::heading);
        let canvas = plot::viewport(document);
        let wrapper = spawn_scene(world, bsn! {
            ChildOf(root) Name("DiagramBoard")
            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
            Children [
                @{heading}--
                Name("DiagramInput") Node { width: percent(100), min_width: px(0), overflow: Overflow::clip() }
                Children [ @canvas ]
            ]
        }).map_err(construction)?;
        let canvas =
            plot::viewport_in(world, wrapper).ok_or_else(|| construction("示意图缺少原生画布"))?;
        if interactive {
            let input = world
                .get::<ChildOf>(canvas)
                .ok_or_else(|| construction("示意图缺少指针区域"))?
                .parent();
            world
                .entity_mut(input)
                .insert((BoardInput { root, canvas }, Pickable::default()));
        }
        let (paths, labels) = CanvasDocument::edit(world, document, |world, token| {
            let paths = drawing
                .paths
                .into_iter()
                .map(|(commands, paint)| {
                    let (path, paint) = path_geometry(commands, paint);
                    CanvasDocument::create_path(world, token, path, Transform::default(), paint)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let labels = drawing
                .labels
                .into_iter()
                .map(|(string, at)| {
                    CanvasDocument::create_text(
                        world,
                        token,
                        text_style(string, color),
                        Transform::from_translation(at.extend(0.0)),
                    )
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok((paths, labels))
        })
        .map_err(construction)?;
        Ok(Self {
            document,
            paths,
            labels,
            ink: color,
        })
    }

    fn plot(&self, world: &mut World, drawing: Drawing) -> Result {
        if self.paths.len() != drawing.paths.len() || self.labels.len() != drawing.labels.len() {
            return Err(std::io::Error::other("示意图更新改变了现有路径或标签数量").into());
        }
        CanvasDocument::edit(world, self.document, |world, token| {
            for (&entity, (commands, paint)) in self.paths.iter().zip(drawing.paths) {
                let (path, paint) = path_geometry(commands, paint);
                CanvasDocument::update_path(
                    world,
                    token,
                    entity,
                    path,
                    Transform::default(),
                    paint,
                )?;
            }
            for (&entity, (label, at)) in self.labels.iter().zip(drawing.labels) {
                CanvasDocument::update_text(world, token, entity, text_style(label, self.ink))?;
                CanvasDocument::update_transform(
                    world,
                    token,
                    entity,
                    Transform::from_translation(at.extend(0.0)),
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn text_style(content: String, color: Color) -> CanvasTextStyle {
    CanvasTextStyle {
        content,
        color,
        font_size: 15.0,
        ..default()
    }
}

fn path_geometry(
    commands: Vec<CanvasPathCommand>,
    paint: CanvasPaint,
) -> (CanvasPath, CanvasPaint) {
    use CanvasPathCommand::{LineTo, MoveTo};
    if commands.is_empty() {
        return (
            CanvasPath {
                commands: vec![MoveTo(Vec2::ZERO), LineTo(Vec2::X)],
            },
            default(),
        );
    }
    (CanvasPath { commands }, paint)
}

fn pointer_position(world: &World, input: &BoardInput, screen: Vec2) -> Option<(Vec2, f32)> {
    let transform = world.get::<UiGlobalTransform>(input.canvas)?;
    let layout = world.get::<ComputedNode>(input.canvas)?;
    let inv_scale = layout.inverse_scale_factor();
    let zoom = CanvasView::zoom(world, input.canvas).ok()?;
    let local = transform.inverse().transform_point2(screen / inv_scale) * inv_scale;
    let center = CanvasView::center(world, input.canvas).ok()?;
    Some((center + local / zoom, zoom))
}

fn line(commands: &mut Vec<CanvasPathCommand>, a: Vec2, b: Vec2) {
    if a != b {
        commands.extend([CanvasPathCommand::MoveTo(a), CanvasPathCommand::LineTo(b)]);
    }
}

fn arrow(commands: &mut Vec<CanvasPathCommand>, a: Vec2, b: Vec2) {
    line(commands, a, b);
    let delta = b - a;
    let size = (delta.length() * 0.34).min(12.0);
    if size > 0.0 {
        let along = delta.normalize() * size;
        let tangent = along.perp() * 0.5;
        line(commands, b, b - along + tangent);
        line(commands, b, b - along - tangent);
    }
}

fn circle(commands: &mut Vec<CanvasPathCommand>, center: Vec2, radius: f32) {
    arc(commands, center, radius, 0.0, std::f32::consts::TAU, 64);
    commands.push(CanvasPathCommand::Close);
}

fn arc(
    commands: &mut Vec<CanvasPathCommand>,
    center: Vec2,
    radius: f32,
    from: f32,
    angle: f32,
    steps: u32,
) {
    let at = |turn| center + Vec2::from_angle(turn) * radius;
    commands.push(CanvasPathCommand::MoveTo(at(from)));
    commands.extend(
        (1..=steps)
            .map(|step| CanvasPathCommand::LineTo(at(from + angle * step as f32 / steps as f32))),
    );
}
