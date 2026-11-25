use crate::view::MarkdownSource;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};
use std::io;
use tracing::{error, info};

const TOOLBAR_GAP: f32 = 8.0;
const SOURCE_MIN_ROWS: usize = 4;
const SOURCE_MAX_ROWS: usize = 12;

#[derive(Component, Clone, Default)]
struct RenderedBody;
#[derive(Component, Clone, Default)]
struct SourceEditor;
#[derive(Component, Clone, Default, Reflect)]
#[reflect(Component)]
struct SourceMode(bool);
#[derive(Component, Clone, Copy)]
enum SourceAction {
    Toggle,
    Copy,
}
#[derive(Component, Clone, Default)]
struct ToggleLabel;
#[derive(Component, Clone, Default)]
struct CopyLabel;

pub(crate) fn install(app: &mut App) {
    app.register_type::<SourceMode>();
}

pub(crate) fn scene(source: String) -> impl Scene {
    let value = source.clone();
    bsn! {
        MarkdownSource(source) SourceMode(false) Name("MarkdownView")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(TOOLBAR_GAP), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(TOOLBAR_GAP), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0 }
            Children [
                @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text("查看原文") WidgetryText ToggleLabel }) as Box<dyn SceneList>)} }
                template(|_| Ok(SourceAction::Toggle)) Name("MarkdownToggleSource") on(activate)--
                @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text("复制正文") WidgetryText CopyLabel }) as Box<dyn SceneList>)} }
                template(|_| Ok(SourceAction::Copy)) Name("MarkdownCopySource") on(activate)
            ]--
            RenderedBody Name("MarkdownRenderedBody")
            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(crate::view::BLOCK_GAP), flex_shrink: 0.0 }--
            @WidgetryReadOnlyTextField {
                @value: value, @multiline: true,
                @auto_size: {Some(AutoSize { min_rows: SOURCE_MIN_ROWS, max_rows: SOURCE_MAX_ROWS })},
            }
            SourceEditor Name("MarkdownSourceEditor") Node { display: Display::None, width: percent(100), min_width: px(0), flex_shrink: 0.0 }
        ]
    }
}

pub(crate) fn sync(world: &mut World, root: Entity, source: &str) -> Result<Option<Entity>> {
    if world.get::<SourceMode>(root).is_none() {
        return Ok(Some(root));
    }
    let Some(children) = world.get::<Children>(root) else {
        return Ok(None);
    };
    let body = children
        .iter()
        .find(|child| world.get::<RenderedBody>(*child).is_some());
    let editor = children
        .iter()
        .find(|child| world.get::<SourceEditor>(*child).is_some());
    let (Some(body), Some(editor)) = (body, editor) else {
        return Ok(None);
    };
    if world.get::<WidgetryTextFieldHeadless>(editor).is_none() {
        return Ok(None);
    }
    if WidgetryTextFieldHeadless::text(world, editor)
        .map_err(|failure| failure.into_bevy_error())?
        != source
    {
        WidgetryTextFieldHeadless::set_text_in_world(world, editor, source)
            .map_err(|failure| failure.into_bevy_error())?;
    }
    Ok(Some(body))
}

fn activate(event: On<Activate>, actions: Query<&SourceAction>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    let button = event.entity;
    commands.queue(move |world: &mut World| -> Result {
        let mut root = button;
        while world.get::<MarkdownSource>(root).is_none() {
            root = world
                .get::<ChildOf>(root)
                .ok_or_else(|| io::Error::other("原文操作缺少所属消息"))?
                .parent();
        }
        match action {
            SourceAction::Copy => {
                let source = world
                    .get::<MarkdownSource>(root)
                    .ok_or_else(|| io::Error::other("消息缺少原文"))?
                    .0
                    .clone();
                copy(world, button, &source, "正文")?;
            }
            SourceAction::Toggle => {
                let mut mode = world
                    .get_mut::<SourceMode>(root)
                    .ok_or_else(|| io::Error::other("原文视图缺少状态"))?;
                mode.0 = !mode.0;
                let showing_source = mode.0;
                let mut pending = vec![root];
                while let Some(entity) = pending.pop() {
                    if let Some(children) = world.get::<Children>(entity) {
                        pending.extend(children.iter());
                    }
                    let body = world.get::<RenderedBody>(entity).is_some();
                    let editor = world.get::<SourceEditor>(entity).is_some();
                    if (body || editor)
                        && let Some(mut node) = world.get_mut::<Node>(entity)
                    {
                        node.display = if (body && !showing_source) || (editor && showing_source) {
                            Display::Flex
                        } else {
                            Display::None
                        };
                    }
                    if world.get::<ToggleLabel>(entity).is_some()
                        && let Some(mut text) = world.get_mut::<Text>(entity)
                    {
                        text.0 = if showing_source {
                            "查看排版"
                        } else {
                            "查看原文"
                        }
                        .into();
                    }
                }
                info!(showing_source, "用户切换回答原文视图");
            }
        }
        Ok(())
    });
    Ok(())
}

pub(crate) fn copy_button(label: &'static str, name: &'static str) -> impl Scene {
    bsn! {
        @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText CopyLabel }) as Box<dyn SceneList>)} }
        Name(name)
    }
}

pub(crate) fn copy(world: &mut World, button: Entity, text: &str, kind: &'static str) -> Result {
    let result = world
        .get_resource_mut::<bevy::clipboard::Clipboard>()
        .ok_or_else(|| io::Error::other("剪贴板服务不存在"))?
        .set_text(text);
    let label = match result {
        Ok(()) => {
            info!(kind, bytes = text.len(), "用户复制回答内容");
            format!("已复制{kind}")
        }
        Err(reason) => {
            error!(kind, error = %reason, "复制回答内容失败");
            format!("复制失败：{reason}")
        }
    };
    let mut pending = vec![button];
    while let Some(entity) = pending.pop() {
        if let Some(children) = world.get::<Children>(entity) {
            pending.extend(children.iter());
        }
        if world.get::<CopyLabel>(entity).is_some()
            && let Some(mut text) = world.get_mut::<Text>(entity)
        {
            text.0.clone_from(&label);
        }
    }
    Ok(())
}
