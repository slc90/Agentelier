use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, io};

pub(super) const MIN: Vec2 = Vec2::new(-244.0, -90.0);
pub(super) const MAX: Vec2 = Vec2::new(244.0, 92.0);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Properties {
    title: Option<String>,
    #[serde(default)]
    kind: Kind,
    nodes: Vec<NodeSpec>,
    #[serde(default)]
    edges: Vec<EdgeSpec>,
    source_tex: Option<String>,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    #[default]
    Graph,
    Commutative,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NodeSpec {
    id: String,
    label: Option<String>,
    tex: Option<String>,
    at: Option<[f32; 2]>,
    #[serde(default)]
    draggable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<[f32; 2]>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EdgeSpec {
    from: String,
    to: String,
    directed: Option<bool>,
    label: Option<String>,
    tex: Option<String>,
}

pub(super) struct Node {
    id: String,
    pub caption: Caption,
    pub at: Vec2,
    pub half: Vec2,
    pub draggable: bool,
}

pub(super) struct Edge {
    pub from: usize,
    pub to: usize,
    pub directed: bool,
    pub caption: Caption,
}

pub(super) enum Caption {
    Plain(String),
    Tex(String),
    None,
}

pub(super) struct Model {
    pub title: Option<String>,
    pub source_tex: Option<String>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Properties {
    pub fn use_values(&mut self, values: &Map<String, Value>, overwrite: bool) -> Result {
        for node in &mut self.nodes {
            if node.draggable
                && (overwrite || node.current.is_none())
                && let Some(value) = values.get(&node.id).and_then(Value::as_str)
            {
                node.current = Some(serde_json::from_str(value)?);
            }
        }
        Ok(())
    }

    pub fn saved_values(&self) -> Map<String, Value> {
        encode(
            self.nodes
                .iter()
                .filter(|node| node.draggable)
                .filter_map(|node| node.current.map(|at| (node.id.as_str(), at))),
        )
    }
}

impl Model {
    pub fn new(props: Properties) -> Result<Self> {
        if !(1..=24).contains(&props.nodes.len()) || props.edges.len() > 64 {
            return Err(io::Error::other("图需要 1 至 24 个节点及最多 64 条边").into());
        }
        if props
            .source_tex
            .as_ref()
            .is_some_and(|source| source.trim().is_empty() || source.len() > 8192)
        {
            return Err(io::Error::other("原 TeX 必须非空且最多 8192 字节").into());
        }
        let mut ids = BTreeMap::new();
        let count = props.nodes.len();
        let nodes = props
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                if node.id.trim().is_empty()
                    || node.id.len() > 64
                    || ids.insert(node.id.as_str(), index).is_some()
                {
                    return Err(io::Error::other("图的节点 ID 必须唯一且非空，最多 64 字节").into());
                }
                if [node.at, node.current]
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|number| !(0.0..=1.0).contains(&number))
                {
                    return Err(io::Error::other("图的节点位置需在 [0,1] 范围内").into());
                }
                let at = node
                    .current
                    .or(node.at)
                    .map(Vec2::from_array)
                    .unwrap_or_else(|| position(props.kind, index, count));
                let caption = Caption::new(
                    node.label.as_deref().or(Some(&node.id)),
                    node.tex.as_deref(),
                )?;
                let half = Vec2::new((caption.width() * 0.5 + 10.0).clamp(20.0, 79.0), 17.0);
                Ok(Node {
                    id: node.id.clone(),
                    caption,
                    at: MIN + at * (MAX - MIN),
                    half,
                    draggable: node.draggable,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let resolve = |id: &str| {
            ids.get(id)
                .copied()
                .ok_or_else(|| io::Error::other(format!("图中没有节点 {id}")))
        };
        let edges = props
            .edges
            .iter()
            .map(|edge| {
                let directed = edge.directed.unwrap_or(props.kind == Kind::Commutative);
                if props.kind == Kind::Commutative && !directed {
                    return Err(io::Error::other("交换图的映射需要明确方向").into());
                }
                Ok(Edge {
                    from: resolve(&edge.from)?,
                    to: resolve(&edge.to)?,
                    directed,
                    caption: Caption::new(edge.label.as_deref(), edge.tex.as_deref())?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title: props.title,
            source_tex: props.source_tex,
            nodes,
            edges,
        })
    }

    pub fn interactive(&self) -> bool {
        self.nodes.iter().any(|node| node.draggable)
    }

    pub fn pick(&self, pointer: Vec2, tolerance: f32) -> Option<(usize, Vec2)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                node.draggable && (pointer - node.at).abs().cmple(node.half + tolerance).all()
            })
            .min_by(|(_, a), (_, b)| {
                a.at.distance_squared(pointer)
                    .total_cmp(&b.at.distance_squared(pointer))
            })
            .map(|(index, node)| (index, node.at))
    }

    pub fn move_node(&mut self, index: usize, at: Vec2) -> bool {
        let Some(node) = self
            .nodes
            .get_mut(index)
            .filter(|node| node.draggable && at.is_finite())
        else {
            return false;
        };
        let at = at.clamp(MIN, MAX);
        if node.at == at {
            return false;
        }
        node.at = at;
        true
    }

    pub fn saved_values(&self) -> Map<String, Value> {
        encode(self.nodes.iter().filter(|node| node.draggable).map(|node| {
            let at = ((node.at - MIN) / (MAX - MIN)).clamp(Vec2::ZERO, Vec2::ONE);
            (node.id.as_str(), at.to_array())
        }))
    }

    pub fn source(&self) -> String {
        let mut pieces = Vec::new();
        if self.interactive() {
            pieces.push("拖动描边节点可调整布局。".to_owned());
        }
        if let Some(original) = &self.source_tex {
            pieces.push(super::super::latex_source(original));
        }
        pieces.join("\n\n")
    }
}

impl Caption {
    fn new(label: Option<&str>, tex: Option<&str>) -> Result<Self> {
        if label.is_some_and(|name| name.is_empty() || name.len() > 96)
            || tex.is_some_and(|formula| formula.trim().is_empty() || formula.len() > 256)
        {
            return Err(
                io::Error::other("图标签须非空且最多 96 字节，数学标签最多 256 字节").into(),
            );
        }
        Ok(if let Some(tex) = tex {
            Self::Tex(tex.to_owned())
        } else if let Some(label) = label {
            Self::Plain(label.to_owned())
        } else {
            Self::None
        })
    }

    fn width(&self) -> f32 {
        match self {
            Self::Plain(label) => label
                .chars()
                .map(|c| if c.is_ascii() { 8.0 } else { 15.0 })
                .sum(),
            Self::Tex(tex) => (tex.len() as f32 * 4.5 + 11.0).min(140.0),
            Self::None => 0.0,
        }
    }
}

fn position(kind: Kind, index: usize, count: usize) -> Vec2 {
    if count == 1 {
        return Vec2::splat(0.5);
    }
    match kind {
        Kind::Graph => {
            Vec2::splat(0.5)
                + Vec2::from_angle(
                    index as f32 * std::f32::consts::TAU / count as f32
                        - std::f32::consts::FRAC_PI_2,
                ) * 0.4
        }
        Kind::Commutative => {
            let columns = (count as f32).sqrt().ceil() as usize;
            let rows = count.div_ceil(columns);
            Vec2::new(
                (index % columns) as f32 / (columns - 1) as f32,
                if rows > 1 {
                    (index / columns) as f32 / (rows - 1) as f32
                } else {
                    0.5
                },
            ) * 0.78
                + Vec2::splat(0.11)
        }
    }
}

fn encode<'a>(positions: impl Iterator<Item = (&'a str, [f32; 2])>) -> Map<String, Value> {
    positions
        .filter_map(|(id, position)| {
            serde_json::to_string(&position)
                .ok()
                .map(|point| (id.to_owned(), Value::String(point)))
        })
        .collect()
}
