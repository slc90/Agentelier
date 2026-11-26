use agentelier_controls::forward_vertical_scroll;
use bevy::prelude::*;
use bevy::text::{FontSource, LineBreak, LineHeight};
use bevy::ui_widgets::Activate;
use bevy_widgetry::scroll_area::{ScrollAxis, WidgetryScrollArea};
use bevy_widgetry::text::{
    WidgetryText, WidgetryTextColorOverrides, WidgetryTextStateColorOverrides,
};

const CODE_SIZE: f32 = 16.0;
const CODE_LINE_HEIGHT: f32 = 24.0;
const SCROLLBAR_SPACE: f32 = 16.0;
const HEADER_GAP: f32 = 6.0;
const CODE_PADDING: f32 = 8.0;
const CODE_BACKGROUND: Color = Color::srgb_u8(0xE4, 0xF0, 0xE8);
const CODE_BORDER: Color = Color::srgb_u8(0xD0, 0xE4, 0xD6);
const CODE_FOREGROUND: Color = Color::srgb_u8(0x14, 0x53, 0x42);
#[derive(Component, Clone, Default)]
struct CodeText;
#[derive(Component, Clone, Default)]
struct CodeScroll;
#[derive(Component, Clone, Default)]
struct CodeBlock;

fn copy_code(event: On<Activate>, mut commands: Commands) {
    let button = event.entity;
    commands.queue(move |world: &mut World| -> Result {
        let mut root = button;
        while world.get::<CodeBlock>(root).is_none() {
            root = world
                .get::<ChildOf>(root)
                .ok_or_else(|| std::io::Error::other("代码复制缺少所属代码块"))?
                .parent();
        }
        let mut pending = vec![root];
        while let Some(entity) = pending.pop() {
            if let Some(children) = world.get::<Children>(entity) {
                pending.extend(children.iter());
            }
            if world.get::<CodeText>(entity).is_some() {
                let text = world
                    .get::<Text>(entity)
                    .ok_or_else(|| std::io::Error::other("代码复制缺少正文"))?
                    .0
                    .clone();
                return crate::source::copy(world, button, &text, "代码");
            }
        }
        Err(std::io::Error::other("代码块缺少正文").into())
    });
}

fn height(text: &str) -> f32 {
    text.split('\n').count() as f32 * CODE_LINE_HEIGHT + SCROLLBAR_SPACE
}

pub(crate) fn update(world: &mut World, root: Entity, text: &str) -> Result {
    let mut pending = vec![root];
    let mut found_text = false;
    let mut found_scroll = false;
    while let Some(entity) = pending.pop() {
        if let Some(children) = world.get::<Children>(entity) {
            pending.extend(children.iter());
        }
        if world.get::<CodeText>(entity).is_some() {
            world
                .get_mut::<Text>(entity)
                .ok_or_else(|| std::io::Error::other("代码块缺少正文"))?
                .set_if_neq(Text(text.to_owned()));
            found_text = true;
        }
        if world.get::<CodeScroll>(entity).is_some() {
            world
                .get_mut::<Node>(entity)
                .ok_or_else(|| std::io::Error::other("代码块缺少滚动布局"))?
                .height = px(height(text));
            found_scroll = true;
        }
    }
    if !found_text || !found_scroll {
        return Err(std::io::Error::other("代码块结构不完整").into());
    }
    Ok(())
}

pub(crate) fn scene(language: &str, text: &str) -> Box<dyn Scene> {
    let label = if language.is_empty() {
        "代码"
    } else {
        language
    }
    .to_owned();
    let height = height(text);
    let text = text.to_owned();
    let font = TextFont {
        font: FontSource::monospace(),
        font_size: CODE_SIZE.into(),
        ..default()
    };
    Box::new(bsn! {
        CodeBlock Name("MarkdownCodeBlock")
        BackgroundColor(CODE_BACKGROUND) BorderColor::all(CODE_BORDER)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(HEADER_GAP), flex_shrink: 0.0, padding: UiRect::all(px(CODE_PADDING)), border: UiRect::all(px(1)) }
        Children [
            Node { column_gap: px(HEADER_GAP), align_items: AlignItems::Center, flex_shrink: 0.0 }
            Children [Text(label) WidgetryText-- @crate::source::copy_button("复制代码", "MarkdownCopyCode") on(copy_code)]--
            @WidgetryScrollArea {
                @axis: ScrollAxis::Horizontal,
                @content: {Some(Box::new(bsn! { on(forward_vertical_scroll) }) as Box<dyn Scene>)},
                @children: {Some(Box::new(bsn_list! {
                    Text(text) CodeText Name("MarkdownCodeText")
                    @WidgetryText {
                        @colors: {WidgetryTextColorOverrides {
                            normal: WidgetryTextStateColorOverrides { foreground: Some(CODE_FOREGROUND) },
                            ..default()
                        }},
                    }
                    template(move |_| Ok(font.clone()))
                    LineHeight::Px(CODE_LINE_HEIGHT) TextLayout::linebreak(LineBreak::NoWrap)
                    Node { flex_shrink: 0.0 }
                }) as Box<dyn SceneList>)}
            }
            Name("MarkdownCodeScroll") CodeScroll Node { width: percent(100), min_width: px(0), height: px(height), flex_shrink: 0.0 }
        ]
    })
}
