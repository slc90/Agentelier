use agentelier_agent_host::{
    AgentHost, ConfigChange, McpConfigEvent, ReceiveAgentEvents, Server, ServerId, ServerSettings,
    Transport,
};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use std::{io, path::PathBuf};
use tracing::error;

const ROW_GAP: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 36.0;

#[derive(Default)]
struct Draft {
    id: Option<ServerId>,
    enabled: bool,
    http: bool,
    name: String,
    command: String,
    args: String,
    directory: String,
    environment: String,
    endpoint: String,
    bearer_environment: String,
}

impl From<&Server> for Draft {
    fn from(server: &Server) -> Self {
        let mut draft = Self {
            id: Some(server.id),
            name: server.settings.name.clone(),
            enabled: server.settings.enabled,
            ..default()
        };
        match &server.settings.transport {
            Transport::Stdio {
                command,
                args,
                working_directory,
                environment,
            } => {
                draft.command = bevy_widgetry_path_display::display_path(command);
                draft.directory = bevy_widgetry_path_display::display_path(working_directory);
                draft.args = serde_json::json!(args).to_string();
                draft.environment = environment.join(", ");
            }
            Transport::StreamableHttp {
                endpoint,
                bearer_environment,
            } => {
                draft.http = true;
                draft.endpoint = endpoint.clone();
                draft.bearer_environment = bearer_environment.clone().unwrap_or_default();
            }
        }
        draft
    }
}

impl Draft {
    fn settings(&self) -> io::Result<ServerSettings> {
        let transport = if self.http {
            let bearer_environment = self.bearer_environment.trim();
            Transport::StreamableHttp {
                endpoint: self.endpoint.trim().into(),
                bearer_environment: (!bearer_environment.is_empty())
                    .then(|| bearer_environment.into()),
            }
        } else {
            let args = if self.args.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&self.args)
                    .map_err(|_| io::Error::other("参数须为 JSON 字符串数组"))?
            };
            Transport::Stdio {
                command: PathBuf::from(self.command.trim()),
                args,
                working_directory: PathBuf::from(self.directory.trim()),
                environment: self
                    .environment
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }
        };
        Ok(ServerSettings {
            name: self.name.clone(),
            enabled: self.enabled,
            transport,
        })
    }
}

#[derive(Resource, Default)]
struct Editor {
    servers: Vec<Server>,
    draft: Draft,
    pending: Option<u64>,
    ready: bool,
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
struct McpConfigState {
    servers: Vec<String>,
    status: String,
    busy: bool,
}

#[derive(Component, Clone, Copy)]
enum Field {
    Name,
    Command,
    Args,
    Directory,
    Environment,
    Endpoint,
    BearerEnvironment,
}

#[derive(Component, Clone, Copy)]
enum Action {
    Select(ServerId),
    New,
    Transport,
    Save,
    SetEnabled(ServerId, bool),
    Remove(ServerId),
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Editor>()
        .init_resource::<McpConfigState>()
        .register_type::<McpConfigState>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

pub(crate) fn open(world: &mut World) {
    let editor = world.resource::<Editor>();
    if !editor.ready
        && editor.pending.is_none()
        && let Err(failure) = request(world, ConfigChange::Read)
    {
        world.resource_mut::<McpConfigState>().status = failure.to_string();
    }
}

fn request(world: &mut World, change: ConfigChange) -> Result {
    let id = world.resource::<AgentHost>().mcp_config(change)?;
    world.resource_mut::<Editor>().pending = Some(id);
    let mut state = world.resource_mut::<McpConfigState>();
    state.busy = true;
    state.status = "正在读取或保存服务器配置…".into();
    Ok(())
}

fn receive(mut events: MessageReader<McpConfigEvent>, mut commands: Commands) {
    for event in events.read() {
        let id = event.operation_id;
        let result = event
            .result
            .as_ref()
            .map(|reply| (reply.servers.clone(), reply.selection))
            .map_err(Clone::clone);
        commands.queue(move |world: &mut World| -> Result {
            let mut editor = world.resource_mut::<Editor>();
            if editor.pending != Some(id) {
                return Ok(());
            }
            editor.pending = None;
            let status = match result {
                Ok((servers, selection)) => {
                    editor.ready = true;
                    editor.draft = selection
                        .and_then(|id| servers.iter().find(|server| server.id == id))
                        .map(Draft::from)
                        .unwrap_or_default();
                    editor.servers = servers;
                    "服务器配置已更新".into()
                }
                Err(failure) => failure,
            };
            let servers = editor
                .servers
                .iter()
                .map(|server| label(&server.settings))
                .collect();
            *world.resource_mut::<McpConfigState>() = McpConfigState {
                servers,
                status,
                busy: false,
            };
            if crate::prompts::mcp_visible(world) {
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
    for (entity, field) in fields {
        let value =
            WidgetryTextFieldHeadless::text(world, entity).map_err(|e| e.into_bevy_error())?;
        let draft = &mut world.resource_mut::<Editor>().draft;
        match field {
            Field::Name => draft.name = value,
            Field::Command => draft.command = value,
            Field::Args => draft.args = value,
            Field::Directory => draft.directory = value,
            Field::Environment => draft.environment = value,
            Field::Endpoint => draft.endpoint = value,
            Field::BearerEnvironment => draft.bearer_environment = value,
        }
    }
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<Editor>().pending.is_some() {
            return Ok(());
        }
        capture(world)?;
        if let Err(failure) = apply(world, action) {
            error!(error = %failure, "MCP 配置操作失败");
            world.resource_mut::<McpConfigState>().status = failure.to_string();
        }
        crate::prompts::render(world)
    });
    Ok(())
}

fn apply(world: &mut World, action: Action) -> Result {
    let mut editor = world.resource_mut::<Editor>();
    match action {
        Action::Select(id) => {
            let server = editor
                .servers
                .iter()
                .find(|server| server.id == id)
                .ok_or_else(|| io::Error::other("MCP 服务器已移除"))?;
            editor.draft = Draft::from(server);
        }
        Action::New => editor.draft = Draft::default(),
        Action::Transport => editor.draft.http = !editor.draft.http,
        Action::Save => {
            let change = ConfigChange::Save(editor.draft.id, editor.draft.settings()?);
            request(world, change)?;
        }
        Action::SetEnabled(id, enabled) => request(world, ConfigChange::SetEnabled(id, enabled))?,
        Action::Remove(id) => request(world, ConfigChange::Remove(id))?,
    }
    Ok(())
}

fn label(settings: &ServerSettings) -> String {
    let transport = match settings.transport {
        Transport::Stdio { .. } => "stdio",
        Transport::StreamableHttp { .. } => "Streamable HTTP",
    };
    let state = if settings.enabled {
        "已启用，未连接"
    } else {
        "已禁用"
    };
    format!("{} · {transport} · {state}", settings.name)
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

fn field(label: &'static str, value: &str, kind: Field, name: &'static str) -> impl Scene {
    let value = value.to_owned();
    bsn! {
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(4), flex_shrink: 0.0 }
        Children [
            Text(label) WidgetryText--
            @WidgetryTextField { @value: value }
            Name(name) template(move |_| Ok(kind))
            Node { width: percent(100), flex_shrink: 0.0 }
        ]
    }
}

pub(crate) fn panel(world: &World) -> Box<dyn Scene> {
    let editor = world.resource::<Editor>();
    let state = world.resource::<McpConfigState>();
    let busy = state.busy || !editor.ready;
    let status = state.status.clone();
    let draft = &editor.draft;
    let transport_label = if draft.http {
        "传输：Streamable HTTP"
    } else {
        "传输：stdio"
    };
    let selected = draft.id;
    let fields: Box<dyn Scene> = if draft.http {
        Box::new(bsn! {
            Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ROW_GAP) }
            Children [
                @field("端点 URL", &draft.endpoint, Field::Endpoint, "McpEndpoint")--
                @field("Bearer 凭据环境变量名称（可选）", &draft.bearer_environment, Field::BearerEnvironment, "McpBearerEnvironment")
            ]
        })
    } else {
        Box::new(bsn! {
            Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ROW_GAP) }
            Children [
                @field("可执行文件绝对路径或 PATH 中的 .exe", &draft.command, Field::Command, "McpCommand")--
                @field("参数（JSON 字符串数组，可留空）", &draft.args, Field::Args, "McpArgs")--
                @field("启动目录（绝对路径）", &draft.directory, Field::Directory, "McpDirectory")--
                @field("允许继承的环境变量（逗号分隔）", &draft.environment, Field::Environment, "McpEnvironment")
            ]
        })
    };
    let rows = editor
        .servers
        .iter()
        .zip(&state.servers)
        .map(|(server, label)| {
            button(
                label.clone(),
                "SelectMcpServer",
                Action::Select(server.id),
                busy || Some(server.id) == selected,
            )
        })
        .collect::<Vec<_>>();
    let controls = selected.map(|id| {
        let enabled = draft.enabled;
        bsn! {
            Node { column_gap: px(ROW_GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button(if enabled { "禁用服务器" } else { "启用服务器" }, "ToggleMcpServer", Action::SetEnabled(id, !enabled), busy)--
                @button("移除服务器", "RemoveMcpServer", Action::Remove(id), busy)
            ]
        }
    });
    Box::new(bsn! {
        Name("McpServerSettings")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ROW_GAP), flex_shrink: 0.0 }
        Children [
            Text(status) WidgetryText Name("McpStatus") TextLayout::linebreak(LineBreak::AnyCharacter)--
            Node { column_gap: px(ROW_GAP), row_gap: px(ROW_GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button("新增服务器", "NewMcpServer", Action::New, busy)--
                {rows}
            ]--
            @controls--
            @field("服务器名称", &draft.name, Field::Name, "McpName")--
            @button(transport_label, "McpTransport", Action::Transport, busy)--
            @fields--
            @button("保存配置", "SaveMcpServer", Action::Save, busy)
        ]
    })
}
