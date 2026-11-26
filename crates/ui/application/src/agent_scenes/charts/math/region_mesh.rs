use super::level_grid::{Grid, Sample, edge_crossing};
use bevy::math::DVec2;
use std::collections::{BTreeMap, BTreeSet};

type Point = [u64; 2];
type Edge = (Point, Point);

pub(super) fn filled_paths(
    grid: &Grid,
    mut evaluate: impl FnMut(DVec2) -> Option<f64>,
) -> Vec<Vec<DVec2>> {
    let mut boundary = Boundary::default();
    let mut intersections = BTreeMap::new();
    for corners in grid.cells() {
        let middle = corners[0].position * 0.5 + corners[2].position * 0.5;
        let Some(score) = evaluate(middle) else {
            continue;
        };
        if score <= 0.0 && corners.iter().all(|sample| inside(*sample)) {
            boundary.include(&corners.map(|sample| sample.position));
            continue;
        }
        let center = Sample::at(middle, Some(score));
        for index in 0..4 {
            let triangle = [corners[index], corners[(index + 1) % 4], center];
            if triangle.iter().any(|sample| sample.value.is_none())
                || !triangle.iter().any(|sample| inside(*sample))
            {
                continue;
            }
            if let Some(polygon) = clip_triangle(triangle, &mut intersections, &mut evaluate) {
                boundary.include(&polygon);
            }
        }
    }
    boundary.loops()
}

fn inside(sample: Sample) -> bool {
    sample.value.is_some_and(|value| value <= 0.0)
}

fn clip_triangle(
    triangle: [Sample; 3],
    intersections: &mut BTreeMap<Edge, Option<DVec2>>,
    evaluate: &mut impl FnMut(DVec2) -> Option<f64>,
) -> Option<Vec<DVec2>> {
    let mut result = Vec::with_capacity(4);
    let mut previous = triangle[2];
    for next in triangle {
        let contained = inside(next);
        if contained != inside(previous) {
            let mut samples = [previous, next];
            samples.sort_by_key(|sample| key(sample.position));
            let [left, right] = samples.map(|sample| key(sample.position));
            let cross = *intersections
                .entry((left, right))
                .or_insert_with(|| edge_crossing(samples, evaluate));
            result.push(cross?);
        }
        if contained {
            result.push(next.position);
        }
        previous = next;
    }
    result.dedup();
    if result.first() == result.last() {
        result.pop();
    }
    (result.len() >= 3).then_some(result)
}

#[derive(Default)]
struct Boundary(BTreeSet<Edge>);

impl Boundary {
    fn include(&mut self, points: &[DVec2]) {
        let Some(&last) = points.last() else {
            return;
        };
        let mut previous = key(last);
        for &point in points {
            let next = key(point);
            if previous != next && !self.0.remove(&(next, previous)) {
                self.0.insert((previous, next));
            }
            previous = next;
        }
    }

    fn loops(mut self) -> Vec<Vec<DVec2>> {
        let mut paths = Vec::new();
        while let Some((start, next)) = self.0.pop_first() {
            let mut points = vec![position(start), position(next)];
            let mut previous = start;
            let mut current = next;
            while current != start {
                let incoming = position(current) - position(previous);
                let next = self
                    .0
                    .range((current, [0; 2])..=(current, [u64::MAX; 2]))
                    .map(|(_, end)| *end)
                    .max_by(|&a, &b| {
                        turn(incoming, position(a) - position(current))
                            .total_cmp(&turn(incoming, position(b) - position(current)))
                    });
                let Some(next) = next else {
                    break;
                };
                self.0.remove(&(current, next));
                points.push(position(next));
                previous = current;
                current = next;
            }
            if current == start && points.len() >= 4 {
                paths.push(points);
            }
        }
        paths
    }
}

fn key(point: DVec2) -> Point {
    point
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

fn position(key: Point) -> DVec2 {
    DVec2::from_array(key.map(f64::from_bits))
}

fn turn(a: DVec2, b: DVec2) -> f64 {
    a.perp_dot(b).atan2(a.dot(b))
}
