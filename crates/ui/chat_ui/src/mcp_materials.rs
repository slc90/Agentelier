use agentelier_agent_host::{
    AgentHost, MaterialKind, MaterialOrigin, MaterialPreview, MaterialReply, MaterialRequest,
    McpMaterialEvent, PromptChoice, ReceiveAgentEvents, ResourceChoice, ServerId,
    expand_resource_uri,
};
use bevy::{prelude::*, text::LineBreak, ui::InteractionDisabled, ui_widgets::Activate};
use bevy_widgetry::{
    button::WidgetryButton,
    text::WidgetryText,
    text_field::{WidgetryTextField, WidgetryTextFieldHeadless},
};
use std::{collections::BTreeMap, io};

const GAP: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 36.0;

#[derive(Resource, Default)]
struct Materials {
    server: Option<ServerId>,
    kind: MaterialKind,
    pending: Option<u64>,
    origin: Option<MaterialOrigin>,
    entries: Vec<ResourceChoice>,
    prompts: Vec<PromptChoice>,
    selected: Option<usize>,
    variables: String,
    uri: String,
    preview: Option<MaterialPreview>,
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
struct McpMaterialState {
    busy: bool,
    entry_count: usize,
    preview_ready: bool,
    status: String,
}

#[derive(Component, Clone, Copy)]
enum Field {
    Variables,
    Uri,
}

#[derive(Component, Clone, Copy)]
enum Action {
    Back,
    Refresh,
    Choose(usize),
    Expand,
    Read,
    Use,
    Cancel,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Materials>()
        .init_resource::<McpMaterialState>()
        .register_type::<McpMaterialState>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

pub(crate) fn opened(world: &World) -> bool {
    world.resource::<Materials>().server.is_some()
}

pub(crate) fn open(world: &mut World, server: ServerId, kind: MaterialKind) -> Result {
    *world.resource_mut::<Materials>() = Materials {
        server: Some(server),
        kind,
        ..default()
    };
    *world.resource_mut::<McpMaterialState>() = McpMaterialState::default();
    if let Err(failure) = submit(world, MaterialRequest::List { server, kind }) {
        world.resource_mut::<McpMaterialState>().status = failure.to_string();
    }
    Ok(())
}

pub(crate) fn close(world: &mut World) -> Result {
    if let Some(id) = world.resource::<Materials>().pending {
        world.resource::<AgentHost>().cancel_mcp_material(id)?;
    }
    *world.resource_mut::<Materials>() = Materials::default();
    *world.resource_mut::<McpMaterialState>() = McpMaterialState::default();
    Ok(())
}

fn submit(world: &mut World, request: MaterialRequest) -> Result {
    let id = world.resource::<AgentHost>().mcp_material(request)?;
    let mut panel = world.resource_mut::<Materials>();
    panel.pending = Some(id);
    panel.preview = None;
    let mut projection = world.resource_mut::<McpMaterialState>();
    projection.busy = true;
    projection.preview_ready = false;
    projection.status = "正在读取 MCP 服务器…".into();
    Ok(())
}

fn receive(mut events: MessageReader<McpMaterialEvent>, mut commands: Commands) {
    for event in events.read() {
        let id = event.operation_id;
        let result = event.result.clone();
        commands.queue(move |world: &mut World| -> Result {
            let mut panel = world.resource_mut::<Materials>();
            if panel.pending != Some(id) {
                return Ok(());
            }
            panel.pending = None;
            let status = match result {
                Ok(MaterialReply::Resources { origin, entries }) => {
                    panel.origin = Some(origin);
                    panel.entries = entries;
                    panel.selected = None;
                    panel.uri.clear();
                    panel.variables.clear();
                    if panel.entries.is_empty() {
                        "此服务器没有可选资源".into()
                    } else {
                        String::new()
                    }
                }
                Ok(MaterialReply::Prompts { origin, entries }) => {
                    panel.origin = Some(origin);
                    panel.prompts = entries;
                    panel.selected = None;
                    panel.variables.clear();
                    if panel.prompts.is_empty() {
                        "此服务器没有可选提示词".into()
                    } else {
                        String::new()
                    }
                }
                Ok(MaterialReply::Preview(preview)) => {
                    let status = if preview.usable {
                        "这是将加入草稿的实际内容"
                    } else {
                        "当前内容只能预览"
                    };
                    panel.preview = Some(preview);
                    status.into()
                }
                Err(failure) => failure,
            };
            *world.resource_mut::<McpMaterialState>() = McpMaterialState {
                busy: false,
                entry_count: panel.entries.len() + panel.prompts.len(),
                preview_ready: panel.preview.as_ref().is_some_and(|result| result.usable),
                status,
            };
            if crate::prompts::mcp_visible(world) && opened(world) {
                crate::prompts::render(world)?;
            }
            Ok(())
        });
    }
}

pub(crate) fn capture(world: &mut World) -> Result {
    let fields = world
        .query::<(Entity, &Field)>()
        .iter(world)
        .map(|(entity, field)| (entity, *field))
        .collect::<Vec<_>>();
    for (entity, kind) in fields {
        let value =
            WidgetryTextFieldHeadless::text(world, entity).map_err(|e| e.into_bevy_error())?;
        let mut state = world.resource_mut::<Materials>();
        match kind {
            Field::Variables => state.variables = value,
            Field::Uri => state.uri = value,
        }
    }
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        capture(world)?;
        if let Err(failure) = apply(world, action) {
            world.resource_mut::<McpMaterialState>().status = failure.to_string();
        }
        crate::prompts::render(world)
    });
    Ok(())
}

fn apply(world: &mut World, action: Action) -> Result {
    let mut state = world.resource_mut::<Materials>();
    if state.pending.is_some() && !matches!(action, Action::Back | Action::Cancel) {
        return Ok(());
    }
    match action {
        Action::Back => close(world)?,
        Action::Refresh => {
            let server = state
                .server
                .ok_or_else(|| io::Error::other("请选择 MCP 服务器"))?;
            let kind = state.kind;
            submit(world, MaterialRequest::List { server, kind })?;
        }
        Action::Choose(index) => {
            match state.kind {
                MaterialKind::Resources => {
                    let entry = state
                        .entries
                        .get(index)
                        .ok_or_else(|| io::Error::other("资源条目已变化"))?;
                    state.uri = if entry.template {
                        String::new()
                    } else {
                        entry.uri.clone()
                    };
                }
                MaterialKind::Prompts => {
                    if state.prompts.get(index).is_none() {
                        return Err(io::Error::other("提示词条目已变化").into());
                    }
                }
            }
            state.selected = Some(index);
            state.variables.clear();
            state.preview = None;
            let mut projection = world.resource_mut::<McpMaterialState>();
            projection.preview_ready = false;
            projection.status.clear();
        }
        Action::Expand => {
            let template = state
                .selected
                .and_then(|index| state.entries.get(index))
                .filter(|entry| entry.template)
                .ok_or_else(|| io::Error::other("请选择资源模板"))?;
            let values = arguments(&state.variables)?;
            state.uri = expand_resource_uri(&template.uri, &values).map_err(io::Error::other)?;
            state.preview = None;
            let mut projection = world.resource_mut::<McpMaterialState>();
            projection.preview_ready = false;
            projection.status = "已展开实际资源 URI".into();
        }
        Action::Read => {
            let origin = state
                .origin
                .clone()
                .ok_or_else(|| io::Error::other("请先刷新 MCP 清单"))?;
            let request = match state.kind {
                MaterialKind::Resources => MaterialRequest::ReadResource {
                    origin,
                    uri: state.uri.trim().to_owned(),
                },
                MaterialKind::Prompts => {
                    let prompt = state
                        .selected
                        .and_then(|index| state.prompts.get(index))
                        .ok_or_else(|| io::Error::other("请选择提示词"))?;
                    let arguments = arguments(&state.variables)?;
                    if let Some(missing) = prompt
                        .parameters
                        .iter()
                        .find(|arg| arg.required && !arguments.contains_key(&arg.name))
                    {
                        return Err(
                            io::Error::other(format!("缺少提示词参数 {}", missing.name)).into()
                        );
                    }
                    MaterialRequest::GetPrompt {
                        origin,
                        name: prompt.name.clone(),
                        arguments,
                    }
                }
            };
            submit(world, request)?;
        }
        Action::Use => {
            let preview = state
                .preview
                .as_ref()
                .filter(|preview| preview.usable)
                .ok_or_else(|| io::Error::other("没有可加入的文本材料"))?
                .text
                .clone();
            crate::conversation::append_mcp_draft(world, &preview)?;
            world.resource_mut::<Materials>().preview = None;
            let mut projection = world.resource_mut::<McpMaterialState>();
            projection.preview_ready = false;
            projection.status = "已加入聊天草稿".into();
        }
        Action::Cancel => {
            if let Some(id) = state.pending {
                world.resource::<AgentHost>().cancel_mcp_material(id)?;
            } else {
                state.preview = None;
                let mut projection = world.resource_mut::<McpMaterialState>();
                projection.preview_ready = false;
                projection.status = "已取消预览".into();
            }
        }
    }
    Ok(())
}

fn arguments(value: &str) -> io::Result<BTreeMap<String, String>> {
    if value.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(value).map_err(|_| io::Error::other("参数须为 JSON 字符串对象"))
}

fn button(
    label: impl Into<String>,
    name: &'static str,
    action: Action,
    disabled: bool,
) -> impl Scene {
    let label = label.into();
    (
        bsn! {
            @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
            Name(name) template(move |_| Ok(action)) on(activate)
            Node { height: px(BUTTON_HEIGHT), flex_shrink: 0.0 }
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn field(value: &str, hint: &'static str, name: &'static str, kind: Field) -> impl Scene {
    let value = value.to_owned();
    bsn! {
        @WidgetryTextField { @value: value, @hint_text: {Some(hint.into())} }
        Name(name) template(move |_| Ok(kind))
        Node { width: percent(100), flex_shrink: 0.0 }
    }
}

pub(crate) fn panel(world: &World) -> Box<dyn Scene> {
    let panel = world.resource::<Materials>();
    let projection = world.resource::<McpMaterialState>();
    let busy = projection.busy;
    let status = projection.status.clone();
    let can_append = projection.preview_ready
        && world
            .resource::<crate::conversation::ChatState>()
            .session
            .is_some();
    let can_cancel = busy || panel.preview.is_some();
    let (
        type_label,
        refresh_label,
        refresh_name,
        read_label,
        read_name,
        use_name,
        cancel_name,
        preview_name,
    ) = match panel.kind {
        MaterialKind::Resources => (
            "Resources",
            "刷新资源清单",
            "RefreshMcpResources",
            "读取预览",
            "ReadMcpResource",
            "UseMcpResource",
            "CancelMcpResource",
            "McpResourcePreview",
        ),
        MaterialKind::Prompts => (
            "Prompts",
            "刷新提示词清单",
            "RefreshMcpPrompts",
            "展开预览",
            "GetMcpPrompt",
            "UseMcpPrompt",
            "CancelMcpPrompt",
            "McpPromptPreview",
        ),
    };
    let title = panel.origin.as_ref().map_or_else(
        || format!("MCP {type_label}"),
        |origin| format!("{} · {type_label}", origin.name),
    );
    let choices = match panel.kind {
        MaterialKind::Resources => resource_choices(panel, busy),
        MaterialKind::Prompts => prompt_choices(panel, busy),
    };
    let preview = panel.preview.as_ref().map(|preview| preview.text.clone());
    let read_blocked = busy
        || panel.origin.is_none()
        || (panel.kind == MaterialKind::Prompts && panel.selected.is_none());
    Box::new(bsn! {
        Name("McpMaterialPanel")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Text(title) WidgetryText--
            Node { column_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button("返回服务器", "BackFromMcpMaterials", Action::Back, false)--
                @button(refresh_label, refresh_name, Action::Refresh, busy)
            ]--
            @{(!status.is_empty()).then(|| bsn! { Text(status) WidgetryText Name("McpMaterialStatus") TextLayout::linebreak(LineBreak::AnyCharacter) })}--
            @choices--
            Node { column_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button(read_label, read_name, Action::Read, read_blocked)--
                @button("加入聊天草稿", use_name, Action::Use, busy || !can_append)--
                @button("取消", cancel_name, Action::Cancel, !can_cancel)
            ]--
            @{preview.map(|text| bsn! {
                Text(text) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) Name(preview_name)
                Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
            })}
        ]
    })
}

fn resource_choices(panel: &Materials, busy: bool) -> Box<dyn Scene> {
    let entries = panel
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let label = if entry.template {
                format!("{} · 模板", entry.name)
            } else {
                entry.name.clone()
            };
            button(
                label,
                "ChooseMcpResource",
                Action::Choose(index),
                busy || panel.selected == Some(index),
            )
        })
        .collect::<Vec<_>>();
    let selected = panel.selected.and_then(|index| panel.entries.get(index));
    let selected_details = selected.map(|entry| {
        format!(
            "{}\n{}\n{}",
            entry.uri,
            entry.mime.as_deref().unwrap_or("MIME 未声明"),
            entry.description.as_deref().unwrap_or("")
        )
    });
    let template = selected.filter(|entry| entry.template).map(|_| {
        bsn! {
            Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
            Children [
                @field(&panel.variables, "模板参数（JSON 字符串对象）", "McpResourceVariables", Field::Variables)--
                @button("展开 URI", "ExpandMcpResource", Action::Expand, busy)
            ]
        }
    });
    Box::new(bsn! {
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
            Children [{entries}]--
            @{selected_details.map(|details| bsn! { Text(details) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) })}--
            @template--
            @field(&panel.uri, "资源 URI", "McpResourceUri", Field::Uri)
        ]
    })
}

fn prompt_choices(panel: &Materials, busy: bool) -> Box<dyn Scene> {
    let entries = panel
        .prompts
        .iter()
        .enumerate()
        .map(|(index, prompt)| {
            let label = prompt.title.clone().unwrap_or_else(|| prompt.name.clone());
            button(
                label,
                "ChooseMcpPrompt",
                Action::Choose(index),
                busy || panel.selected == Some(index),
            )
        })
        .collect::<Vec<_>>();
    let details = panel.selected.and_then(|index| panel.prompts.get(index)).map(|prompt| {
        let mut details = format!("{}\n{}", prompt.name, prompt.description.as_deref().unwrap_or_default());
        for param in &prompt.parameters {
            details.push_str(&format!("\n{}（{}） {}", param.name,
                if param.required { "必填" } else { "可选" },
                param.description.as_deref().unwrap_or_default()));
        }
        let parameters = !prompt.parameters.is_empty();
        bsn! {
            Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
            Children [
                Text(details) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                @{parameters.then(|| field(&panel.variables, "提示词参数（JSON 字符串对象）", "McpPromptArguments", Field::Variables))}
            ]
        }
    });
    Box::new(bsn! {
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
            Children [{entries}]--
            @details
        ]
    })
}
