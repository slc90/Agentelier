use super::{Axis, BOTTOM_LEFT, TOP_RIGHT, draw_straight, label, map, stroke};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::{
    canvas::{CanvasEditToken, CanvasPathCommand},
    job::JobFailure,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

const RADIUS: f64 = 104.0;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::agent_scenes::charts) enum AngleUnit {
    Radians,
    Degrees,
}

impl AngleUnit {
    pub fn period(self) -> f64 {
        match self {
            Self::Radians => TAU,
            Self::Degrees => 360.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Radians => "rad",
            Self::Degrees => "°",
        }
    }

    pub fn radians(self, theta: f64) -> f64 {
        theta.rem_euclid(self.period()) * (TAU / self.period())
    }

    fn label(self, eighth: usize) -> &'static str {
        match self {
            Self::Radians => ["0", "π/4", "π/2", "3π/4", "π", "5π/4", "3π/2", "7π/4"][eighth],
            Self::Degrees => ["0°", "45°", "90°", "135°", "180°", "225°", "270°", "315°"][eighth],
        }
    }
}

pub(in crate::agent_scenes::charts) fn point(at: DVec2) -> DVec2 {
    let size = (TOP_RIGHT - BOTTOM_LEFT).abs().as_dvec2();
    DVec2::splat(0.5) + at * RADIUS / size
}

pub(super) fn axes(
    world: &mut World,
    token: CanvasEditToken,
    radius: &Axis,
    unit: AngleUnit,
    ink: Color,
    grid: Color,
) -> Result<(), JobFailure> {
    let center = map(point(DVec2::ZERO));
    let mut paths = Vec::new();
    for fraction in radius
        .ticks
        .iter()
        .map(|tick| tick.fraction)
        .chain([1.0])
        .filter(|&fraction| fraction > 0.0)
    {
        for i in 0..72 {
            let angle = f64::from(i) * TAU / 72.0;
            let at = map(point(DVec2::from_angle(angle) * fraction));
            paths.push(if i == 0 {
                CanvasPathCommand::MoveTo(at)
            } else {
                CanvasPathCommand::LineTo(at)
            });
        }
        paths.push(CanvasPathCommand::Close);
    }
    for index in 0..8 {
        let direction = DVec2::from_angle(index as f64 * TAU / 8.0);
        draw_straight(&mut paths, center, map(point(direction)));
        label(
            world,
            token,
            unit.label(index),
            map(point(direction * 1.20)),
            ink,
            None,
        )?;
    }
    for tick in radius.ticks.iter().filter(|tick| tick.fraction > 0.0) {
        let at = map(point(DVec2::new(tick.fraction, 0.0))) + Vec2::new(0.0, -11.0);
        label(world, token, &tick.label, at, ink, None)?;
    }
    stroke(world, token, paths, grid, 0.9)?;
    Ok(())
}
