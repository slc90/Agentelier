use super::{axis::Axis, construction, invalid};
use agentelier_agent_host::EcsError;
use bevy::{
    math::DVec2,
    prelude::*,
    ui::{InteractionDisabled, UiSystems},
};
use bevy_widgetry::{
    canvas::{
        CanvasDocument, CanvasEditToken, CanvasGeometry, CanvasLayerId, CanvasLineCap,
        CanvasLineJoin, CanvasPaint, CanvasPath, CanvasPathCommand, CanvasPlugin, CanvasStroke,
        CanvasText, CanvasTextStyle, CanvasView,
    },
    job::JobFailure,
    scene::spawn_scene,
    text::WidgetryText,
    theme::WidgetryTheme,
};
use std::io;

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
    pub paint: CanvasPaint,
}

impl Mark {
    pub fn geometry(&self) -> (CanvasPath, CanvasPaint) {
        path_geometry(self.commands.clone(), self.paint)
    }
}

pub(super) struct Content {
    pub title: Option<String>,
    pub x: Axis,
    pub y: Axis,
    pub marks: Vec<Mark>,
    pub baseline: Option<Baseline>,
}

#[derive(Clone, Copy)]
pub(super) struct Baseline {
    pub fraction: f64,
    pub value: f64,
}

impl Baseline {
    fn label(&self) -> String {
        format!("基线 {}", display_number(self.value))
    }

    fn geometry(&self, ink: Color) -> (CanvasPath, CanvasPaint) {
        let mut commands = Vec::new();
        if (0.0..=1.0).contains(&self.fraction) {
            for dash in 0..36 {
                let left = f64::from(dash) / 36.0;
                let right = (f64::from(dash) + 0.58) / 36.0;
                draw_straight(
                    &mut commands,
                    map(DVec2::new(left, self.fraction)),
                    map(DVec2::new(right, self.fraction)),
                );
            }
        }
        path_geometry(commands, stroke_paint(ink, 1.4))
    }
}

#[derive(Clone, Copy)]
pub(super) enum Style {
    Line,
    Scatter,
    Area,
    Bars,
}

pub(super) struct Rendered {
    pub root: Entity,
    pub document: Entity,
    pub layers: Vec<Layer>,
    reference: Option<Reference>,
}

struct Reference {
    path: Entity,
    note: Entity,
    ink: Color,
}

#[derive(Clone, Copy)]
pub(super) struct Layer {
    pub id: CanvasLayerId,
    pub path: Entity,
}

#[derive(Component)]
pub(super) struct RetainedPlot {
    pub rendered: Rendered,
    pub legends: Vec<Legend>,
    title: Option<String>,
    x: Axis,
    y: Axis,
}

impl RetainedPlot {
    pub fn build(
        world: &mut World,
        parent: Entity,
        content: Content,
        style: Style,
    ) -> std::result::Result<Entity, EcsError> {
        let title = content.title.clone();
        let (x, y) = (content.x.clone(), content.y.clone());
        let legends = content
            .marks
            .iter()
            .map(|mark| mark.legend.clone())
            .collect();
        let rendered = mount(world, parent, content, style)?;
        let root = rendered.root;
        world.entity_mut(root).insert(Self {
            rendered,
            title,
            x,
            y,
            legends,
        });
        Ok(root)
    }

    pub fn get(world: &World, root: Entity) -> Result<&Self> {
        world
            .get::<Self>(root)
            .ok_or_else(|| io::Error::other("图表缺少绘图状态").into())
    }

    pub fn select_series(world: &mut World, root: Entity, selected: Option<&str>) -> Result {
        let chart = Self::get(world, root)?;
        let document = chart.rendered.document;
        let layers = CanvasDocument::layers(world, document)?;
        let affected: Vec<_> = chart
            .rendered
            .layers
            .iter()
            .zip(&chart.legends)
            .filter_map(|(layer, legend)| {
                let visible = selected.is_none_or(|name| name == legend.name);
                let current = layers.iter().find(|entry| entry.id() == layer.id)?;
                (visible != current.visible()).then_some((layer.id, visible))
            })
            .collect();
        if !affected.is_empty() {
            CanvasDocument::edit(world, document, |world, token| {
                for (layer, visible) in affected {
                    CanvasDocument::set_layer_visible(world, token, layer, visible)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn update(world: &mut World, root: Entity, content: &Content) -> Result<bool> {
        let chart = Self::get(world, root)?;
        let rendered = &chart.rendered;
        if (chart.title.as_ref(), &chart.x, &chart.y)
            != (content.title.as_ref(), &content.x, &content.y)
            || !chart
                .legends
                .iter()
                .eq(content.marks.iter().map(|mark| &mark.legend))
            || rendered.reference.is_some() != content.baseline.is_some()
        {
            return Ok(false);
        }
        let document = rendered.document;
        let mut changed = Vec::new();
        for (layer, mark) in rendered.layers.iter().zip(&content.marks) {
            let (path, paint) = mark.geometry();
            keep_change(world, &mut changed, layer.path, path, paint)?;
        }
        let note = if let Some((reference, baseline)) =
            rendered.reference.as_ref().zip(content.baseline.as_ref())
        {
            let (path, paint) = baseline.geometry(reference.ink);
            keep_change(world, &mut changed, reference.path, path, paint)?;
            Some((reference.note, baseline.label()))
        } else {
            None
        };
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
        if let Some((entity, label)) = note
            && let Some(mut text) = world.get_mut::<Text>(entity)
        {
            text.set_if_neq(Text(label));
        }
        Ok(true)
    }
}

fn keep_change(
    world: &World,
    changed: &mut Vec<(Entity, CanvasPath, CanvasPaint)>,
    entity: Entity,
    path: CanvasPath,
    paint: CanvasPaint,
) -> Result {
    let Some(CanvasGeometry::Path(previous)) = world.get::<CanvasGeometry>(entity) else {
        return Err(io::Error::other("图表缺少已挂载的绘图路径").into());
    };
    if previous != &path || world.get::<CanvasPaint>(entity) != Some(&paint) {
        path.validate(&Transform::default(), &paint)?;
        changed.push((entity, path, paint));
    }
    Ok(())
}

pub(super) fn palette(index: usize) -> Color {
    let [r, g, b] = PALETTE[index % PALETTE.len()];
    Color::srgb_u8(r, g, b)
}

pub(super) fn contrast(color: Color) -> Color {
    let color = color.to_linear();
    let luminance = 0.2126 * color.red + 0.7152 * color.green + 0.0722 * color.blue;
    if luminance > 0.179 {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

pub(super) fn display_number(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if (1e-4..1e7).contains(&value.abs()) {
        value.to_string()
    } else {
        format!("{value:e}")
    }
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
    let name = match style {
        Style::Line => "NativeLineChart",
        Style::Scatter => "NativeScatterChart",
        Style::Area => "NativeAreaChart",
        Style::Bars => "NativeBarChart",
    };
    let root = root(world, parent, name, content.title.as_deref())?;
    let result = decorate(world, root, content, style);
    if let Err(error) = result {
        world.despawn(root);
        return Err(error);
    }
    result
}

pub(super) fn root(
    world: &mut World,
    parent: Entity,
    name: &'static str,
    title: Option<&str>,
) -> std::result::Result<Entity, EcsError> {
    if title.is_some_and(|title| title.trim().is_empty() || title.len() > 256) {
        return Err(invalid("title 必须非空且最多 256 字节"));
    }
    spawn_scene(world, bsn! {
        ChildOf(parent) Name(name)
        Node { width: percent(100), min_width: px(0), max_width: px(880), flex_direction: FlexDirection::Column, row_gap: px(5) }
    }).map_err(construction)
}

pub(super) fn document(world: &mut World, root: Entity) -> std::result::Result<Entity, EcsError> {
    let document = spawn_scene(
        world,
        bsn! { ChildOf(root) Name("ChartDocument") template(|_| Ok(CanvasDocument)) },
    )
    .map_err(construction)?;
    CanvasDocument::set_history_capacity(world, document, 0).map_err(construction)?;
    Ok(document)
}

pub(super) fn viewport(document: Entity) -> impl Scene {
    bsn! {
        @CanvasView { @document: {Some(document)} }
        NativePlot Name("NativePlotViewport") Pickable::IGNORE InteractionDisabled
        Node { width: percent(100), min_width: px(0), min_height: px(140), aspect_ratio: {Some(WIDTH / HEIGHT)} }
        template(|context| Ok(BackgroundColor(context.resource::<WidgetryTheme>().text_field.editable.normal.background)))
    }
}

pub(super) fn heading(title: String) -> impl Scene {
    bsn! {
        Text(title) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
        TextFont { font_size: bevy::text::FontSize::Px(19.0) }
    }
}

fn decorate(
    world: &mut World,
    root: Entity,
    content: Content,
    style: Style,
) -> std::result::Result<Rendered, EcsError> {
    let Content {
        title,
        x,
        y,
        marks,
        baseline,
    } = content;
    let document = document(world, root)?;
    let theme = world.resource::<WidgetryTheme>();
    let ink = theme.text.normal.foreground;
    let grid = theme.text_field.editable.normal.border;
    let (layers, reference) = CanvasDocument::edit(world, document, |world, token| {
        axes(world, token, &x, &y, ink, grid)?;
        let reference = if let Some(baseline) = &baseline {
            let (path, paint) = baseline.geometry(ink);
            let entity =
                CanvasDocument::create_path(world, token, path, Transform::default(), paint)?;
            world.entity_mut(entity).insert(Name::new("ChartBaseline"));
            Some(entity)
        } else {
            None
        };
        let mut layers = Vec::new();
        for (index, mark) in marks.iter().enumerate() {
            let transform = Transform::default();
            if matches!(style, Style::Line) && mark.commands.is_empty() {
                continue;
            }
            let (geometry, paint) = mark.geometry();
            let path = match style {
                Style::Line => {
                    CanvasDocument::create_path(world, token, geometry, transform, paint)?
                }
                Style::Scatter | Style::Area | Style::Bars => {
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
        Ok((layers, reference))
    })
    .map_err(construction)?;
    let y_name = y.name;
    let x_name = x.name;
    let title = title.map(heading);
    let (legend_width, legend_height) = match style {
        Style::Line => (18., 3.),
        Style::Scatter => (8., 8.),
        Style::Area => (18., 8.),
        Style::Bars => (11., 11.),
    };
    let legend: Vec<_> = marks
        .into_iter()
        .map(|mark| legend(mark.legend, legend_width, legend_height))
        .collect();
    let legend = bevy::scene::RelatedScenes::<Children>::new(legend);
    spawn_scene(world, bsn! {
        ChildOf(root)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{title}--
            Text(y_name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)--
            @viewport(document)--
            Text(x_name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
            Node { align_self: AlignSelf::Center }--
            Node { min_width: px(0), column_gap: px(14), row_gap: px(4), flex_wrap: FlexWrap::Wrap }
            @legend
        ]
    }).map_err(construction)?;
    let reference = if let Some((path, baseline)) = reference.zip(baseline) {
        let caption = baseline.label();
        let note = spawn_scene(world, bsn! {
            ChildOf(root) Name("ChartBaselineLabel")
            Text(caption) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
        }).map_err(construction)?;
        Some(Reference { path, note, ink })
    } else {
        None
    };
    Ok(Rendered {
        root,
        document,
        layers,
        reference,
    })
}

pub(super) fn legend(item: Legend, width: f32, height: f32) -> impl Scene {
    let Legend { name, color } = item;
    bsn! {
        Node { min_width: px(0), max_width: percent(100), align_items: AlignItems::Center, column_gap: px(6), overflow: Overflow::clip_x() }
        Children [
            Node { width: px(width), height: px(height), flex_shrink: 0.0, border_radius: BorderRadius::MAX }
            BackgroundColor(color)--
            Text(name) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::NoWrap)
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
            x.category_count
                .map(|count| (TOP_RIGHT.x - BOTTOM_LEFT.x) * 0.93 / count as f32),
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
            y.category_count.map(|_| 66.0),
        )?;
    }
    stroke(world, token, grid_lines, grid, 0.7)?;
    stroke(world, token, axes, ink, 1.2)?;
    Ok(())
}

pub(super) fn label(
    world: &mut World,
    token: CanvasEditToken,
    text: &str,
    at: Vec2,
    color: Color,
    width: Option<f32>,
) -> std::result::Result<(), JobFailure> {
    let entity = CanvasDocument::create_text(
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
    if let Some(width) = width
        && let Some(text) = world.get::<CanvasText>(entity)
        && text.size().x > width
    {
        let scale = width / text.size().x;
        let transform =
            Transform::from_translation(at.extend(0.0)).with_scale(Vec3::new(scale, scale, 1.0));
        CanvasDocument::update_transform(world, token, entity, transform)?;
    }
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

pub(super) fn stroke_paint(color: Color, width: f32) -> CanvasPaint {
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

fn path_geometry(
    commands: Vec<CanvasPathCommand>,
    paint: CanvasPaint,
) -> (CanvasPath, CanvasPaint) {
    if commands.is_empty() {
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
    (CanvasPath { commands }, paint)
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

pub(super) fn append_polygon(commands: &mut Vec<CanvasPathCommand>, mut points: Vec<DVec2>) {
    for (axis, edge, retain_higher) in [
        (0, 0.0, true),
        (0, 1.0, false),
        (1, 0.0, true),
        (1, 1.0, false),
    ] {
        let input = std::mem::take(&mut points);
        let inside = |point: DVec2| {
            if retain_higher {
                point[axis] >= edge
            } else {
                point[axis] <= edge
            }
        };
        let Some(&last) = input.last() else { return };
        let mut from = last;
        for to in input {
            if inside(from) != inside(to) {
                points.push(intersection(from, to, axis, edge));
            }
            if inside(to) {
                points.push(to);
            }
            from = to;
        }
    }
    let mut points: Vec<_> = points.into_iter().map(map).collect();
    points.dedup();
    if points.first() == points.last() {
        points.pop();
    }
    let [first, remaining @ ..] = points.as_slice() else {
        return;
    };
    let area = remaining
        .windows(2)
        .map(|pair| {
            (pair[0] - *first)
                .as_dvec2()
                .perp_dot((pair[1] - *first).as_dvec2())
        })
        .sum::<f64>();
    if area.abs() > 0.00001 {
        commands.push(CanvasPathCommand::MoveTo(*first));
        commands.extend(remaining.iter().copied().map(CanvasPathCommand::LineTo));
        commands.push(CanvasPathCommand::Close);
    }
}

pub(super) fn intersection(from: DVec2, to: DVec2, axis: usize, edge: f64) -> DVec2 {
    let scale = from[axis]
        .abs()
        .max(to[axis].abs())
        .max(edge.abs())
        .max(1.0);
    let start = from[axis] / scale;
    let t = ((edge / scale - start) / (to[axis] / scale - start)).clamp(0.0, 1.0);
    let mut point = from * (1.0 - t) + to * t;
    point[axis] = edge;
    point
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
