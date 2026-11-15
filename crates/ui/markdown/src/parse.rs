use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}
#[derive(Clone, PartialEq)]
pub(crate) enum BlockKind {
    Prose,
    Heading(u8),
    Raw,
    Rule,
}
#[derive(Clone, PartialEq)]
pub(crate) struct Block {
    pub kind: BlockKind,
    pub quote: bool,
    pub runs: Vec<Run>,
    pub links: Vec<String>,
}
pub(crate) struct Tree {
    pub event: Event<'static>,
    pub range: Range<usize>,
    pub children: Vec<Tree>,
}
fn append(stack: &mut [Tree], roots: &mut Vec<Tree>, node: Tree) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else {
        roots.push(node);
    }
}
pub(crate) fn parse(source: &str) -> Vec<Block> {
    let mut roots = Vec::new();
    let mut stack: Vec<Tree> = Vec::new();
    for (event, range) in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH).into_offset_iter()
    {
        match event {
            Event::Start(tag) => stack.push(Tree {
                event: Event::Start(tag.into_static()),
                range,
                children: Vec::new(),
            }),
            Event::End(_) => {
                if let Some(mut node) = stack.pop() {
                    node.range.end = node.range.end.max(range.end);
                    append(&mut stack, &mut roots, node);
                }
            }
            event => append(
                &mut stack,
                &mut roots,
                Tree {
                    event: event.into_static(),
                    range,
                    children: Vec::new(),
                },
            ),
        }
    }
    let mut blocks = Vec::new();
    collect(&roots, source, false, &mut blocks);
    blocks
}
fn collect(nodes: &[Tree], source: &str, quote: bool, blocks: &mut Vec<Block>) {
    for node in nodes {
        match &node.event {
            Event::Start(Tag::BlockQuote(_)) => collect(&node.children, source, true, blocks),
            Event::Start(Tag::Paragraph | Tag::Heading { .. }) => {
                let kind = if let Event::Start(Tag::Heading { level, .. }) = node.event {
                    BlockKind::Heading(level as u8)
                } else {
                    BlockKind::Prose
                };
                let (runs, links) = inline(&node.children);
                blocks.push(Block {
                    kind,
                    quote,
                    runs,
                    links,
                });
            }
            Event::Rule => blocks.push(Block {
                kind: BlockKind::Rule,
                quote,
                runs: Vec::new(),
                links: Vec::new(),
            }),
            _ => blocks.push(Block {
                kind: BlockKind::Raw,
                quote,
                runs: vec![Run {
                    text: source
                        .get(node.range.clone())
                        .unwrap_or_default()
                        .to_owned(),
                    ..Default::default()
                }],
                links: Vec::new(),
            }),
        }
    }
}
pub(crate) fn inline(nodes: &[Tree]) -> (Vec<Run>, Vec<String>) {
    let mut runs = Vec::new();
    let mut links = Vec::new();
    inlines(nodes, Run::default(), &mut runs, &mut links);
    (runs, links)
}
fn inlines(nodes: &[Tree], style: Run, runs: &mut Vec<Run>, links: &mut Vec<String>) {
    for node in nodes {
        let mut next = style.clone();
        let text = match &node.event {
            Event::Start(tag) => {
                match tag {
                    Tag::Strong => next.bold = true,
                    Tag::Emphasis => next.italic = true,
                    Tag::Strikethrough => next.strike = true,
                    Tag::Link { dest_url, .. } => {
                        next.link = true;
                        if !links.iter().any(|url| url == dest_url.as_ref()) {
                            links.push(dest_url.to_string());
                        }
                    }
                    Tag::Image { dest_url, .. } => {
                        runs.push(Run {
                            text: "[图片：".into(),
                            ..style.clone()
                        });
                        inlines(&node.children, style.clone(), runs, links);
                        runs.push(Run {
                            text: format!(" · {dest_url}]"),
                            ..style.clone()
                        });
                        continue;
                    }
                    _ => {}
                }
                inlines(&node.children, next, runs, links);
                continue;
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => text.to_string(),
            Event::Code(text) => {
                next.code = true;
                text.to_string()
            }
            Event::SoftBreak | Event::HardBreak => "\n".into(),
            _ => String::new(),
        };
        if !text.is_empty() {
            next.text = text;
            runs.push(next);
        }
    }
}
