use super::{
    super::{
        super::{
            axis::Axis,
            plot::{self, Content, Legend, LegendSymbol},
        },
        space_view::{Triangles, world_point},
    },
    COLOR, Mark,
};
use bevy::{math::DVec3, prelude::*};
use bevy_widgetry::canvas::CanvasPathCommand;

pub(super) fn clip(from: DVec3, to: DVec3) -> Option<DVec3> {
    if !to.is_finite() {
        return None;
    }
    let delta = to - from;
    let factor = (0..3).fold(1.0_f64, |factor, axis| {
        let target = to[axis];
        let edge = target.clamp(0.0, 1.0);
        if edge == target {
            factor
        } else {
            factor.min((edge - from[axis]) / delta[axis])
        }
    });
    Some((from + delta * factor).clamp(DVec3::ZERO, DVec3::ONE))
}

pub(super) fn planar(title: Option<String>, x: Axis, y: Axis, marks: &[Mark]) -> Content {
    let mut arrows = Vec::new();
    let mut zeros = Vec::new();
    for mark in marks {
        match *mark {
            Mark::Arrow { from, to } => append_arrow(
                &mut arrows,
                plot::map(from.truncate()),
                plot::map(to.truncate()),
            ),
            Mark::Zero(at) => plot::append_marker(&mut zeros, at.truncate()),
        }
    }
    let color = color();
    let marks = vec![
        plot::Mark {
            legend: Legend {
                name: "向量".into(),
                color,
            },
            symbol: LegendSymbol::Hidden,
            commands: arrows,
            paint: plot::stroke_paint(color, 1.75),
        },
        plot::Mark {
            legend: Legend {
                name: "零向量".into(),
                color,
            },
            symbol: LegendSymbol::Hidden,
            commands: zeros,
            paint: plot::stroke_paint(color, 6.6),
        },
    ];
    Content {
        title,
        x,
        y,
        marks,
        baseline: None,
    }
}

fn append_arrow(commands: &mut Vec<CanvasPathCommand>, from: Vec2, to: Vec2) {
    use CanvasPathCommand::{LineTo, MoveTo};
    let axis = to - from;
    let direction = axis.normalize_or_zero();
    if direction == Vec2::ZERO {
        return;
    }
    let size = (axis.length() * 0.42).min(7.5);
    let head = to - direction * size;
    let cross = direction.perp() * size * 0.55;
    commands.extend([
        MoveTo(from),
        LineTo(to),
        MoveTo(head + cross),
        LineTo(to),
        LineTo(head - cross),
    ]);
}

pub(super) fn spatial(marks: &[Mark]) -> Triangles {
    let mut mesh = Triangles::default();
    let color = color();
    let flat = [color.to_linear().to_f32_array(); 3];
    for mark in marks {
        match *mark {
            Mark::Arrow { from, to } => {
                mesh.arrow(world_point(from), world_point(to), 0.020, color)
            }
            Mark::Zero(point) => {
                let at = world_point(point);
                let ring = [Vec3::X, Vec3::Z, -Vec3::X, -Vec3::Z].map(|point| at + point * 0.035);
                for i in 0..4 {
                    let [a, b] = [ring[i], ring[(i + 1) % 4]];
                    mesh.push([a, b, at + Vec3::Y * 0.035], flat);
                    mesh.push([b, a, at - Vec3::Y * 0.035], flat);
                }
            }
        }
    }
    mesh
}

fn color() -> Color {
    let [r, g, b] = COLOR;
    Color::srgb_u8(r, g, b)
}
