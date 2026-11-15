use crate::parse::{self, Block, BlockKind, Run};
use bevy::prelude::*;
use bevy::text::{FontSource, FontStyle, FontWeight, LineBreak, Strikethrough, Underline};
use bevy::ui::UiSystems;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;

const BODY_SIZE: f32 = 18.0;
const BLOCK_GAP: f32 = 10.0;
const QUOTE_INSET: f32 = 14.0;
const HEADING_SIZES: [f32; 6] = [30.0, 26.0, 23.0, 20.0, 20.0, 20.0];
const LINK_GAP: f32 = 6.0;
const QUOTE_BORDER: f32 = 3.0;
const RULE_THICKNESS: f32 = 1.0;
const RULE_COLOR: Color = Color::srgb(0.5, 0.5, 0.5);
const QUOTE_COLOR: Color = Color::srgb(0.45, 0.5, 0.6);
#[derive(Component, Clone, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct MarkdownSource(pub String);
#[derive(SceneComponent, Default, Clone)]
#[scene(MarkdownProps)]
pub struct MarkdownView;
#[derive(Default)]
pub struct MarkdownProps {
    pub source: String,
}
impl MarkdownView {
    fn scene(props: MarkdownProps) -> impl Scene {
        let source = props.source;
        bsn! { MarkdownSource(source) Name("MarkdownView") Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(BLOCK_GAP), flex_shrink: 0.0 } }
    }
}
pub struct MarkdownPlugin;
impl Plugin for MarkdownPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MarkdownSource>()
            .add_systems(PostUpdate, render.before(UiSystems::Prepare));
        crate::links::install(app);
    }
}
#[derive(Component)]
struct RenderedBlock {
    index: usize,
    block: Block,
}
fn render(world: &mut World) -> Result {
    let mut changed = world.query_filtered::<(Entity, &MarkdownSource), Changed<MarkdownSource>>();
    let updates: Vec<_> = changed
        .iter(world)
        .map(|(entity, source)| (entity, source.0.clone()))
        .collect();
    for (root, source) in updates {
        let blocks = parse::parse(&source);
        let mut query = world.query::<(Entity, &RenderedBlock, &ChildOf)>();
        let existing: Vec<_> = query
            .iter(world)
            .filter(|(_, _, parent)| parent.parent() == root)
            .map(|(entity, block, _)| (entity, block.index, block.block.clone()))
            .collect();
        let mut ordered = Vec::new();
        for (index, block) in blocks.into_iter().enumerate() {
            if let Some((entity, _, _)) = existing
                .iter()
                .find(|(_, old_index, old)| *old_index == index && old == &block)
            {
                ordered.push(*entity);
                continue;
            }
            let scene = block_scene(&block);
            let entity = world
                .commands()
                .spawn_scene_with_error_handler(bsn! {
                    ChildOf(root) Name(format!("MarkdownBlock-{index}"))
                    template(move |_| Ok(RenderedBlock { index, block: block.clone() }))
                    @{scene}
                })
                .id();
            ordered.push(entity);
        }
        for (entity, _, _) in existing {
            if !ordered.contains(&entity) {
                world.entity_mut(entity).despawn();
            }
        }
        world.commands().entity(root).replace_children(&ordered);
    }
    Ok(())
}
fn block_scene(block: &Block) -> Box<dyn Scene> {
    if block.kind == BlockKind::Rule {
        return Box::new(
            bsn! { Node { width: percent(100), height: px(RULE_THICKNESS), flex_shrink: 0.0 } BackgroundColor(RULE_COLOR) },
        );
    }
    let size = match block.kind {
        BlockKind::Heading(level) => HEADING_SIZES[usize::from(level - 1)],
        _ => BODY_SIZE,
    };
    let heading = matches!(block.kind, BlockKind::Heading(_));
    let spans: Vec<Box<dyn Scene>> = block
        .runs
        .iter()
        .map(|run| span(run, size, heading))
        .collect();
    let links: Vec<Box<dyn Scene>> = block
        .links
        .iter()
        .map(|url| crate::links::scene(url))
        .collect();
    let inset = if block.quote { QUOTE_INSET } else { 0.0 };
    Box::new(bsn! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(LINK_GAP), flex_shrink: 0.0, padding: UiRect::left(px(inset)), border: UiRect::left(px(if block.quote { QUOTE_BORDER } else { 0.0 })) }
        BorderColor::all(QUOTE_COLOR)
        Children [
            Text("") WidgetryText TextFont { font_size: {bevy::text::FontSize::Px(size)} } TextLayout::linebreak(LineBreak::AnyCharacter)
            Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
            Children [{spans}]--
            {links}
        ]
    })
}
fn span(run: &Run, size: f32, heading: bool) -> Box<dyn Scene> {
    let font = TextFont {
        font_size: size.into(),
        font: if run.code {
            FontSource::monospace()
        } else {
            FontSource::default()
        },
        weight: if heading || run.bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if run.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        ..default()
    };
    let strike = run.strike;
    let underline = run.link;
    let text = run.text.clone();
    Box::new(bsn! { TextSpan(text) WidgetryText template(move |context| {
        if strike { context.entity.insert(Strikethrough); }
        if underline { context.entity.insert(Underline); }
        Ok(font.clone())
    }) })
}
