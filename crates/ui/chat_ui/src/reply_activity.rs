use crate::conversation::{ChatTool, ChatTurn};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy_widgetry::scroll_area::{
    WidgetryScrollArea, WidgetryScrollAreaContent, WidgetryScrollAreaViewport,
};
use bevy_widgetry::text::WidgetryText;
use std::cmp::Reverse;
use std::io;

const TOOL_LOG_MAX_HEIGHT: f32 = 180.0;
const TOOL_PANEL_PADDING: f32 = 8.0;
const TOOL_PANEL_GAP: f32 = 4.0;
const ARGUMENT_PREVIEW_BYTES: usize = 240;
const RESULT_PREVIEW_BYTES: usize = 420;

#[derive(Component, Clone, Default)]
pub(super) struct Activity;
#[derive(Component, Clone, Default)]
struct Summary;
#[derive(Component, Clone, Default)]
struct ToolLog;
#[derive(Component, Clone, Default)]
struct Entries;

pub(super) struct Presentation {
    pub(super) running: bool,
    pub(super) has_tools: bool,
    summary: String,
    entries: String,
}

impl Presentation {
    pub(super) fn new(turn: &ChatTurn, running: bool) -> Self {
        let mut tools = turn.tools.iter().collect::<Vec<_>>();
        tools.sort_by_key(|tool| {
            (
                tool.status != "running",
                Reverse((tool.request_index, tool.output_index)),
            )
        });
        let has_tools = !tools.is_empty();
        let mut summary = if has_tools {
            format!("工具调用 · {} 项", tools.len())
        } else {
            String::new()
        };
        if running {
            if has_tools {
                summary.push_str(" · ");
            }
            summary.push_str(match turn.phase.as_str() {
                "project_context" => "正在准备项目上下文",
                "compaction" => "正在压缩上下文",
                "model" => "等待模型响应",
                "tool" => "正在处理工具调用",
                "cleanup" => "正在结束任务",
                _ => "任务进行中",
            });
        }
        Self {
            running,
            has_tools,
            summary,
            entries: tools
                .into_iter()
                .map(entry)
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }
}

fn entry(tool: &ChatTool) -> String {
    let status = match tool.status.as_str() {
        "receiving" => "接收参数中",
        "pending" => "待执行",
        "running" => "执行中",
        "succeeded" => "成功",
        "failed" => "失败",
        "timed_out" => "超时",
        "cancelled" => "已取消",
        "not_executed" => "未执行",
        "unconfirmed" | "external_unconfirmed" => "结果未确认",
        status => status,
    };
    let mut text = format!("{} · {status}", tool.name);
    if !tool.arguments.trim().is_empty() {
        let arguments = excerpt(
            &tool.arguments,
            ARGUMENT_PREVIEW_BYTES,
            tool.arguments_truncated,
        );
        text.push_str(&format!("\n参数  {arguments}"));
    }
    if let Some(output) = tool
        .output
        .as_deref()
        .filter(|output| !output.trim().is_empty())
    {
        let result = excerpt(output, RESULT_PREVIEW_BYTES, false);
        text.push_str(&format!("\n结果  {result}"));
    }
    text
}

fn excerpt(value: &str, max_bytes: usize, truncated: bool) -> String {
    let value = value.trim();
    let end = value.floor_char_boundary(max_bytes);
    let mut preview = value[..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if truncated || end < value.len() {
        preview.push('…');
    }
    preview
}

pub(super) fn panel() -> impl Scene {
    bsn! {
        Activity Name("ChatReplyActivity")
        Node {
            display: Display::None, width: percent(100), min_width: px(0), flex_shrink: 0.0,
            flex_direction: FlexDirection::Column, row_gap: px(TOOL_PANEL_GAP)
        }
        Children [
            Text::default() WidgetryText Summary Name("ChatReplyActivitySummary")
            TextColor({Color::srgb_u8(35, 94, 71)})
            TextLayout::linebreak(LineBreak::AnyCharacter)--
            @WidgetryScrollArea {
                @children: {Some(Box::new(bsn_list! {
                    Text::default() WidgetryText Entries Name("ChatReplyToolEntries")
                    TextLayout::linebreak(LineBreak::AnyCharacter)
                    Node { width: percent(100), min_width: px(0), flex_shrink: 0.0, padding: UiRect::all(px(TOOL_PANEL_PADDING)) }
                }) as Box<dyn SceneList>)}
            }
            ToolLog Name("ChatReplyToolLog")
            BackgroundColor({Color::srgba_u8(35, 94, 71, 16)})
            Node {
                display: Display::None, width: percent(100), min_width: px(0),
                max_height: px(TOOL_LOG_MAX_HEIGHT), min_height: px(0), flex_shrink: 0.0,
                border_radius: BorderRadius::all(px(6))
            }
        ]
    }
}

pub(super) fn reconcile(world: &mut World, root: Entity, content: Presentation) -> Result {
    let panel = child::<Activity>(world, root)?;
    set_display(
        world,
        panel,
        if content.running || content.has_tools {
            Display::Flex
        } else {
            Display::None
        },
    )?;
    let summary = child::<Summary>(world, panel)?;
    let log = child::<ToolLog>(world, panel)?;
    let viewport = child::<WidgetryScrollAreaViewport>(world, log)?;
    let scroll_content = child::<WidgetryScrollAreaContent>(world, viewport)?;
    let entries = child::<Entries>(world, scroll_content)?;
    set_display(
        world,
        log,
        if content.has_tools {
            Display::Grid
        } else {
            Display::None
        },
    )?;
    for (entity, text) in [(summary, content.summary), (entries, content.entries)] {
        world
            .get_mut::<Text>(entity)
            .ok_or_else(|| io::Error::other("助手活动缺少文本"))?
            .set_if_neq(Text(text));
    }
    Ok(())
}

fn child<T: Component>(world: &World, parent: Entity) -> Result<Entity> {
    world
        .get::<Children>(parent)
        .and_then(|children| {
            children
                .iter()
                .find(|child| world.get::<T>(*child).is_some())
        })
        .ok_or_else(|| io::Error::other("助手活动区域缺少子实体").into())
}

fn set_display(world: &mut World, entity: Entity, display: Display) -> Result {
    world
        .get_mut::<Node>(entity)
        .ok_or_else(|| io::Error::other("助手活动布局不可用"))?
        .map_unchanged(|node| &mut node.display)
        .set_if_neq(display);
    Ok(())
}
