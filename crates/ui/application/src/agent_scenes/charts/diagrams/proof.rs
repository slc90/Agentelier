use super::{construction, latex_source, plot};
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use agentelier_markdown::MarkdownView;
use bevy::{prelude::*, text::Justify, ui::UiSystems};
use bevy_widgetry::{
    scene::spawn_scene,
    scroll_area::{ScrollAxis, WidgetryScrollArea},
    text::WidgetryText,
    theme::WidgetryTheme,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

const TEMPLATE: &str = "agentelier.proof_tree";
const GAP: f32 = 20.0;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    root: String,
    nodes: Vec<NodeSpec>,
    source_tex: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NodeSpec {
    id: String,
    conclusion: String,
    #[serde(default)]
    premises: Vec<String>,
    rule: Option<Rule>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum Rule {
    Text(String),
    Math { tex: String },
}

struct Inference {
    id: String,
    conclusion: String,
    premises: Vec<Inference>,
    rule: Option<Rule>,
    branch: f32,
    line: f32,
    note: f32,
}

#[derive(Clone, Component, Default)]
struct ProofFormula;

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Render a native logical proof tree with ordered premises, inference bars, rule labels and typeset conclusions. Requires an explicit derivation tree.",
        &[
            ("title", "Optional nonblank title."),
            ("root", "Required ID of the final conclusion."),
            ("nodes", "1 to 64 nodes {id,conclusion,premises?,rule?}; conclusion is TeX without delimiters. premises is an ordered list of child IDs. rule is optional plain text or {tex}. All nodes must form a single tree, depth at most 16. A node without premises is an assumption or axiom."),
            ("source_tex", "Optional original TeX, preserved as readable and copyable source alongside the tree."),
        ],
        build, None, None,
    )
}

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, center_formula_text.before(UiSystems::Content));
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let Properties {
        title,
        root,
        nodes,
        source_tex,
    } = props;
    let proof = validate(root, nodes).map_err(super::super::invalid)?;
    if source_tex
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || s.len() > 8192)
    {
        return Err(super::super::invalid("原 TeX 必须非空且最多 8192 字节"));
    }
    let root = plot::root(world, parent, "NativeProofTree", title.as_deref())?;
    let width = proof.branch + GAP;
    let tree = proof.scene();
    let title = title.map(plot::heading);
    let source = source_tex.as_deref().map(latex_source).map(|source| {
        bsn! {
            Name("ProofSource") @MarkdownView { @source: source }
        }
    });
    let contents = bsn_list! {
        Name("ProofTreeContent") Node { width: px(width), min_width: px(width), padding: UiRect::horizontal(px(GAP * 0.5)), flex_shrink: 0.0 }
        Children [ @tree ]
    };
    match spawn_scene(world, bsn! {
        ChildOf(root)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(8) }
        Children [
            @{title}--
            @WidgetryScrollArea {
                @axis: ScrollAxis::Horizontal,
                @content: {Some(Box::new(bsn! { on(agentelier_controls::forward_vertical_scroll) }) as Box<dyn Scene>)},
                @children: {Some(Box::new(contents) as Box<dyn SceneList>)},
            }
            Name("ProofTreeScroll") Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            @{source}
        ]
    }).map_err(construction) {
        Ok(_) => Ok(root),
        Err(error) => { world.despawn(root); Err(error) }
    }
}

fn validate(root: String, nodes: Vec<NodeSpec>) -> Result<Inference> {
    if !(1..=64).contains(&nodes.len()) {
        return Err(io::Error::other("证明树需要 1 至 64 个节点").into());
    }
    let mut by_id = BTreeMap::new();
    for node in nodes {
        if node.id.trim().is_empty()
            || node.id.len() > 64
            || node.conclusion.trim().is_empty()
            || node.conclusion.len() > 512
            || node.premises.len() > 12
        {
            return Err(
                io::Error::other("证明节点要求有效 ID、非空公式，以及最多 12 个前提").into(),
            );
        }
        if node.rule.as_ref().is_some_and(|rule| {
            let text = match rule {
                Rule::Text(label) => label,
                Rule::Math { tex } => tex,
            };
            text.trim().is_empty() || text.len() > 192
        }) {
            return Err(io::Error::other("推理规则标签须非空且最多 192 字节").into());
        }
        if by_id.insert(node.id.clone(), node).is_some() {
            return Err(io::Error::other("证明节点 ID 重复").into());
        }
    }
    let tree = take_inference(&root, &mut by_id, 1)?;
    if !by_id.is_empty() {
        return Err(io::Error::other("部分证明节点无法从最终结论到达").into());
    }
    Ok(tree)
}

fn take_inference(
    id: &str,
    left: &mut BTreeMap<String, NodeSpec>,
    depth: usize,
) -> Result<Inference> {
    if depth > 16 {
        return Err(io::Error::other("证明树的深度不能超过 16 层").into());
    }
    let node = left
        .remove(id)
        .ok_or_else(|| io::Error::other(format!("证明节点 {id} 不存在、重复引用或形成循环")))?;
    let premises = node
        .premises
        .iter()
        .map(|id| take_inference(id, left, depth + 1))
        .collect::<Result<Vec<_>>>()?;
    let span = premises.iter().map(|proof| proof.branch).sum::<f32>()
        + GAP * premises.len().saturating_sub(1) as f32;
    let line = span.max(formula_width(&node.conclusion));
    let note = node
        .rule
        .as_ref()
        .map(|rule| {
            let width = match rule {
                Rule::Text(label) => (label.chars().count() as f32 * 9.0 + 11.0).clamp(48.0, 180.0),
                Rule::Math { tex } => formula_width(tex),
            };
            width + 9.0
        })
        .unwrap_or(0.0);
    Ok(Inference {
        id: node.id,
        conclusion: node.conclusion,
        premises,
        rule: node.rule,
        branch: line + note,
        line,
        note,
    })
}

impl Inference {
    fn scene(self) -> Box<dyn Scene> {
        let Self {
            id,
            conclusion,
            premises,
            rule,
            branch,
            line,
            note,
        } = self;
        let inference = !premises.is_empty() || rule.is_some();
        let name = format!("ProofInference-{id}");
        let formula = formulate(conclusion, line);
        let premises: Vec<_> = premises.into_iter().map(Self::scene).collect();
        let above = (!premises.is_empty()).then(|| {
            let premises = bevy::scene::RelatedScenes::<Children>::new(premises);
            bsn! {
                Name("ProofPremises")
                Node { width: px(line), flex_direction: FlexDirection::Row, align_items: AlignItems::End, justify_content: JustifyContent::Center, column_gap: px(GAP) }
                @premises
            }
        });
        let rule = rule.map(|rule| match rule {
            Rule::Text(text) => Box::new(bsn! {
                Node { width: px(note - 9.0), max_width: px(note - 9.0) }
                Name("ProofRule") Text(text) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
            }) as Box<dyn Scene>,
            Rule::Math { tex } => Box::new(bsn! { Name("ProofRule") @formulate(tex, note - 9.0) }) as Box<dyn Scene>,
        });
        let bar = inference.then(|| bsn! {
            Node { width: px(branch), min_height: px(3), align_items: AlignItems::Center, column_gap: px(9), flex_shrink: 0.0 }
            Children [
                Name("ProofInferenceBar") Node { width: px(line), height: px(1.4), flex_shrink: 0.0 }
                template(|context| Ok(BackgroundColor(context.resource::<WidgetryTheme>().text.normal.foreground)))--
                @{rule}
            ]
        });
        Box::new(bsn! {
            Name(name) Node { width: px(branch), flex_direction: FlexDirection::Column, align_items: AlignItems::Start, row_gap: px(2), flex_shrink: 0.0 }
            Children [ @{above}-- @{bar}-- @formula ]
        })
    }
}

fn formula_width(tex: &str) -> f32 {
    (tex.chars().count() as f32 * 5.0 + 22.0).clamp(56.0, 440.0)
}

fn formulate(tex: String, width: f32) -> impl Scene {
    let source = format!("${tex}$");
    bsn! {
        Name("ProofFormula") ProofFormula Node { width: px(width), flex_shrink: 0.0 }
        Children [ @MarkdownView { @source: source, @source_controls: false } ]
    }
}

fn center_formula_text(
    formulas: Query<Entity, With<ProofFormula>>,
    children: Query<&Children>,
    mut text: Query<&mut TextLayout>,
) {
    for formula in &formulas {
        for child in children.iter_descendants(formula) {
            if let Ok(mut layout) = text.get_mut(child)
                && layout.justify != Justify::Center
            {
                layout.justify = Justify::Center;
            }
        }
    }
}
