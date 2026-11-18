use crate::conversation::{ChatState, ChatTool};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};

const CARD_GAP: f32 = 8.0;
const SUMMARY_CHARACTERS: usize = 120;
const DETAIL_MIN_ROWS: usize = 3;
const DETAIL_MAX_ROWS: usize = 12;

#[derive(Component, Clone, Reflect)]
#[reflect(Component)]
pub(crate) struct ToolCard {
    key: String,
    tool: ChatTool,
    expanded: bool,
}
#[derive(Component, Clone, Default)]
struct ToolSummary;
#[derive(Component, Clone, Default)]
struct ToolDetails;
#[derive(Component, Clone, Default)]
struct ToggleLabel;

pub(crate) fn install(app: &mut App) {
    app.register_type::<ToolCard>()
        .add_systems(PostUpdate, synchronize.before(UiSystems::Content));
}

pub(crate) fn card(key: String, tool: ChatTool, expanded: bool) -> impl Scene {
    let toggle_label = if expanded {
        "收起工具详情"
    } else {
        "查看工具详情"
    };
    let summary = summary(&tool);
    let details = details(&tool);
    let toggle_name = format!("ToolToggle-{key}");
    let detail_name = format!("ToolDetails-{key}");
    bsn! {
        template(move |_| Ok(ToolCard {key:key.clone(), tool:tool.clone(), expanded}))
        Node { width: percent(100), min_width: px(0), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(CARD_GAP), padding: UiRect::all(px(CARD_GAP)) }
        Children [
            Text(summary) WidgetryText ToolSummary--
            @WidgetryButton { @children: {Some(Box::new(bsn_list! {Text(toggle_label) WidgetryText ToggleLabel}) as Box<dyn SceneList>)} }
            Name(toggle_name) Node { align_self: AlignSelf::Start } on(toggle)--
            @WidgetryReadOnlyTextField {
                @value: details, @multiline: true,
                @auto_size: {Some(AutoSize {min_rows:DETAIL_MIN_ROWS, max_rows:DETAIL_MAX_ROWS})},
            }
            ToolDetails Name(detail_name)
            Node { display: {if expanded {Display::Flex} else {Display::None}}, width: percent(100), min_width: px(0), flex_shrink: 0.0 }
        ]
    }
}

fn status(value: &str) -> &str {
    match value {
        "unconfirmed" => "结果未确认",
        "receiving" => "接收中",
        "pending" => "待执行",
        "running" => "执行中",
        "succeeded" => "成功",
        "failed" => "失败",
        "timed_out" => "超时",
        "cancelled" => "取消",
        "not_executed" => "未执行",
        _ => value,
    }
}
fn preview(value: &str) -> String {
    let text: String = value.chars().take(SUMMARY_CHARACTERS).collect();
    if text.len() < value.len() {
        format!("{text}…")
    } else {
        text
    }
}
fn summary(tool: &ChatTool) -> String {
    let process = tool
        .output
        .as_deref()
        .and_then(|output| serde_json::from_str::<serde_json::Value>(output).ok());
    let process_state = process
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("state"))
        .and_then(|state| state.as_str());
    let label = match (process_state, tool.status.as_str()) {
        (Some("running"), "succeeded") => "调用成功 · 进程仍在运行",
        (Some("running"), _) => "调用失败 · 进程仍在运行",
        _ => status(&tool.status),
    };
    let result = if tool.name == "apply_patch" {
        process
            .as_ref()
            .and_then(|value| value.get("actions"))
            .and_then(|value| value.as_array())
            .map(|actions| {
                let completed = actions
                    .iter()
                    .filter(|action| action["status"] == "completed")
                    .count();
                let paths = actions
                    .iter()
                    .filter_map(|action| action["target"].as_str())
                    .collect::<Vec<_>>()
                    .join("；");
                format!(
                    "{completed}/{} 个文件动作已完成 · {}",
                    actions.len(),
                    preview(&paths)
                )
            })
    } else if tool.name == "view_image" {
        process
            .as_ref()
            .and_then(|value| value.get("data"))
            .filter(|data| data.get("sha256").is_some())
            .map(|data| {
                format!(
                    "{} · {} × {} → {} × {} · {} · SHA256 {}",
                    data["original_format"].as_str().unwrap_or(""),
                    data["original_width"],
                    data["original_height"],
                    data["width"],
                    data["height"],
                    preview(data["path"].as_str().unwrap_or("")),
                    data["sha256"].as_str().unwrap_or("")
                )
            })
    } else {
        None
    };
    format!(
        "工具 · {} · {} · 请求 {} / 输出 {} · {}\n参数：{}\n结果：{}",
        tool.source.as_deref().unwrap_or(&tool.name),
        label,
        tool.request_index,
        tool.output_index,
        if tool.retained {
            "已保留"
        } else {
            "尚未纳入上下文"
        },
        preview(&tool.arguments),
        result
            .or_else(|| tool.output.as_deref().map(preview))
            .unwrap_or_else(|| "尚无结果".into())
    )
}
fn details(tool: &ChatTool) -> String {
    format!(
        "输出项：{}\ncall_id：{}\n参数{}：\n{}\n结果：\n{}",
        tool.id,
        tool.call_id,
        if tool.arguments_truncated {
            "（超限，仅展示前缀）"
        } else {
            ""
        },
        tool.arguments,
        tool.output.as_deref().unwrap_or("尚无结果")
    )
}

fn toggle(
    event: On<Activate>,
    parents: Query<&ChildOf>,
    mut cards: Query<&mut ToolCard>,
    mut state: ResMut<ChatState>,
) -> Result {
    let root = parents.get(event.entity)?.parent();
    let mut card = cards.get_mut(root)?;
    card.expanded = !card.expanded;
    if card.expanded {
        if !state.expanded_tools.contains(&card.key) {
            state.expanded_tools.push(card.key.clone());
        }
    } else {
        state.expanded_tools.retain(|key| key != &card.key);
    }
    Ok(())
}

pub(crate) fn set_tool(world: &mut World, root: Entity, tool: ChatTool) -> Result {
    let mut card = world
        .get_mut::<ToolCard>(root)
        .ok_or_else(|| std::io::Error::other("工具行缺少状态"))?;
    if card.tool != tool {
        card.tool = tool;
    }
    Ok(())
}

fn synchronize(world: &mut World) -> Result {
    let mut cards = world.query_filtered::<(Entity, &ToolCard), Changed<ToolCard>>();
    let cards: Vec<_> = cards
        .iter(world)
        .map(|(root, card)| (root, card.clone()))
        .collect();
    for (root, card) in cards {
        let children = world
            .get::<Children>(root)
            .map(|children| children.to_vec())
            .unwrap_or_default();
        for child in children {
            if world.get::<ToolSummary>(child).is_some() {
                let mut text = world
                    .get_mut::<Text>(child)
                    .ok_or_else(|| std::io::Error::other("工具摘要缺少正文"))?;
                text.set_if_neq(Text(summary(&card.tool)));
            } else if world.get::<ToolDetails>(child).is_some() {
                world
                    .get_mut::<Node>(child)
                    .ok_or_else(|| std::io::Error::other("工具详情缺少布局"))?
                    .display = if card.expanded {
                    Display::Flex
                } else {
                    Display::None
                };
                if world.get::<WidgetryTextFieldHeadless>(child).is_some() {
                    let value = details(&card.tool);
                    if WidgetryTextFieldHeadless::text(world, child)
                        .map_err(|failure| failure.into_bevy_error())?
                        != value
                    {
                        WidgetryTextFieldHeadless::set_text_in_world(world, child, value)
                            .map_err(|failure| failure.into_bevy_error())?;
                    }
                }
            } else {
                let mut pending = vec![child];
                while let Some(label) = pending.pop() {
                    if world.get::<ToggleLabel>(label).is_some() {
                        world
                            .get_mut::<Text>(label)
                            .ok_or_else(|| std::io::Error::other("工具详情按钮缺少文字"))?
                            .set_if_neq(Text::new(if card.expanded {
                                "收起工具详情"
                            } else {
                                "查看工具详情"
                            }));
                    } else if let Some(children) = world.get::<Children>(label) {
                        pending.extend(children.iter());
                    }
                }
            }
        }
    }
    Ok(())
}
