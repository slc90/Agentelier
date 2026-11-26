use super::{
    super::{arc, arrow, circle, line, plot},
    Drawing,
    model::{Model, Object},
};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::canvas::{
    CanvasFill, CanvasPaint,
    CanvasPathCommand::{self, Close, LineTo, MoveTo},
};
use std::f32::consts::{PI, TAU};

impl Model {
    pub fn drawing(&self) -> Drawing {
        let mapped: Vec<_> = self
            .locations
            .iter()
            .map(|&point| self.plane.map(point))
            .collect();
        let mut pens = Pens::default();
        for (point, &at) in self.points.iter().zip(&mapped) {
            if point.draggable {
                circle(&mut pens.draggable, at, 5.7);
            } else {
                circle(&mut pens.fixed, at, 3.4);
            }
            pens.label(
                point.label.clone(),
                at + Vec2::new(
                    10.0 + point.label.chars().count().min(14) as f32 * 3.0,
                    -12.0,
                ),
            );
        }
        for object in &self.objects {
            match object {
                Object::Segment(from, to, is_arrow) => {
                    let render = if *is_arrow { arrow } else { line };
                    render(&mut pens.lines, mapped[*from], mapped[*to]);
                }
                Object::Circle(center, radius) => circle(
                    &mut pens.lines,
                    mapped[*center],
                    (radius * self.plane.scale) as f32,
                ),
                Object::Angle([a, center, b]) => {
                    angle(&mut pens, [mapped[*a], mapped[*center], mapped[*b]])
                }
                Object::Polygon(indices, area) => {
                    let points: Vec<_> = indices.iter().map(|&index| mapped[index]).collect();
                    let [first, rest @ ..] = points.as_slice() else {
                        continue;
                    };
                    let simple = simple_polygon(&points);
                    let target = if simple {
                        &mut pens.polygons
                    } else {
                        &mut pens.lines
                    };
                    target.push(MoveTo(*first));
                    target.extend(rest.iter().copied().map(LineTo));
                    target.push(Close);
                    if *area {
                        pens.label(polygon_area(self, indices, simple), average(&points));
                    }
                }
                Object::Decomposition(a, b) => {
                    let [from, to] = [mapped[*a], mapped[*b]];
                    let elbow = Vec2::new(to.x, from.y);
                    let delta = self.locations[*b] - self.locations[*a];
                    arrow(&mut pens.lines, from, to);
                    dashed(&mut pens.accent, from, elbow);
                    dashed(&mut pens.accent, elbow, to);
                    if from.x != to.x {
                        pens.label(
                            format!("Δx = {}", number(delta.x)),
                            (from + elbow) * 0.5 + Vec2::new(0.0, 16.0),
                        );
                    } else {
                        pens.label("Δx = 0".into(), from + Vec2::new(12.0, 26.0));
                    }
                    if from.y != to.y {
                        pens.label(
                            format!("Δy = {}", number(delta.y)),
                            (elbow + to) * 0.5 + Vec2::new(38.0, 0.0),
                        );
                    } else {
                        pens.label("Δy = 0".into(), elbow + Vec2::new(26.0, 14.0));
                    }
                }
            }
        }
        pens.drawing()
    }
}

#[derive(Default)]
struct Pens {
    polygons: Vec<CanvasPathCommand>,
    lines: Vec<CanvasPathCommand>,
    accent: Vec<CanvasPathCommand>,
    fixed: Vec<CanvasPathCommand>,
    draggable: Vec<CanvasPathCommand>,
    labels: Vec<(String, Vec2)>,
    occupied: Vec<Rect>,
}

impl Pens {
    fn label(&mut self, label: String, position: Vec2) {
        let width = label
            .chars()
            .map(|ch| if ch.is_ascii() { 8.0 } else { 15.0 })
            .sum::<f32>();
        let size = Vec2::new((width + 8.0).min(624.0), 22.0);
        let extent = Vec2::new(315.0, 138.0) - size * 0.5;
        let place = |offset: Vec2| (position + offset).clamp(-extent, extent);
        let axes = [
            Vec2::NEG_Y,
            Vec2::Y,
            Vec2::X,
            Vec2::NEG_X,
            Vec2::new(1.0, -1.0),
            Vec2::new(-1.0, -1.0),
            Vec2::ONE,
            Vec2::new(-1.0, 1.0),
        ];
        let offsets = std::iter::once(Vec2::ZERO).chain((1..=6).flat_map(|step| {
            axes.into_iter()
                .map(move |axis| axis * Vec2::new(24.0, 22.0) * step as f32)
        }));
        let at = offsets
            .map(place)
            .find(|&at| {
                let area = Rect::from_center_size(at, size);
                self.occupied
                    .iter()
                    .all(|other| other.intersect(area).is_empty())
            })
            .unwrap_or_else(|| place(Vec2::ZERO));
        self.occupied.push(Rect::from_center_size(at, size));
        self.labels.push((label, at));
    }

    fn drawing(self) -> Drawing {
        let line_color = plot::palette(0);
        let accent = plot::palette(1);
        let mut polygon = plot::stroke_paint(line_color, 1.65);
        polygon.fill = Some(CanvasFill::Solid(line_color.with_alpha(0.18)));
        let fixed = CanvasPaint {
            fill: Some(CanvasFill::Solid(line_color)),
            ..default()
        };
        let mut draggable = plot::stroke_paint(accent, 2.0);
        draggable.fill = Some(CanvasFill::Solid(Color::WHITE));
        Drawing {
            paths: vec![
                (self.polygons, polygon),
                (self.lines, plot::stroke_paint(line_color, 2.0)),
                (self.accent, plot::stroke_paint(accent, 1.7)),
                (self.fixed, fixed),
                (self.draggable, draggable),
            ],
            labels: self.labels,
        }
    }
}

fn angle(pens: &mut Pens, [from, center, to]: [Vec2; 3]) {
    let (a, b) = (
        (from - center).normalize_or_zero(),
        (to - center).normalize_or_zero(),
    );
    if a == Vec2::ZERO || b == Vec2::ZERO {
        pens.label("角未定义".into(), center + Vec2::new(30.0, 17.0));
        return;
    }
    let start = a.to_angle();
    let turn = (b.to_angle() - start + PI).rem_euclid(TAU) - PI;
    let radius = (from.distance(center).min(to.distance(center)) * 0.34).min(27.0);
    if turn.abs() > f32::EPSILON {
        arc(&mut pens.accent, center, radius, start, turn, 22);
    }
    let at = center + Vec2::from_angle(start + turn * 0.5) * (radius + 21.0);
    pens.label(format!("{:.1}°", turn.to_degrees().abs()), at);
}

fn dashed(commands: &mut Vec<CanvasPathCommand>, from: Vec2, to: Vec2) {
    let offset = to - from;
    let length = offset.length();
    if length <= 0.0 || !length.is_finite() {
        return;
    }
    let direction = offset / length;
    let pieces = (length / 9.0).ceil().min(120.0) as usize;
    for step in 0..pieces {
        let start = step as f32 * length / pieces as f32;
        let end = ((step as f32 + 0.56) * length / pieces as f32).min(length);
        line(commands, from + direction * start, from + direction * end);
    }
}

fn polygon_area(model: &Model, indices: &[usize], simple: bool) -> String {
    if !simple {
        return "面积未定义".into();
    }
    let first = model.locations[indices[0]];
    let area = indices[1..]
        .windows(2)
        .map(|pair| {
            let a = model.locations[pair[0]] - first;
            let b = model.locations[pair[1]] - first;
            a.perp_dot(b) * 0.5
        })
        .sum::<f64>()
        .abs();
    if area.is_finite() {
        format!("面积 = {}", number(area))
    } else {
        "面积未定义".into()
    }
}

fn simple_polygon(points: &[Vec2]) -> bool {
    let edges: Vec<_> = (0..points.len())
        .map(|i| {
            [
                points[i].as_dvec2(),
                points[(i + 1) % points.len()].as_dvec2(),
            ]
        })
        .collect();
    for (index, &a) in edges.iter().enumerate() {
        for (other, &b) in edges.iter().enumerate().skip(index + 2) {
            if index == 0 && other == edges.len() - 1 {
                continue;
            }
            if intersects(a, b) {
                return false;
            }
        }
    }
    true
}

fn intersects([a, b]: [DVec2; 2], [c, d]: [DVec2; 2]) -> bool {
    let orientation = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a);
    let [u, v, x, y] = [
        orientation(a, b, c),
        orientation(a, b, d),
        orientation(c, d, a),
        orientation(c, d, b),
    ];
    let contains =
        |a: DVec2, b: DVec2, p: DVec2| p.cmpge(a.min(b)).all() && p.cmple(a.max(b)).all();
    (u * v < 0.0 && x * y < 0.0)
        || (u == 0.0 && contains(a, b, c))
        || (v == 0.0 && contains(a, b, d))
        || (x == 0.0 && contains(c, d, a))
        || (y == 0.0 && contains(c, d, b))
}

fn average(points: &[Vec2]) -> Vec2 {
    let n = points.len() as f32;
    points
        .iter()
        .fold(Vec2::ZERO, |sum, &point| sum + point / n)
}

fn number(value: f64) -> String {
    if value != 0.0 && !(0.001..1e6).contains(&value.abs()) {
        return format!("{value:.2e}");
    }
    let num = format!("{value:.3}");
    num.trim_end_matches('0').trim_end_matches('.').to_owned()
}
