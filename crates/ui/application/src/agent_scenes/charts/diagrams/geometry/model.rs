use super::super::super::axis::{AxisKind, Range};
use bevy::{math::DVec2, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Properties {
    pub title: Option<String>,
    bounds: [[f64; 2]; 2],
    points: Vec<PointDefinition>,
    #[serde(default)]
    objects: Vec<ObjectDefinition>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PointDefinition {
    id: String,
    label: Option<String>,
    formula: Option<String>,
    #[serde(default)]
    draggable: bool,
    position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<PointValue>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Position {
    Free {
        at: [f64; 2],
    },
    OnCircle {
        center: String,
        radius: f64,
        angle: f64,
    },
    OnSegment {
        from: String,
        to: String,
        t: f64,
    },
}

#[derive(Copy, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum PointValue {
    Free([f64; 2]),
    Angle(f64),
    Fraction(f64),
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ObjectDefinition {
    Segment {
        from: String,
        to: String,
        #[serde(default)]
        arrow: bool,
    },
    Circle {
        center: String,
        radius: f64,
    },
    Angle {
        a: String,
        vertex: String,
        b: String,
    },
    Polygon {
        points: Vec<String>,
        #[serde(default)]
        area: bool,
    },
    Decomposition {
        from: String,
        to: String,
    },
}

#[derive(Copy, Clone)]
enum Constraint {
    Free,
    Circle { center: usize, radius: f64 },
    Segment { from: usize, to: usize },
}

pub(super) enum Object {
    Segment(usize, usize, bool),
    Circle(usize, f64),
    Angle([usize; 3]),
    Polygon(Vec<usize>, bool),
    Decomposition(usize, usize),
}

pub(super) struct Point {
    id: String,
    pub label: String,
    formula: Option<String>,
    pub draggable: bool,
    constraint: Constraint,
    value: PointValue,
}

pub(super) struct Model {
    pub title: Option<String>,
    pub plane: Plane,
    pub points: Vec<Point>,
    pub locations: Vec<DVec2>,
    pub objects: Vec<Object>,
}

pub(super) struct Plane {
    bounds: [DVec2; 2],
    center: DVec2,
    pub scale: f64,
}

impl Properties {
    pub fn use_values(&mut self, values: &Map<String, Value>, overwrite: bool) -> Result {
        for point in &mut self.points {
            if !point.draggable || (!overwrite && point.current.is_some()) {
                continue;
            }
            if let Some(encoded) = values.get(&point.id).and_then(Value::as_str) {
                let candidate: PointValue = serde_json::from_str(encoded)?;
                if candidate.compatible(point.position.initial()) {
                    point.current = Some(candidate);
                }
            }
        }
        Ok(())
    }

    pub fn saved_values(&self) -> Map<String, Value> {
        saved(
            self.points
                .iter()
                .filter(|point| point.draggable)
                .filter_map(|point| point.current.map(|current| (point.id.as_str(), current))),
        )
    }
}

impl Position {
    fn initial(&self) -> PointValue {
        match *self {
            Self::Free { at } => PointValue::Free(at),
            Self::OnCircle { angle, .. } => PointValue::Angle(angle),
            Self::OnSegment { t, .. } => PointValue::Fraction(t),
        }
    }
}

impl PointValue {
    fn compatible(self, other: Self) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    fn valid(self) -> bool {
        match self {
            Self::Free(at) => at.into_iter().all(f64::is_finite),
            Self::Angle(angle) => angle.is_finite(),
            Self::Fraction(t) => (0.0..=1.0).contains(&t),
        }
    }
}

impl Plane {
    fn new(bounds: [[f64; 2]; 2]) -> Result<Self> {
        for range in bounds {
            Range::new(Some(range), [], AxisKind::Number)?;
        }
        let [[left, right], [bottom, top]] = bounds;
        let scale = (552.0 / (right - left)).min(202.0 / (top - bottom));
        if !scale.is_finite() || scale <= 0.0 {
            return Err(io::Error::other("无法缩放几何范围").into());
        }
        Ok(Self {
            bounds: [DVec2::new(left, bottom), DVec2::new(right, top)],
            center: DVec2::new(left.midpoint(right), bottom.midpoint(top)),
            scale,
        })
    }

    pub fn map(&self, at: DVec2) -> Vec2 {
        let xy = (at - self.center) * self.scale;
        Vec2::new(xy.x as f32, -xy.y as f32)
    }

    pub fn unmap(&self, pixel: Vec2) -> DVec2 {
        self.center + DVec2::new(f64::from(pixel.x), -f64::from(pixel.y)) / self.scale
    }
}

impl Model {
    pub fn new(props: Properties) -> Result<Self> {
        if !(1..=32).contains(&props.points.len()) || props.objects.len() > 64 {
            return Err(io::Error::other("几何示意需要 1 至 32 个点及最多 64 个对象").into());
        }
        let plane = Plane::new(props.bounds)?;
        let mut ids = BTreeMap::new();
        for (index, point) in props.points.iter().enumerate() {
            if point.id.trim().is_empty()
                || point.id.len() > 64
                || ids.insert(point.id.as_str(), index).is_some()
            {
                return Err(io::Error::other("几何点的 ID 必须唯一且非空，最多 64 字节").into());
            }
        }
        let resolve = |id: &str| {
            ids.get(id)
                .copied()
                .ok_or_else(|| io::Error::other(format!("找不到几何点 {id}")))
        };
        let points = props
            .points
            .iter()
            .map(|point| {
                let label = point.label.clone().unwrap_or_else(|| point.id.clone());
                if label.trim().is_empty()
                    || label.len() > 96
                    || point
                        .formula
                        .as_ref()
                        .is_some_and(|tex| tex.trim().is_empty() || tex.len() > 256)
                {
                    return Err(io::Error::other(
                        "几何点标签必须非空且至多 96 字节，原公式至多 256 字节",
                    )
                    .into());
                }
                let constraint = match &point.position {
                    Position::Free { .. } => Constraint::Free,
                    Position::OnCircle { center, radius, .. } => Constraint::Circle {
                        center: resolve(center)?,
                        radius: valid_radius(*radius, &plane)?,
                    },
                    Position::OnSegment { from, to, .. } => Constraint::Segment {
                        from: resolve(from)?,
                        to: resolve(to)?,
                    },
                };
                let initial = point.position.initial();
                let value = point.current.unwrap_or(initial);
                if !initial.valid() || !value.valid() || !initial.compatible(value) {
                    return Err(
                        io::Error::other(format!("点 {} 的坐标或约束参数无效", point.id)).into(),
                    );
                }
                Ok(Point {
                    id: point.id.clone(),
                    label,
                    formula: point.formula.clone(),
                    draggable: point.draggable,
                    constraint,
                    value,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut objects = Vec::new();
        for object in props.objects {
            objects.push(match object {
                ObjectDefinition::Segment { from, to, arrow } => {
                    Object::Segment(resolve(&from)?, resolve(&to)?, arrow)
                }
                ObjectDefinition::Circle { center, radius } => {
                    Object::Circle(resolve(&center)?, valid_radius(radius, &plane)?)
                }
                ObjectDefinition::Angle { a, vertex, b } => {
                    Object::Angle([resolve(&a)?, resolve(&vertex)?, resolve(&b)?])
                }
                ObjectDefinition::Polygon { points, area } => {
                    let indices = points
                        .iter()
                        .map(|point| resolve(point))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    let unique: BTreeSet<_> = indices.iter().copied().collect();
                    if !(3..=32).contains(&indices.len()) || unique.len() != indices.len() {
                        return Err(io::Error::other("多边形需要 3 至 32 个各不相同的顶点").into());
                    }
                    Object::Polygon(indices, area)
                }
                ObjectDefinition::Decomposition { from, to } => {
                    Object::Decomposition(resolve(&from)?, resolve(&to)?)
                }
            });
        }
        let locations = evaluate(&points, &plane)?;
        Ok(Self {
            title: props.title,
            plane,
            points,
            locations,
            objects,
        })
    }

    pub fn interactive(&self) -> bool {
        self.points.iter().any(|point| point.draggable)
    }

    pub fn pick(&self, pointer: Vec2, radius: f32) -> Option<(usize, Vec2)> {
        self.points
            .iter()
            .zip(&self.locations)
            .enumerate()
            .filter(|(_, (point, _))| point.draggable)
            .filter_map(|(index, (_, at))| {
                let at = self.plane.map(*at);
                let distance = at.distance(pointer);
                (distance <= radius).then_some((index, at, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(index, at, _)| (index, at))
    }

    pub fn move_point(&mut self, index: usize, destination: DVec2) -> Result<bool> {
        if !destination.is_finite() {
            return Ok(false);
        }
        let target = match self.points[index].constraint {
            Constraint::Free => PointValue::Free(
                destination
                    .clamp(self.plane.bounds[0], self.plane.bounds[1])
                    .to_array(),
            ),
            Constraint::Circle { center, .. } => {
                let direction = destination - self.locations[center];
                if direction == DVec2::ZERO {
                    return Ok(false);
                }
                PointValue::Angle(direction.y.atan2(direction.x).to_degrees())
            }
            Constraint::Segment { from, to } => {
                let a = self.locations[from];
                let d = self.locations[to] - a;
                let magnitude = d.abs().max_element();
                if magnitude == 0.0 {
                    return Ok(false);
                }
                let scaled = d / magnitude;
                let t = ((destination / magnitude - a / magnitude).dot(scaled)
                    / scaled.length_squared())
                .clamp(0.0, 1.0);
                if !t.is_finite() {
                    return Ok(false);
                }
                PointValue::Fraction(t)
            }
        };
        let old = std::mem::replace(&mut self.points[index].value, target);
        let next = match evaluate(&self.points, &self.plane) {
            Ok(value) => value,
            Err(error) => {
                self.points[index].value = old;
                return Err(error);
            }
        };
        if next == self.locations {
            self.points[index].value = old;
            return Ok(false);
        }
        self.locations = next;
        Ok(true)
    }

    pub fn saved_values(&self) -> Map<String, Value> {
        saved(
            self.points
                .iter()
                .filter(|point| point.draggable)
                .map(|point| (point.id.as_str(), point.value)),
        )
    }

    pub fn details(&self) -> String {
        let mut blocks: Vec<_> = self
            .points
            .iter()
            .filter_map(|point| point.formula.as_ref().map(|tex| format!("$${tex}$$")))
            .collect();
        if self.interactive() {
            blocks.push("拖动描边点可调整几何。".into());
        }
        blocks.join("\n\n")
    }
}

fn evaluate(points: &[Point], plane: &Plane) -> Result<Vec<DVec2>> {
    let mut places = vec![None; points.len()];
    let mut completed = 0;
    while completed < points.len() {
        let mut progress = false;
        for (index, point) in points.iter().enumerate() {
            if places[index].is_some() {
                continue;
            }
            let at = match (point.constraint, point.value) {
                (Constraint::Free, PointValue::Free(at)) => Some(DVec2::from_array(at)),
                (Constraint::Circle { center, radius }, PointValue::Angle(angle)) => places[center]
                    .map(|at| {
                        at + DVec2::from_angle(angle.rem_euclid(360.0).to_radians()) * radius
                    }),
                (Constraint::Segment { from, to }, PointValue::Fraction(t)) => places[from]
                    .zip(places[to])
                    .map(|(a, b)| a * (1.0 - t) + b * t),
                _ => return Err(io::Error::other("几何点的参数与约束不兼容").into()),
            };
            if let Some(at) = at {
                if !at.is_finite() || !plane.map(at).is_finite() {
                    return Err(
                        io::Error::other(format!("点 {} 超出可计算的几何范围", point.id)).into(),
                    );
                }
                places[index] = Some(at);
                completed += 1;
                progress = true;
            }
        }
        if !progress {
            return Err(io::Error::other("几何点约束存在循环依赖").into());
        }
    }
    places
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| io::Error::other("无法解析几何点约束").into())
}

fn valid_radius(radius: f64, plane: &Plane) -> Result<f64> {
    if radius.is_finite() && radius > 0.0 && (radius * plane.scale) <= f64::from(f32::MAX) {
        Ok(radius)
    } else {
        Err(io::Error::other("圆的半径必须为可显示的有限正数").into())
    }
}

fn saved<'a>(points: impl Iterator<Item = (&'a str, PointValue)>) -> Map<String, Value> {
    points
        .filter_map(|(id, value)| {
            serde_json::to_string(&value)
                .ok()
                .map(|encoded| (id.to_owned(), Value::String(encoded)))
        })
        .collect()
}
