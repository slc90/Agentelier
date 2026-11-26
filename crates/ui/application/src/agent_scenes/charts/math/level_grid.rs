use bevy::math::DVec2;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) struct Sample {
    index: usize,
    pub position: DVec2,
    pub value: Option<f64>,
}

impl Sample {
    pub(super) fn at(position: DVec2, value: Option<f64>) -> Self {
        Self {
            index: usize::MAX,
            position,
            value,
        }
    }
}

pub(super) struct Grid {
    columns: usize,
    rows: usize,
    samples: Vec<Sample>,
}

impl Grid {
    pub(super) fn sample(
        bounds: [DVec2; 2],
        columns: usize,
        rows: usize,
        mut evaluate: impl FnMut(DVec2) -> Option<f64>,
    ) -> Self {
        let len = (columns + 1) * (rows + 1);
        let mut samples = Vec::with_capacity(len);
        for y in 0..=rows {
            for x in 0..=columns {
                let fraction = DVec2::new(x as f64 / columns as f64, y as f64 / rows as f64);
                let position = bounds[0] * (DVec2::ONE - fraction) + bounds[1] * fraction;
                samples.push(Sample {
                    index: samples.len(),
                    position,
                    value: evaluate(position),
                });
            }
        }
        Self {
            columns,
            rows,
            samples,
        }
    }

    pub(super) fn cells(&self) -> impl Iterator<Item = [Sample; 4]> + '_ {
        (0..self.rows).flat_map(move |y| {
            (0..self.columns).map(move |x| {
                let stride = self.columns + 1;
                let start = stride * y + x;
                [
                    self.samples[start],
                    self.samples[start + 1],
                    self.samples[start + stride + 1],
                    self.samples[start + stride],
                ]
            })
        })
    }

    pub(super) fn contours(
        &self,
        mut evaluate: impl FnMut(DVec2) -> Option<f64>,
    ) -> Vec<Vec<DVec2>> {
        let mut crossings = BTreeMap::new();
        let mut paths = PathGraph::default();
        for cell in self.cells() {
            if cell.iter().any(|sample| sample.value.is_none()) {
                continue;
            }
            let mut edges = [None; 4];
            let mut valid_crossings = true;
            for i in 0..4 {
                let mut pair = [cell[i], cell[(i + 1) % 4]];
                if pair[0].value.is_some_and(|value| value >= 0.0)
                    == pair[1].value.is_some_and(|value| value >= 0.0)
                {
                    continue;
                }
                pair.sort_by_key(|sample| sample.index);
                let key = pair.map(|sample| sample.index);
                edges[i] = *crossings
                    .entry(key)
                    .or_insert_with(|| edge_crossing(pair, &mut evaluate));
                valid_crossings &= edges[i].is_some();
            }
            if !valid_crossings {
                continue;
            }
            match edges {
                [Some(a), Some(b), Some(c), Some(d)] => {
                    let center = cell[0].position * 0.5 + cell[2].position * 0.5;
                    let Some(value) = evaluate(center) else {
                        continue;
                    };
                    if (value >= 0.0) == cell[0].value.is_some_and(|corner| corner >= 0.0) {
                        paths.connect(a, b);
                        paths.connect(c, d);
                    } else {
                        paths.connect(a, d);
                        paths.connect(b, c);
                    }
                }
                incomplete => {
                    let hits: Vec<_> = incomplete.into_iter().flatten().collect();
                    if let [a, b] = hits.as_slice() {
                        paths.connect(*a, *b);
                    }
                }
            }
        }
        paths.finish()
    }
}

pub(super) fn edge_crossing(
    [first, last]: [Sample; 2],
    evaluate: &mut impl FnMut(DVec2) -> Option<f64>,
) -> Option<DVec2> {
    let (mut a, mut b) = (first.position, last.position);
    let (mut fa, mut fb) = (first.value?, last.value?);
    if fa == 0.0 {
        return Some(a);
    }
    if fb == 0.0 {
        return Some(b);
    }
    if (fa >= 0.0) == (fb >= 0.0) {
        return None;
    }
    let initial_minimum = fa.abs().min(fb.abs());
    let initial_maximum = fa.abs().max(fb.abs());
    for _ in 0..24 {
        let middle = a * 0.5 + b * 0.5;
        let value = evaluate(middle)?;
        if value == 0.0 {
            return Some(middle);
        }
        if (value >= 0.0) == (fa >= 0.0) {
            a = middle;
            fa = value;
        } else {
            b = middle;
            fb = value;
        }
    }
    let (best, error) = if fa.abs() < fb.abs() {
        (a, fa.abs())
    } else {
        (b, fb.abs())
    };
    let enclosure = fa.abs().max(fb.abs());
    // An unresolved sign change can be a pole or jump instead of a zero.
    (error < initial_minimum * 0.025
        || (error <= initial_minimum && enclosure < initial_maximum * 0.001))
        .then_some(best)
}

#[derive(Default)]
struct PathGraph {
    vertices: Vec<DVec2>,
    indices: BTreeMap<[u64; 2], usize>,
    adjacency: Vec<Vec<usize>>,
    edges: Vec<[usize; 2]>,
    present: BTreeSet<[usize; 2]>,
}

impl PathGraph {
    fn vertex(&mut self, point: DVec2) -> usize {
        let key = [point.x.to_bits(), point.y.to_bits()];
        *self.indices.entry(key).or_insert_with(|| {
            let index = self.vertices.len();
            self.vertices.push(point);
            self.adjacency.push(Vec::new());
            index
        })
    }

    fn connect(&mut self, start: DVec2, end: DVec2) {
        if start == end {
            return;
        }
        let mut pair = [self.vertex(start), self.vertex(end)];
        pair.sort_unstable();
        if !self.present.insert(pair) {
            return;
        }
        let edge = self.edges.len();
        self.adjacency[pair[0]].push(edge);
        self.adjacency[pair[1]].push(edge);
        self.edges.push(pair);
    }

    fn finish(self) -> Vec<Vec<DVec2>> {
        let mut used = vec![false; self.edges.len()];
        let mut paths = Vec::new();
        for (start, adjacent) in self
            .adjacency
            .iter()
            .enumerate()
            .filter(|(_, edges)| edges.len() != 2)
        {
            for &edge in adjacent {
                if !used[edge] {
                    paths.push(self.follow(start, edge, &mut used));
                }
            }
        }
        for (edge, pair) in self.edges.iter().enumerate() {
            if !used[edge] {
                paths.push(self.follow(pair[0], edge, &mut used));
            }
        }
        paths
    }

    fn follow(&self, start: usize, mut edge: usize, used: &mut [bool]) -> Vec<DVec2> {
        let mut current = start;
        let mut path = vec![self.vertices[start]];
        loop {
            used[edge] = true;
            let [left, right] = self.edges[edge];
            current = if left == current { right } else { left };
            path.push(self.vertices[current]);
            let adjacent = &self.adjacency[current];
            if current == start || adjacent.len() != 2 {
                break;
            }
            let Some(next) = adjacent.iter().copied().find(|&next| !used[next]) else {
                break;
            };
            edge = next;
        }
        path
    }
}
