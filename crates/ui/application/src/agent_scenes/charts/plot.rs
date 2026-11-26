use super::{axis::Axis, construction, invalid};
use agentelier_agent_host::EcsError;
use bevy::{
    math::DVec2,
    prelude::*,
    ui::{InteractionDisabled, UiSystems},
};
use bevy_widgetry::{
    canvas::{
        CanvasDocument, CanvasEditToken, CanvasLayerId, CanvasLineCap, CanvasLineJoin, CanvasPaint,
        CanvasPath, CanvasPathCommand, CanvasPlugin, CanvasStroke, CanvasTextStyle, CanvasView,
    },
    job::JobFailure,
    scene::spawn_scene,
    text::WidgetryText,
    theme::WidgetryTheme,
};

const WIDTH: f32 = 656.0;
const HEIGHT: f32 = 291.0;
const BOTTOM_LEFT: Vec2 = Vec2::new(-252.0, 105.0);
const TOP_RIGHT: Vec2 = Vec2::new(272.0, -127.0);
const TICK_FONT: f32 = 15.0;
pub(super) const LINE_WIDTH: f32 = 2.2;

pub(super) const PALETTE: [[u8; 3]; 8] = [
    [37, 103, 79],
    [49, 105, 167],
    [155, 78, 50],
    [122, 81, 159],
    [155, 114, 33],
    [28, 124, 129],
    [156, 68, 91],
    [77, 105, 124],
];

#[derive(Component, Clone, Default)]
struct NativePlot;

#[derive(Clone, PartialEq)]
pub(super) struct Legend {
    pub name: String,
    pub color: Color,
}

pub(super) struct Mark {
    pub legend: Legend,
    pub commands: Vec<CanvasPathCommand>,
    pub width: f32,
}

impl Mark {
    pub fn geometry(&self) -> (CanvasPath, CanvasPaint) {
        if self.commands.is_empty() {
            return (
                CanvasPath {
                    commands: vec![
                        CanvasPathCommand::MoveTo(Vec2::ZERO),
                        CanvasPathCommand::LineTo(Vec2::X),
                    ],
                },
                CanvasPaint {
                    fill: None,
                    ..default()
                },
            );
        }
        (
            CanvasPath {
                commands: self.commands.clone(),
            },
            stroke_paint(self.legend.color, self.width),
        )
    }
}

pub(super) struct Content {
    pub title: Option<String>,
    pub x: Axis,
    pub y: Axis,
    pub marks: Vec<Mark>,
}

#[derive(Clone, Copy)]
pub(super) enum Style {
    Line,
    Scatter,
}

pub(super) struct Rendered {
    pub root: Entity,
    pub document: Entity,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Copy)]
pub(super) struct Layer {
    pub id: CanvasLayerId,
    pub path: Entity,
}

pub(super) fn palette(index: usize) -> Color {
    let [r, g, b] = PALETTE[index % PALETTE.len()];
    Color::srgb_u8(r, g, b)
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(CanvasPlugin)
        .add_systems(PostUpdate, fit.after(UiSystems::Layout));
}

pub(super) fn mount(
    world: &mut World,
    parent: Entity,
    content: Content,
    style: Style,
) -> std::result::Result<Rendered, EcsError> {
    if content
        .title
        .as_ref()
        .is_some_and(|title| title.trim().is_empty() || title.len() > 256)
    {
        return Err(invalid("title 必须非空且最多 256 字节"));
    }
    let name = match style {
        Style::Line => "NativeLineChart",
        Style::Scatter => "NativeScatterChart",
    };
    let root = spawn_scene(world, bsn! {
        ChildOf(parent) Name(name)
        Node { width: percent(100), min_width: px(0), max_width: px(880), flex_direction: FlexDirection::Column, row_gap: px(5) }
    }).map_err(construction)?;
    let result = decorate(world, root, content, style);
    if let Err(error) = result {
        world.despawn(root);
        return Err(error);
    }
    result
}

fn decorate(
    world: &mut World,
    root: Entity,
    content: Content,
    style: Style,
) -> std::result::Result<Rendered, EcsError> {
    let Content { title, x, y, marks } = content;
    let document = spawn_scene(
        world,
        bsn! { ChildOf(root) Name("ChartDocument") template(|_| Ok(CanvasDocument)) },
    )
    .map_err(construction)?;
    let theme = world.resource::<WidgetryTheme>();
    let ink = theme.text.normal.foreground;
    let grid = theme.text_field.editable.normal.border;
    CanvasDocument::set_history_capacity(world, document, 0).map_err(construction)?;
    let layers = CanvasDocument::edit(world, document, |world, token| {
        axes(world, token, &x, &y, ink, grid)?;
        let mut layers = Vec::new();
        for (index, mark) in marks.iter().enumerate() {
            let (geometry, paint) = mark.geometry();
            let transform = Transform::default();
            let path = match style {
                Style::Line if mark.commands.is_empty() => continue,
                Style::Line => {
                    CanvasDocument::create_path(world, token, geometry, transform, paint)?
                }
                Style::Scatter => {
                    let id =
                        CanvasDocument::create_layer(world, token, index + 1, &mark.legend.name)?;
                    let path = CanvasDocument::create_path_in_layer(
                        world, token, id, geometry, transform, paint,
                    )?;
                    layers.push(Layer { id, path });
                    path
                }
            };
            world
                .entity_mut(path)
                .insert(Name::new(format!("ChartSeries-{index}")));
        }
        Ok(layers)
    })
    .map_err(construction)?;
    let y_name = y.name;
    let x_name = x.name;
    let title = title.map(|title| {
        bsn! {
            Text(title) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
            TextFont { font_size: bevy::text::FontSize::Px(19.0) }
        }
    });
    let legend: Vec<_> = marks
        .into_iter()
        .map(|mark| legend(mark.legend, style))
        .collect();
    let legend = bevy::scene::RelatedScenes::<Children>::new(legend);
    spawn_scene(world, bsn! {
        ChildOf(root)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{title}--
            Text(y_name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)--
            @CanvasView { @document: {Some(document)} }
            NativePlot Name("NativePlotViewport") Pickable::IGNORE InteractionDisabled
            Node { width: percent(100), min_width: px(0), min_height: px(140), aspect_ratio: {Some(WIDTH / HEIGHT)} }
            template(|context| Ok(BackgroundColor(context.resource::<WidgetryTheme>().text_field.editable.normal.background)))--
            Text(x_name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
            Node { align_self: AlignSelf::Center }--
            Node { min_width: px(0), column_gap: px(14), row_gap: px(4), flex_wrap: FlexWrap::Wrap }
            @legend
        ]
    }).map_err(construction)?;
    Ok(Rendered {
        root,
        document,
        layers,
    })
}

fn legend(item: Legend, style: Style) -> impl Scene {
    let Legend { name, color } = item;
    let (width, height) = match style {
        Style::Line => (18., 3.),
        Style::Scatter => (8., 8.),
    };
    bsn! {
        Node { min_width: px(0), max_width: percent(100), align_items: AlignItems::Center, column_gap: px(6), overflow: Overflow::clip_x() }
        Children [
            Node { width: px(width), height: px(height), flex_shrink: 0.0, border_radius: BorderRadius::MAX }
            BackgroundColor(color)--
            Text(name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordBoundary)
            Node { min_width: px(0) }
        ]
    }
}

fn axes(
    world: &mut World,
    token: CanvasEditToken,
    x: &Axis,
    y: &Axis,
    ink: Color,
    grid: Color,
) -> std::result::Result<(), JobFailure> {
    let mut grid_lines = Vec::new();
    let mut axes = Vec::new();
    draw_straight(
        &mut axes,
        BOTTOM_LEFT,
        Vec2::new(TOP_RIGHT.x, BOTTOM_LEFT.y),
    );
    draw_straight(
        &mut axes,
        BOTTOM_LEFT,
        Vec2::new(BOTTOM_LEFT.x, TOP_RIGHT.y),
    );
    for tick in &x.ticks {
        let at = map(DVec2::new(tick.fraction, 0.0)).x;
        draw_straight(
            &mut grid_lines,
            Vec2::new(at, BOTTOM_LEFT.y),
            Vec2::new(at, TOP_RIGHT.y),
        );
        draw_straight(
            &mut axes,
            Vec2::new(at, BOTTOM_LEFT.y),
            Vec2::new(at, BOTTOM_LEFT.y + 5.0),
        );
        label(
            world,
            token,
            &tick.label,
            Vec2::new(at, BOTTOM_LEFT.y + 17.0),
            ink,
        )?;
    }
    for tick in &y.ticks {
        let at = map(DVec2::new(0.0, tick.fraction)).y;
        draw_straight(
            &mut grid_lines,
            Vec2::new(BOTTOM_LEFT.x, at),
            Vec2::new(TOP_RIGHT.x, at),
        );
        draw_straight(
            &mut axes,
            Vec2::new(BOTTOM_LEFT.x - 5.0, at),
            Vec2::new(BOTTOM_LEFT.x, at),
        );
        label(
            world,
            token,
            &tick.label,
            Vec2::new(BOTTOM_LEFT.x - 40.0, at),
            ink,
        )?;
    }
    stroke(world, token, grid_lines, grid, 0.7)?;
    stroke(world, token, axes, ink, 1.2)?;
    Ok(())
}

fn label(
    world: &mut World,
    token: CanvasEditToken,
    text: &str,
    at: Vec2,
    color: Color,
) -> std::result::Result<(), JobFailure> {
    CanvasDocument::create_text(
        world,
        token,
        CanvasTextStyle {
            content: text.to_owned(),
            font_size: TICK_FONT,
            color,
            ..default()
        },
        Transform::from_translation(at.extend(0.0)),
    )?;
    Ok(())
}

fn stroke(
    world: &mut World,
    token: CanvasEditToken,
    commands: Vec<CanvasPathCommand>,
    color: Color,
    width: f32,
) -> std::result::Result<Entity, JobFailure> {
    CanvasDocument::create_path(
        world,
        token,
        CanvasPath { commands },
        Transform::default(),
        stroke_paint(color, width),
    )
}

fn stroke_paint(color: Color, width: f32) -> CanvasPaint {
    CanvasPaint {
        fill: None,
        stroke: Some(CanvasStroke {
            color,
            width,
            cap: CanvasLineCap::Round,
            join: CanvasLineJoin::Round,
        }),
        ..default()
    }
}

fn draw_straight(commands: &mut Vec<CanvasPathCommand>, from: Vec2, to: Vec2) {
    commands.push(CanvasPathCommand::MoveTo(from));
    commands.push(CanvasPathCommand::LineTo(to));
}

pub(super) fn map(normalized: DVec2) -> Vec2 {
    BOTTOM_LEFT + normalized.as_vec2() * (TOP_RIGHT - BOTTOM_LEFT)
}

pub(super) fn append_line(commands: &mut Vec<CanvasPathCommand>, points: &[DVec2]) {
    if let [point] = points
        && (0.0..=1.0).contains(&point.x)
        && (0.0..=1.0).contains(&point.y)
    {
        let at = map(*point);
        draw_straight(commands, at, at + Vec2::new(0.1, 0.0));
        return;
    }
    let mut previous = None;
    for points in points.windows(2) {
        if let Some((from, to)) = clip_line(points[0], points[1]) {
            let (from, to) = (map(from), map(to));
            if from == to {
                previous = None;
                continue;
            }
            if previous != Some(from) {
                commands.push(CanvasPathCommand::MoveTo(from));
            }
            commands.push(CanvasPathCommand::LineTo(to));
            previous = Some(to);
        } else {
            previous = None;
        }
    }
}

fn clip_line(from: DVec2, to: DVec2) -> Option<(DVec2, DVec2)> {
    let delta = to - from;
    if !delta.is_finite() {
        return None;
    }
    let (mut first, mut last) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-delta.x, from.x),
        (delta.x, 1.0 - from.x),
        (-delta.y, from.y),
        (delta.y, 1.0 - from.y),
    ] {
        if p < 0.0 {
            first = first.max(q / p);
        } else if p > 0.0 {
            last = last.min(q / p);
        } else if q < 0.0 {
            return None;
        }
    }
    (first <= last).then(|| {
        let start = (from + delta * first).clamp(DVec2::ZERO, DVec2::ONE);
        let end = (from + delta * last).clamp(DVec2::ZERO, DVec2::ONE);
        (start, end)
    })
}

fn fit(
    plots: Query<(Entity, &ComputedNode), (With<NativePlot>, Changed<ComputedNode>)>,
    mut commands: Commands,
) {
    for (entity, layout) in plots.iter() {
        let logical = layout.size() * layout.inverse_scale_factor();
        if logical.x <= 0.0 || logical.y <= 0.0 {
            continue;
        }
        CanvasView::set_zoom(
            &mut commands,
            entity,
            (logical.x / WIDTH).min(logical.y / HEIGHT).clamp(0.1, 10.0),
        );
    }
}
