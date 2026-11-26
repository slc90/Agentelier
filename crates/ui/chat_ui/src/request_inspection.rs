use crate::conversation::{ChatHistoryItem, ChatRequest, ChatState, ChatTurn};
use crate::project_context::ProjectSnapshot;
use agentelier_controls::AgentelierButton;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};

const GAP: f32 = 8.0;
const MIN_ROWS: usize = 3;
const MAX_ROWS: usize = 12;
const HTTP_SWITCHING_PROTOCOLS: u16 = 101;

#[derive(Clone, Reflect, PartialEq, Eq)]
pub struct ChatPublication {
    pub id: u64,
    pub window: u64,
    pub kind: String,
    pub snapshot: ProjectSnapshot,
    pub rendered: Option<String>,
}
#[derive(Component, Clone, Copy)]
enum Navigate {
    First,
    Previous,
    Next,
    Last,
}
#[derive(Component, Clone, Default)]
struct Metadata;
#[derive(Component, Clone, Copy)]
enum Body {
    Base,
    Behavior,
    Project,
    Order,
    Memory,
}
#[derive(Component, Clone, Default)]
struct Sources {
    rendered: Option<(agentelier_conversation_bridge::SessionId, u64)>,
}
#[derive(Component, Clone, Default)]
struct SourcesLabel;

pub(crate) fn install(app: &mut App) {
    app.add_systems(PostUpdate, synchronize.before(UiSystems::Content));
}
fn button(label: &'static str, name: &'static str, action: Navigate) -> impl Scene {
    bsn! {
        @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
        Name(name) action on(navigate)
    }
}
fn field(name: &'static str, body: Body) -> impl Scene {
    bsn! {
        @WidgetryReadOnlyTextField {
            @value: {String::new()}, @multiline: true,
            @auto_size: {Some(AutoSize { min_rows: MIN_ROWS, max_rows: MAX_ROWS })},
        }
        Name(name) body Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
    }
}
pub(crate) fn panel() -> impl Scene {
    bsn! {
        Name("RequestInspection")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(GAP), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0 }
            Children [
                @button("首次请求", "InspectFirstRequest", Navigate::First)--
                @button("上一次请求", "InspectPreviousRequest", Navigate::Previous)--
                @button("下一次请求", "InspectNextRequest", Navigate::Next)--
                @button("最近请求", "InspectLastRequest", Navigate::Last)
            ]--
            Text::default() WidgetryText Metadata Name("SelectedRequestMetadata") TextLayout::linebreak(LineBreak::AnyCharacter)--
            Text("固定指令 · developer · 按保存顺序") WidgetryText--
            @field("SelectedRequestBase", Body::Base)--
            Text("本次请求的情境指令") WidgetryText--
            @field("SelectedRequestBehavior", Body::Behavior)--
            Text("长期记忆") WidgetryText--
            @field("SelectedRequestMemory", Body::Memory)--
            Text("自动项目材料 · user · 最终包装正文") WidgetryText--
            @field("SelectedRequestProject", Body::Project)--
            @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text("展开所选请求来源") WidgetryText SourcesLabel }) as Box<dyn SceneList>)} }
            Name("ToggleSelectedRequestSources") Node { align_self: AlignSelf::Start } on(toggle)--
            Sources::default() Name("SelectedRequestSources")
            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }--
            Text("实际组装顺序 · 密文仅显示项类型") WidgetryText--
            @field("SelectedRequestOrder", Body::Order)
        ]
    }
}
fn requests(state: &ChatState) -> Vec<(&ChatTurn, &ChatRequest)> {
    state
        .session
        .as_ref()
        .map(|session| {
            session
                .previous_turns
                .iter()
                .chain(session.turn.iter())
                .flat_map(|turn| turn.requests.iter().map(move |request| (turn, request)))
                .collect()
        })
        .unwrap_or_default()
}
fn selected(state: &ChatState, list: &[(&ChatTurn, &ChatRequest)]) -> Option<usize> {
    state
        .selected_request
        .and_then(|key| {
            list.iter()
                .position(|(turn, request)| key == (turn.id, request.request))
        })
        .or_else(|| list.len().checked_sub(1))
}
fn navigate(event: On<Activate>, actions: Query<&Navigate>, mut state: ResMut<ChatState>) {
    let Ok(action) = actions.get(event.event_target()) else {
        return;
    };
    let list = requests(&state);
    let Some(current) = selected(&state, &list) else {
        return;
    };
    let index = match action {
        Navigate::First => 0,
        Navigate::Previous => current.saturating_sub(1),
        Navigate::Next => (current + 1).min(list.len() - 1),
        Navigate::Last => list.len() - 1,
    };
    let key = (list[index].0.id, list[index].1.request);
    state.selected_request = Some(key);
}
fn toggle(_: On<Activate>, mut state: ResMut<ChatState>) {
    state.inspection_sources_expanded = !state.inspection_sources_expanded;
}
fn order(request: &ChatRequest) -> String {
    let mut lines = Vec::new();
    if request.additional_tools_in_input {
        lines.push("developer · additional_tools · 会话工具目录".into());
    }
    lines.extend(
        request
            .base_hashes
            .iter()
            .map(|hash| format!("developer · 固定指令 · {hash}")),
    );
    for item in &request.items {
        let description = match item {
            ChatHistoryItem::Project { publication } => format!("user · 自动项目 P{publication}"),
            ChatHistoryItem::User => "user · 用户输入".into(),
            ChatHistoryItem::AgentResult => "Agent 协作结果".into(),
            ChatHistoryItem::Assistant { phase } => format!("assistant · {phase:?}"),
            ChatHistoryItem::FunctionCall { call_id, name } => {
                format!("function_call · {name} · {call_id}")
            }
            ChatHistoryItem::FunctionResult { call_id } => {
                format!("function_call_output · {call_id}")
            }
            ChatHistoryItem::Reasoning { encrypted } => {
                format!("reasoning · 含密文 {encrypted}（不展示内容）")
            }
            ChatHistoryItem::Compaction => "compaction（不展示密文）".into(),
        };
        lines.push(description);
    }
    let mut materials = request.skills.clone();
    if let Some(behavior) = &request.behavior {
        materials.push((
            behavior.input_index,
            format!("developer · 本次请求情境 · {}", behavior.hash),
        ));
    }
    if let Some(memory) = &request.memory {
        materials.push((
            memory.input_index,
            format!("{} · 长期记忆 · {}", memory.role, memory.publication_id),
        ));
    }
    if let Some((index, source)) = &request.transcript {
        materials.push((
            *index,
            format!(
                "developer · 文稿引用 · {} · SHA-256 {}",
                source.label(),
                source.text_sha256
            ),
        ));
    }
    materials.sort_by_key(|(index, _)| *index);
    for (index, text) in materials {
        lines.insert(index.min(lines.len()), text);
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(index, text)| format!("{}. {text}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}
fn synchronize(world: &mut World) -> Result {
    let mounted = world
        .query::<Ref<Sources>>()
        .iter(world)
        .any(|sources| sources.is_added());
    let state = world.resource_ref::<ChatState>();
    if !state.is_changed() && !mounted {
        return Ok(());
    }
    let expanded = state.inspection_sources_expanded;
    let list = requests(&state);
    let mut metadata = "尚无已准备的请求；采集状态见下方".to_owned();
    let (mut base, mut project, mut protocol) = (String::new(), String::new(), String::new());
    let mut behavior_body = String::new();
    let mut memory_body = String::new();
    let mut publication = None;
    if let Some(index) = selected(&state, &list) {
        let (turn, request) = list[index];
        let status = request
            .response_status
            .map(|status| match status {
                HTTP_SWITCHING_PROTOCOLS => "已通过 Codex WebSocket 发送请求".into(),
                _ => format!("已收到 HTTP 响应 {status}（不表示模型完成或遵守规则）"),
            })
            .unwrap_or_else(|| {
                if request.sending_started {
                    "已开始发送；尚无响应事实".into()
                } else {
                    "材料已准备；发送事实未记录".into()
                }
            });
        metadata = format!(
            "请求 {}/{} · Turn {} / 请求 {} · 窗口 {}\n{status}\n冻结载荷 {} bytes · SHA256 {}",
            index + 1,
            list.len(),
            turn.id,
            request.request,
            request.window,
            request.payload_bytes,
            request.payload_hash
        );
        base = if request.base_hashes.is_empty() {
            "本次请求未附加客户端固定指令".into()
        } else {
            turn.request_instructions.clone()
        };
        behavior_body = request.behavior.as_ref().map_or_else(
            || "本次请求未附加情境指令".into(),
            |behavior| {
                format!(
                    "developer · 输入 {} · SHA-256 {}\n\n{}",
                    behavior.input_index + 1,
                    behavior.hash,
                    behavior.text
                )
            },
        );
        memory_body = request.memory.as_ref().map_or_else(
            || "本次请求未注入长期记忆".into(),
            |memory| {
                format!(
                    "{} · 输入 {} · 发布 {}\n摘要 SHA-256 {}\n\n{}",
                    memory.role,
                    memory.input_index + 1,
                    memory.publication_id,
                    memory.hash,
                    memory.text
                )
            },
        );
        protocol = order(request);
        if let Some(session) = &state.session
            && let Some(found) = session
                .project_publications
                .iter()
                .find(|publication| Some(publication.id) == request.publication)
        {
            let reused = index
                .checked_sub(1)
                .is_some_and(|previous| list[previous].1.publication == request.publication);
            metadata.push_str(&format!("\n项目 P{} · {}{} · 发布窗口 {}\n会话目录：{}\n项目边界：{}\n来源 {} 份 · 原始字节 {} · 内容摘要 {}", found.id,
                if reused { "沿用；原发布类型：" } else { "" }, found.kind, found.window, found.snapshot.working_directory,
                found.snapshot.project_root.as_deref().unwrap_or("无 .git，仅会话目录"), found.snapshot.sources.len(), found.snapshot.source_bytes, found.snapshot.hash));
            project = found
                .rendered
                .clone()
                .unwrap_or_else(|| "成功为空；未生成自动项目协议项".into());
            publication = Some((session.id, found.clone()));
        }
    }
    for mut text in world
        .query_filtered::<&mut Text, With<Metadata>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(metadata.clone()));
    }
    for mut text in world
        .query_filtered::<&mut Text, With<SourcesLabel>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(
            if expanded {
                "收起所选请求来源"
            } else {
                "展开所选请求来源"
            }
            .into(),
        ));
    }
    let fields = world
        .query::<(Entity, &Body)>()
        .iter(world)
        .map(|(entity, body)| (entity, *body))
        .collect::<Vec<_>>();
    for (entity, body) in fields {
        let value = match body {
            Body::Base => &base,
            Body::Behavior => &behavior_body,
            Body::Project => &project,
            Body::Order => &protocol,
            Body::Memory => &memory_body,
        };
        if WidgetryTextFieldHeadless::text(world, entity)
            .map_err(|failure| failure.into_bevy_error())?
            != *value
        {
            WidgetryTextFieldHeadless::set_text_in_world(world, entity, value)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    let key = publication
        .as_ref()
        .map(|(session, publication)| (*session, publication.id));
    let roots = world
        .query::<(Entity, &Sources)>()
        .iter(world)
        .map(|(entity, sources)| (entity, sources.rendered != key))
        .collect::<Vec<_>>();
    for (root, changed) in roots {
        world
            .get_mut::<Node>(root)
            .ok_or_else(|| std::io::Error::other("请求来源缺少布局"))?
            .display = if expanded {
            Display::Flex
        } else {
            Display::None
        };
        if !changed {
            continue;
        }
        let children = world
            .get::<Children>(root)
            .map(|children| children.to_vec())
            .unwrap_or_default();
        for child in children {
            world.entity_mut(child).despawn();
        }
        if let Some((_, publication)) = &publication {
            for (index, source) in publication.snapshot.sources.iter().enumerate() {
                let scene =
                    crate::project_context::source_scene(source.clone(), index, "SelectedRequest");
                world
                    .commands()
                    .spawn_scene_with_error_handler(bsn! { ChildOf(root) @scene });
            }
        }
        world
            .get_mut::<Sources>(root)
            .ok_or_else(|| std::io::Error::other("请求来源缺少状态"))?
            .rendered = key;
    }
    Ok(())
}
