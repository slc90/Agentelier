use super::{
    super::{Drawing, line, plot},
    model::{Caption, Edge, Model, Node},
};
use bevy::prelude::*;
use bevy_widgetry::canvas::{
    CanvasFill,
    CanvasPathCommand::{self, Close, LineTo, MoveTo},
};
use std::collections::BTreeMap;

impl Model {
    pub fn drawing(&self) -> (Drawing, Vec<(String, Vec2)>) {
        let mut normal = Vec::new();
        let mut movable = Vec::new();
        let mut lines = Vec::new();
        let mut labels = Vec::new();
        let mut math = Vec::new();
        let mut repeated: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for edge in &self.edges {
            let key = (edge.from.min(edge.to), edge.from.max(edge.to));
            repeated.entry(key).or_default().push(edge);
        }
        for edges in repeated.into_values() {
            let n = edges.len() as f32;
            for (index, edge) in edges.into_iter().enumerate() {
                let offset = index as f32 - (n - 1.0) * 0.5;
                let label = connection(&mut lines, edge, &self.nodes, offset);
                caption(&edge.caption, label, &mut labels, &mut math);
            }
        }
        for node in &self.nodes {
            let brush = if node.draggable {
                &mut movable
            } else {
                &mut normal
            };
            let [left, top] = (node.at - node.half).to_array();
            let [right, bottom] = (node.at + node.half).to_array();
            brush.extend([
                MoveTo(Vec2::new(left, top)),
                LineTo(Vec2::new(right, top)),
                LineTo(Vec2::new(right, bottom)),
                LineTo(Vec2::new(left, bottom)),
                Close,
            ]);
            caption(&node.caption, node.at, &mut labels, &mut math);
        }
        let frame = |color| {
            let mut paint = plot::stroke_paint(color, 1.6);
            paint.fill = Some(CanvasFill::Solid(Color::srgb_u8(244, 250, 247)));
            paint
        };
        let green = plot::palette(0);
        let drawing = Drawing {
            paths: vec![
                (lines, plot::stroke_paint(green, 2.0)),
                (normal, frame(green)),
                (movable, frame(plot::palette(1))),
            ],
            labels,
        };
        (drawing, math)
    }
}

fn caption(
    caption: &Caption,
    at: Vec2,
    labels: &mut Vec<(String, Vec2)>,
    formulas: &mut Vec<(String, Vec2)>,
) {
    match caption {
        Caption::Plain(label) => labels.push((label.clone(), at)),
        Caption::Tex(tex) => formulas.push((tex.clone(), at)),
        Caption::None => {}
    }
}

fn connection(
    commands: &mut Vec<CanvasPathCommand>,
    edge: &Edge,
    nodes: &[Node],
    offset: f32,
) -> Vec2 {
    let [from, to] = [&nodes[edge.from], &nodes[edge.to]];
    if from.at.distance_squared(to.at) < 1e-5 {
        return self_loop(commands, from, edge.directed, offset);
    }
    let axis = to.at - from.at;
    let perpendicular = (axis.normalize() * if edge.from > edge.to { -1.0 } else { 1.0 }).perp();
    let control = (from.at + to.at) * 0.5 + perpendicular * offset * 31.0;
    let a = border(from, control - from.at);
    let b = border(to, control - to.at);
    let point = |t: f32| a.lerp(control, t).lerp(control.lerp(b, t), t);
    curve(commands, point, edge.directed);
    let sign = if offset == 0.0 && perpendicular.y.abs() >= 0.6 {
        -perpendicular.y.signum()
    } else if offset < 0.0 {
        -1.0
    } else {
        1.0
    };
    (point(0.5) + perpendicular * sign * 17.0)
        .clamp(Vec2::new(-293.0, -123.0), Vec2::new(293.0, 123.0))
}

fn border(node: &Node, direction: Vec2) -> Vec2 {
    let t = (node.half / direction.abs()).min_element();
    node.at + direction * t
}

fn self_loop(
    commands: &mut Vec<CanvasPathCommand>,
    node: &Node,
    directed: bool,
    offset: f32,
) -> Vec2 {
    let side = if node.at.y < -37.0 { 1.0 } else { -1.0 };
    let rise = side * (55.0 + offset.abs() * 9.0).min(76.0);
    let a = node.at + Vec2::new(node.half.x * 0.65, node.half.y * side);
    let b = node.at + Vec2::new(-node.half.x * 0.65, node.half.y * side);
    let u = node.at + Vec2::new(44.0 + offset * 14.0, rise);
    let v = node.at + Vec2::new(-44.0 + offset * 14.0, rise);
    let point = |t: f32| {
        let q = u.lerp(v, t);
        a.lerp(u, t).lerp(q, t).lerp(q.lerp(v.lerp(b, t), t), t)
    };
    curve(commands, point, directed);
    (point(0.5) + Vec2::new(0.0, side * 14.0))
        .clamp(Vec2::new(-294.0, -124.0), Vec2::new(294.0, 124.0))
}

fn curve(commands: &mut Vec<CanvasPathCommand>, point: impl Fn(f32) -> Vec2, directed: bool) {
    commands.push(MoveTo(point(0.0)));
    commands.extend((1..=24).map(|i| LineTo(point(i as f32 / 24.0))));
    if directed {
        let end = point(1.0);
        let heading = (end - point(0.94)).normalize_or_zero();
        let base = end - heading * 10.0;
        let wing = heading.perp() * 5.0;
        line(commands, base + wing, end);
        line(commands, end, base - wing);
    }
}
