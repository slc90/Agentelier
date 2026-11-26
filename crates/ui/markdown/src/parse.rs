use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::borrow::Cow;
use std::ops::Range;

const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_MATH);

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Run {
    pub content: RunContent,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) enum RunContent {
    Text(String),
    Math(Formula),
}

impl Default for RunContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl RunContent {
    pub(crate) fn source(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Math(formula) => &formula.original,
        }
    }
}
#[derive(Clone, PartialEq)]
pub(crate) enum BlockKind {
    Prose,
    Heading(u8),
    Raw,
    Rule,
    Code { language: String, text: String },
    DisplayMath(Formula),
    Table(Table),
}

#[derive(Clone, PartialEq)]
pub(crate) struct Formula {
    pub(crate) latex: String,
    pub(crate) original: String,
}
#[derive(Clone, PartialEq)]
pub(crate) struct Cell {
    pub runs: Vec<Run>,
    pub links: Vec<String>,
}
#[derive(Clone, PartialEq)]
pub(crate) struct Table {
    pub alignments: Vec<Alignment>,
    pub rows: Vec<Vec<Cell>>,
}
#[derive(Clone, PartialEq)]
pub(crate) struct ListPosition {
    pub depth: usize,
    pub marker: Option<String>,
}
#[derive(Clone, PartialEq)]
pub(crate) struct Block {
    pub kind: BlockKind,
    pub quote: bool,
    pub runs: Vec<Run>,
    pub links: Vec<String>,
    pub list: Option<ListPosition>,
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
    let input = bracket_math(source);
    let mut roots = Vec::new();
    let mut stack: Vec<Tree> = Vec::new();
    for (event, range) in Parser::new_ext(&input, OPTIONS).into_offset_iter() {
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
            Event::DisplayMath(latex)
                if source.get(range.clone()).is_some_and(|original| {
                    original.starts_with("\\(") && original.ends_with("\\)")
                }) =>
            {
                append(
                    &mut stack,
                    &mut roots,
                    Tree {
                        event: Event::InlineMath(latex.into_static()),
                        range,
                        children: Vec::new(),
                    },
                )
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

fn bracket_math(source: &str) -> Cow<'_, str> {
    let mut result = Cow::Borrowed(source);
    if !source.contains("\\[") && !source.contains("\\(") {
        return result;
    }
    let bytes = source.as_bytes();
    let mut opening = None;
    let mut substitutions = Vec::new();
    let mut literal = 0_usize;
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        match event {
            Event::Start(
                Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::Link { .. } | Tag::Image { .. },
            ) => {
                literal += 1;
                opening = None;
            }
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::Link | TagEnd::Image) => {
                literal = literal.saturating_sub(1);
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableCell,
            )
            | Event::Code(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Rule => opening = None,
            Event::Text(_) if literal == 0 => {
                let Some(start) = range.start.checked_sub(1) else {
                    continue;
                };
                if bytes.get(start) != Some(&b'\\') || escaped(bytes, start) {
                    continue;
                }
                match bytes.get(range.start).copied() {
                    Some(b'[') => opening = Some((start, b']')),
                    Some(b'(') => opening = Some((start, b')')),
                    Some(close @ (b']' | b')'))
                        if let Some((open, expected)) = opening
                            && close == expected =>
                    {
                        // Equal-length replacements keep every parser offset aligned with MarkdownSource.
                        opening = None;
                        let normalized = result.to_mut();
                        normalized.replace_range(open..open + 2, "$$");
                        normalized.replace_range(start..start + 2, "$$");
                        substitutions.push(open..start + 2);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Cow::Owned(normalized) = &mut result {
        let mut confirmed = Parser::new_ext(normalized, OPTIONS)
            .into_offset_iter()
            .filter_map(|(event, range)| matches!(event, Event::DisplayMath(_)).then_some(range))
            .peekable();
        let mut rejected = Vec::new();
        for range in substitutions {
            while confirmed
                .next_if(|candidate| candidate.start < range.start)
                .is_some()
            {}
            if confirmed.next_if(|candidate| *candidate == range).is_none() {
                rejected.push(range);
            }
        }
        drop(confirmed);
        for range in rejected {
            let open = range.start..range.start + 2;
            let close = range.end - 2..range.end;
            normalized.replace_range(open.clone(), &source[open]);
            normalized.replace_range(close.clone(), &source[close]);
        }
    }
    result
}

fn escaped(bytes: &[u8], start: usize) -> bool {
    bytes[..start]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        != 0
}

fn collect(nodes: &[Tree], source: &str, quote: bool, blocks: &mut Vec<Block>) {
    let mut index = 0;
    while let Some(node) = nodes.get(index) {
        if is_inline(&node.event) {
            let end = nodes[index..]
                .iter()
                .take_while(|node| is_inline(&node.event))
                .count()
                + index;
            collect_inline(&nodes[index..end], source, BlockKind::Prose, quote, blocks);
            index = end;
            continue;
        }
        index += 1;
        match &node.event {
            Event::Start(Tag::Table(alignments)) => {
                let rows = node
                    .children
                    .iter()
                    .map(|row| {
                        row.children
                            .iter()
                            .map(|cell| {
                                let (runs, links) = inline(&cell.children, source);
                                Cell { runs, links }
                            })
                            .collect()
                    })
                    .collect();
                blocks.push(Block {
                    kind: BlockKind::Table(Table {
                        alignments: alignments.clone(),
                        rows,
                    }),
                    quote,
                    runs: Vec::new(),
                    links: Vec::new(),
                    list: None,
                });
            }
            Event::Start(Tag::BlockQuote(_)) => collect(&node.children, source, true, blocks),
            Event::Start(Tag::List(start)) => {
                for (offset, item) in node.children.iter().enumerate() {
                    let mut item_blocks = Vec::new();
                    collect(&item.children, source, quote, &mut item_blocks);
                    let marker = start
                        .map_or_else(|| "•".into(), |start| format!("{}.", start + offset as u64));
                    for (index, mut block) in item_blocks.into_iter().enumerate() {
                        if let Some(list) = &mut block.list {
                            list.depth += 1;
                        } else {
                            block.list = Some(ListPosition {
                                depth: 0,
                                marker: (index == 0).then(|| marker.clone()),
                            });
                        }
                        blocks.push(block);
                    }
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(language) => language.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let text = node
                    .children
                    .iter()
                    .filter_map(|child| match &child.event {
                        Event::Text(text) => Some(text.as_ref()),
                        _ => None,
                    })
                    .collect();
                blocks.push(Block {
                    kind: BlockKind::Code { language, text },
                    quote,
                    runs: Vec::new(),
                    links: Vec::new(),
                    list: None,
                });
            }
            Event::Start(Tag::Paragraph | Tag::Heading { .. }) => {
                let kind = if let Event::Start(Tag::Heading { level, .. }) = node.event {
                    BlockKind::Heading(level as u8)
                } else {
                    BlockKind::Prose
                };
                collect_inline(&node.children, source, kind, quote, blocks);
            }
            Event::Rule => blocks.push(Block {
                kind: BlockKind::Rule,
                quote,
                runs: Vec::new(),
                links: Vec::new(),
                list: None,
            }),
            _ => blocks.push(Block {
                kind: BlockKind::Raw,
                quote,
                runs: vec![Run {
                    content: RunContent::Text(
                        source
                            .get(node.range.clone())
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                    ..Default::default()
                }],
                links: Vec::new(),
                list: None,
            }),
        }
    }
}

fn collect_inline(
    nodes: &[Tree],
    source: &str,
    kind: BlockKind,
    quote: bool,
    blocks: &mut Vec<Block>,
) {
    let mut start = 0;
    for (index, node) in nodes.iter().enumerate() {
        let Event::DisplayMath(latex) = &node.event else {
            continue;
        };
        collect_prose(&nodes[start..index], source, &kind, quote, blocks);
        let original = source
            .get(node.range.clone())
            .unwrap_or_default()
            .to_owned();
        blocks.push(Block {
            kind: BlockKind::DisplayMath(Formula {
                latex: latex.to_string(),
                original,
            }),
            quote,
            runs: Vec::new(),
            links: Vec::new(),
            list: None,
        });
        start = index + 1;
    }
    collect_prose(&nodes[start..], source, &kind, quote, blocks);
}

fn collect_prose(
    nodes: &[Tree],
    source: &str,
    kind: &BlockKind,
    quote: bool,
    blocks: &mut Vec<Block>,
) {
    let (runs, links) = inline(nodes, source);
    if !links.is_empty()
        || runs
            .iter()
            .any(|run| !run.content.source().trim().is_empty())
    {
        blocks.push(Block {
            kind: kind.clone(),
            quote,
            runs,
            links,
            list: None,
        });
    }
}

fn is_inline(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::TaskListMarker(_)
            | Event::Start(
                Tag::Strong
                    | Tag::Emphasis
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. }
            )
    )
}
fn inline(nodes: &[Tree], source: &str) -> (Vec<Run>, Vec<String>) {
    let mut runs = Vec::new();
    let mut links = Vec::new();
    inlines(nodes, source, Run::default(), &mut runs, &mut links);
    (runs, links)
}
fn inlines(nodes: &[Tree], source: &str, style: Run, runs: &mut Vec<Run>, links: &mut Vec<String>) {
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
                            content: RunContent::Text("[图片：".into()),
                            ..style.clone()
                        });
                        inlines(&node.children, source, style.clone(), runs, links);
                        runs.push(Run {
                            content: RunContent::Text(format!(" · {dest_url}]")),
                            ..style.clone()
                        });
                        continue;
                    }
                    _ => {}
                }
                inlines(&node.children, source, next, runs, links);
                continue;
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => text.to_string(),
            Event::InlineMath(latex) => {
                let original = source
                    .get(node.range.clone())
                    .unwrap_or_default()
                    .to_owned();
                let price = original.starts_with('$')
                    && source
                        .as_bytes()
                        .get(node.range.end)
                        .is_some_and(u8::is_ascii_digit);
                next.content = if price {
                    RunContent::Text(original)
                } else {
                    RunContent::Math(Formula {
                        latex: latex.to_string(),
                        original,
                    })
                };
                runs.push(next);
                continue;
            }
            Event::DisplayMath(_) => source
                .get(node.range.clone())
                .unwrap_or_default()
                .to_owned(),
            Event::Code(text) => {
                next.code = true;
                text.to_string()
            }
            Event::SoftBreak | Event::HardBreak => "\n".into(),
            Event::TaskListMarker(checked) => if *checked { "[x] " } else { "[ ] " }.into(),
            _ => String::new(),
        };
        if !text.is_empty() {
            next.content = RunContent::Text(text);
            runs.push(next);
        }
    }
}
