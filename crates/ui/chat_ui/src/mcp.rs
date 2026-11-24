use agentelier_agent_host::{
    AgentHost, ConfigChange, ConnectionState, ConnectionStatus, HostFailed, McpConfigEvent,
    McpStatusEvent, ReceiveAgentEvents, Server, ServerId, ServerSettings, Transport,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::prelude::*;
use bevy::text::{FontSize, LineBreak};
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_widgetry::collapsible::{WidgetryCollapsible, WidgetryCollapsiblePlugin};
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::list_view::{
    WidgetryListModel, WidgetryListView, WidgetryListViewAppExt, WidgetryListViewRenderer,
};
use bevy_widgetry::message_box::{
    WidgetryMessageBoxButtons, WidgetryMessageBoxResult, WidgetryMessageBoxResultEvent,
    widgetry_message_box,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::segmented::{
    WidgetrySegmented, WidgetrySegmentedOption, WidgetrySegmentedPlugin,
};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::toggle_button::{
    WidgetryToggleButton, WidgetryToggleButtonChanged, WidgetryToggleButtonPlugin,
};
use bevy_widgetry::window::{
    WidgetryModalWindow, WidgetryWindowBackground, WidgetryWindowControlsConfig,
    WidgetryWindowTitle, owned_widgetry_window, widgetry_window_target,
};
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
    connections: Vec<ConnectionStatus>,
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

#[derive(Resource)]
struct ServerList(Entity);
#[derive(Component, Clone, Default)]
struct Manager;
#[derive(Component, Clone, Default)]
struct Content;
#[derive(Component, Clone, Default)]
struct ToolList;
#[derive(Component, Clone, Default)]
struct SelectedStatus;
#[derive(Component, Clone, Copy)]
struct PathPicker(Field);

#[derive(Clone, PartialEq, Eq)]
struct ServerRow {
    id: ServerId,
    name: String,
    status: String,
    overview: String,
}

impl ServerRow {
    fn content(&self) -> impl SceneList + use<> {
        let action = Action::Select(self.id);
        let name = self.name.clone();
        let overview = self.overview.clone();
        let entity_name = format!("SelectMcpServer-{}", self.id);
        bsn_list! {
            @AgentelierButton {
                @style: AgentelierButtonStyle::Secondary,
                @children: bsn_list! {
                    Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect::all(px(4)) }
                    Children [
                        Text(name) WidgetryText TextLayout::no_wrap()--
                        Text(overview) WidgetryText TextFont { font_size: FontSize::Px(16.0) } TextLayout::linebreak(LineBreak::AnyCharacter)
                    ]
                }
            }
            Name(entity_name) template(move |_| Ok(action)) on(activate)
            Node { width: percent(100), height: percent(100), min_width: px(0), overflow: Overflow::clip() }
        }
    }
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
    Materials(ServerId, agentelier_agent_host::MaterialKind),
    New,
    Save,
    Remove(ServerId),
    Pick(Field),
}

pub(crate) fn install(app: &mut App) -> Result {
    agentelier_controls::install_owned_window_close::<Or<(With<Manager>, With<PathPicker>)>>(app);
    if !app.is_plugin_added::<WidgetryCollapsiblePlugin>() {
        app.add_plugins(WidgetryCollapsiblePlugin);
    }
    if !app.is_plugin_added::<WidgetrySegmentedPlugin>() {
        app.add_plugins(WidgetrySegmentedPlugin);
    }
    if !app.is_plugin_added::<WidgetryToggleButtonPlugin>() {
        app.add_plugins(WidgetryToggleButtonPlugin);
    }
    app.register_widgetry_list_view::<ServerRow>()
        .map_err(|failure| failure.into_bevy_error())?;
    let source = app
        .world_mut()
        .spawn(WidgetryListModel::<ServerRow>::default())
        .id();
    crate::mcp_materials::install(app);
    app.insert_resource(ServerList(source))
        .init_resource::<Editor>()
        .init_resource::<McpConfigState>()
        .register_type::<McpConfigState>()
        .add_observer(closed)
        .add_systems(
            Update,
            (receive, receive_status, receive_failure)
                .chain()
                .after(ReceiveAgentEvents),
        );
    Ok(())
}

pub(crate) fn open(world: &mut World) -> Result {
    if world
        .query_filtered::<Entity, With<Manager>>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    if world.resource::<Editor>().pending.is_none()
        && let Err(failure) = request(world, ConfigChange::Read)
    {
        world.resource_mut::<McpConfigState>().status = failure.to_string();
    }
    let panel = panel(world);
    world.commands().spawn_scene_with_error_handler(bsn! {
        Manager Name("McpManagerWindow")
        @owned_widgetry_window(
            Window { title: "MCP 管理".into(), resolution: (1000, 730).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                Content Name("McpManagerContent")
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                Children [ @panel ]
            },
        )
    });
    Ok(())
}

pub(super) fn render(world: &mut World) -> Result {
    let Some(content) = world
        .query_filtered::<Entity, With<Content>>()
        .iter(world)
        .next()
    else {
        return Ok(());
    };
    let panel = panel(world);
    world.entity_mut(content).despawn_related::<Children>();
    world
        .commands()
        .spawn_scene_with_error_handler(panel)
        .insert(ChildOf(content));
    Ok(())
}

fn closed(_: On<Despawn<Manager>>, mut commands: Commands) {
    commands.queue(|world: &mut World| -> Result {
        crate::mcp_materials::close(world)?;
        let pickers = world
            .query_filtered::<Entity, With<PathPicker>>()
            .iter(world)
            .collect::<Vec<_>>();
        for picker in pickers {
            world.commands().entity(picker).try_despawn();
        }
        Ok(())
    });
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
            let mut state = world.resource_mut::<McpConfigState>();
            state.status = status;
            state.busy = false;
            update_catalog(world)?;
            if !crate::mcp_materials::opened(world) {
                render(world)?;
            }
            Ok(())
        });
    }
}

fn update_catalog(world: &mut World) -> Result {
    let editor = world.resource::<Editor>();
    let rows: Vec<_> = editor
        .servers
        .iter()
        .map(|server| {
            let state = editor
                .connections
                .iter()
                .find(|status| status.server == server.id);
            ServerRow {
                id: server.id,
                name: server.settings.name.clone(),
                status: label(&server.settings, state, true),
                overview: label(&server.settings, state, false),
            }
        })
        .collect();
    let details = tools_text(editor);
    let status = selected_status(editor);
    world.resource_mut::<McpConfigState>().servers = rows
        .iter()
        .map(|row| format!("{} · {}", row.name, row.status))
        .collect();
    let list = world.resource::<ServerList>().0;
    let mut model = world
        .get_mut::<WidgetryListModel<ServerRow>>(list)
        .ok_or_else(|| io::Error::other("MCP 列表未初始化"))?;
    if model.len() != rows.len()
        || rows
            .iter()
            .enumerate()
            .any(|(i, row)| model.get(i).is_none_or(|item| item.id != row.id))
    {
        model.clear();
        for row in rows {
            model
                .push(row)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    } else {
        for (index, row) in rows.into_iter().enumerate() {
            if model.get(index) != Some(&row)
                && let Some(item) = model
                    .get_mut(index)
                    .map_err(|failure| failure.into_bevy_error())?
            {
                *item = row;
            }
        }
    }
    for mut text in world
        .query_filtered::<&mut Text, With<ToolList>>()
        .iter_mut(world)
    {
        text.0.clone_from(&details);
    }
    for mut text in world
        .query_filtered::<&mut Text, With<SelectedStatus>>()
        .iter_mut(world)
    {
        text.0.clone_from(&status);
    }
    Ok(())
}

fn receive_status(mut events: MessageReader<McpStatusEvent>, mut commands: Commands) {
    let Some(event) = events.read().last() else {
        return;
    };
    let connections = event.0.clone();
    commands.queue(move |world: &mut World| -> Result {
        world.resource_mut::<Editor>().connections = connections;
        update_catalog(world)
    });
}

fn receive_failure(mut events: MessageReader<HostFailed>, mut commands: Commands) {
    if let Some(failure) = events.read().last() {
        let reason = failure.reason.clone();
        commands.queue(move |world: &mut World| -> Result {
            capture(world)?;
            let mut editor = world.resource_mut::<Editor>();
            editor.pending = None;
            editor.ready = false;
            let mut state = world.resource_mut::<McpConfigState>();
            state.busy = false;
            state.status = reason;
            render(world)
        });
    }
}

fn capture(world: &mut World) -> Result {
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
        if matches!(action, Action::Remove(_) | Action::Pick(_)) {
            return Ok(());
        }
        render(world)
    });
    Ok(())
}

fn apply(world: &mut World, action: Action) -> Result {
    let mut editor = world.resource_mut::<Editor>();
    match action {
        Action::Materials(id, kind) => crate::mcp_materials::open(world, id, kind)?,
        Action::Select(id) => {
            let server = editor
                .servers
                .iter()
                .find(|server| server.id == id)
                .ok_or_else(|| io::Error::other("MCP 服务器已移除"))?;
            editor.draft = Draft::from(server);
        }
        Action::New => editor.draft = Draft::default(),
        Action::Save => {
            let change = ConfigChange::Save(editor.draft.id, editor.draft.settings()?);
            request(world, change)?;
        }
        Action::Remove(id) => confirm_removal(world, id)?,
        Action::Pick(field) => pick_path(world, field)?,
    }
    Ok(())
}

fn label(
    settings: &ServerSettings,
    connection: Option<&ConnectionStatus>,
    detailed: bool,
) -> String {
    let transport = match settings.transport {
        Transport::Stdio { .. } => "stdio",
        Transport::StreamableHttp { .. } => "Streamable HTTP",
    };
    let status = match connection.map(|status| &status.state) {
        Some(ConnectionState::Starting) => "启动中".into(),
        Some(ConnectionState::Negotiating) => "协商中".into(),
        Some(ConnectionState::Ready(info)) => {
            let capabilities = [
                info.tools.then_some("Tools"),
                info.resources.then_some("Resources"),
                info.prompts.then_some("Prompts"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
            if detailed {
                format!(
                    "已连接 · {} · {}",
                    info.version,
                    if capabilities.is_empty() {
                        "无 Tools、Resources 或 Prompts 能力"
                    } else {
                        &capabilities
                    }
                )
            } else if capabilities.is_empty() {
                "已连接".into()
            } else {
                format!("已连接 · {capabilities}")
            }
        }
        Some(ConnectionState::Closing) => "关闭中".into(),
        Some(ConnectionState::Closed) => "已关闭".into(),
        Some(ConnectionState::Failed { stage, reason }) if detailed => {
            format!("{stage}失败：{reason}")
        }
        Some(ConnectionState::Failed { stage, .. }) => format!("{stage}失败"),
        None => "未连接".into(),
    };
    let tools = match connection.and_then(|status| status.tools) {
        Some(Ok(count)) => format!(" · {count} 项工具"),
        Some(Err(reason)) if detailed => format!(" · 工具发现失败：{reason}"),
        Some(Err(_)) => " · 工具发现失败".into(),
        None => String::new(),
    };
    let enabled = if settings.enabled {
        "已启用"
    } else {
        "已禁用"
    };
    format!("{transport} · 已保存 · {enabled}\n{status}{tools}")
}

fn tools_text(editor: &Editor) -> String {
    let Some(id) = editor.draft.id else {
        return String::new();
    };
    let Some(connection) = editor.connections.iter().find(|status| status.server == id) else {
        return "尚无已发现工具。".into();
    };
    match (&connection.state, connection.tools) {
        (ConnectionState::Ready(_), Some(Ok(_))) => {
            let mut names = connection.tool_names.clone();
            if names.is_empty() {
                names.push("服务器没有可用工具。".into());
            }
            names.extend(
                connection
                    .tool_issues
                    .iter()
                    .map(|issue| format!("无法注册 · {issue}")),
            );
            names.join("\n")
        }
        (ConnectionState::Ready(_), Some(Err(reason))) => reason.into(),
        (ConnectionState::Ready(capabilities), None) if capabilities.tools => {
            "正在发现工具…".into()
        }
        _ => "尚无已发现工具。".into(),
    }
}

fn selected_status(editor: &Editor) -> String {
    let Some(server) = editor
        .servers
        .iter()
        .find(|server| Some(server.id) == editor.draft.id)
    else {
        return String::new();
    };
    let connection = editor
        .connections
        .iter()
        .find(|status| status.server == server.id);
    label(&server.settings, connection, true)
}

fn manager_window(world: &mut World) -> Result<Entity> {
    let root = world
        .query_filtered::<Entity, With<Manager>>()
        .single(world)?;
    widgetry_window_target(world, root).ok_or_else(|| io::Error::other("MCP 窗口未绑定").into())
}

fn transport_picker(http: bool, busy: bool) -> impl Scene {
    (
        bsn! {
            @WidgetrySegmented
            Name("McpTransport") Node { align_self: AlignSelf::Start }
            on(transport_changed)
            Children [
                @WidgetrySegmentedOption Name("McpTransportStdio")
                @{(!http).then_some(bsn! { Checked })}
                Children [ Text("STDIO") WidgetryText ]--
                @WidgetrySegmentedOption Name("McpTransportHttp")
                @{http.then_some(bsn! { Checked })}
                Children [ Text("Streamable HTTP") WidgetryText ]
            ]
        },
        busy.then_some(bsn! { InteractionDisabled }),
    )
}

fn transport_changed(event: On<ValueChange<usize>>, mut commands: Commands) {
    let http = event.value == 1;
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<Editor>().pending.is_some()
            || world.resource::<Editor>().draft.http == http
        {
            return Ok(());
        }
        capture(world)?;
        world.resource_mut::<Editor>().draft.http = http;
        render(world)
    });
}

#[derive(Component, Clone, Copy)]
struct EnableServer(ServerId);

fn enable_button(id: ServerId, enabled: bool, busy: bool) -> impl Scene {
    let label = if enabled {
        "禁用服务器"
    } else {
        "启用服务器"
    };
    (
        bsn! {
            @WidgetryToggleButton { @selected: enabled }
            Name("ToggleMcpServer") template(move |_| Ok(EnableServer(id)))
            on(enabled_changed)
            Node { height: px(BUTTON_HEIGHT), padding: UiRect::horizontal(px(10)) }
            Children [ Text(label) WidgetryText Pickable::IGNORE ]
        },
        busy.then_some(bsn! { InteractionDisabled }),
    )
}

fn enabled_changed(
    event: On<WidgetryToggleButtonChanged>,
    servers: Query<&EnableServer>,
    mut commands: Commands,
) -> Result {
    let id = servers.get(event.entity)?.0;
    let enabled = event.selected;
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<Editor>().pending.is_some() {
            return Ok(());
        }
        capture(world)?;
        if let Err(failure) = request(world, ConfigChange::SetEnabled(id, enabled)) {
            world.resource_mut::<McpConfigState>().status = failure.to_string();
        }
        render(world)
    });
    Ok(())
}

#[derive(Component, Clone, Copy)]
struct RemoveServer(ServerId);

fn confirm_removal(world: &mut World, id: ServerId) -> Result {
    let server = world
        .resource::<Editor>()
        .servers
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| io::Error::other("MCP 服务器已移除"))?;
    let prompt = format!("移除「{}」的服务器配置并断开连接？", server.settings.name);
    let parent = manager_window(world)?;
    world.commands().spawn_scene_with_error_handler(bsn! {
        Name("ConfirmMcpRemoval")
        @widgetry_message_box(parent, "确认移除 MCP 服务器", WidgetryMessageBoxButtons::YesNo,
            Default::default(), bsn_list! { Text(prompt) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) })
        template(move |_| Ok(RemoveServer(id)))
        on(removal_confirmed)
    });
    Ok(())
}

fn removal_confirmed(
    event: On<WidgetryMessageBoxResultEvent>,
    removal: Query<&RemoveServer>,
    mut commands: Commands,
) -> Result {
    if event.result == WidgetryMessageBoxResult::Yes {
        let id = removal.get(event.entity)?.0;
        commands.queue(move |world: &mut World| -> Result {
            if let Err(failure) = request(world, ConfigChange::Remove(id)) {
                world.resource_mut::<McpConfigState>().status = failure.to_string();
            }
            render(world)
        });
    }
    Ok(())
}

fn pick_path(world: &mut World, field: Field) -> Result {
    if world
        .query_filtered::<Entity, With<PathPicker>>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let (mode, title) = match field {
        Field::Command => (WidgetryFileDialogMode::PickFile, "选择 MCP 可执行文件"),
        Field::Directory => (WidgetryFileDialogMode::PickDirectory, "选择 MCP 启动目录"),
        _ => return Err(io::Error::other("此 MCP 字段不接受文件路径").into()),
    };
    let parent = manager_window(world)?;
    world.commands().spawn_scene_with_error_handler(bsn! {
        Name("McpPathWindow")
        template(move |_| Ok(PathPicker(field)))
        template(move |_| Ok(WidgetryModalWindow { parent }))
        @owned_widgetry_window(
            Window { title: title.into(), resolution: (840, 580).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                @WidgetryFileDialog { @mode: mode }
                Name("McpPathPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(path_picked)
            },
        )
    });
    Ok(())
}

fn path_picked(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let (root, PathPicker(field)) = world.query::<(Entity, &PathPicker)>().single(world)?;
        let field = *field;
        if let Some(window) = widgetry_window_target(world, root) {
            world.write_message(bevy::window::WindowCloseRequested { window });
        }
        let mut editor = world.resource_mut::<Editor>();
        match (field, result) {
            (Field::Command, WidgetryFileDialogResult::File(path)) => {
                editor.draft.command = bevy_widgetry_path_display::display_path(&path);
            }
            (Field::Directory, WidgetryFileDialogResult::Directory(path)) => {
                editor.draft.directory = bevy_widgetry_path_display::display_path(&path);
            }
            _ => return Ok(()),
        }
        render(world)
    });
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
            @AgentelierButton {
                @style: {match action {
                    Action::Remove(_) => AgentelierButtonStyle::Danger,
                    Action::Save | Action::New => AgentelierButtonStyle::Primary,
                    _ => AgentelierButtonStyle::Secondary,
                }},
                @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)}
            }
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

fn panel(world: &World) -> impl Scene + use<> {
    let materials = crate::mcp_materials::opened(world);
    let right = if materials {
        crate::mcp_materials::panel(world)
    } else {
        editor_panel(world)
    };
    let list = (!materials).then(|| {
        server_panel(
            world.resource::<ServerList>().0,
            world.resource::<McpConfigState>().busy || !world.resource::<Editor>().ready,
        )
    });
    bsn! {
        Name("McpManagement")
        Node { width: percent(100), height: percent(100), min_height: px(0), column_gap: px(14), padding: UiRect::all(px(14)) }
        Children [
            @list--
            @WidgetryScrollArea {
                @children: bsn_list! { @right }
            }
            Name("McpEditorScroll") Node { flex_grow: 1.0, min_width: px(0), min_height: px(0), height: percent(100) }
        ]
    }
}

fn server_panel(source: Entity, busy: bool) -> impl Scene {
    bsn! {
        Node { width: px(290), min_width: px(160), flex_direction: FlexDirection::Column, min_height: px(0), row_gap: px(10) }
        Children [
            @button("新增服务器", "NewMcpServer", Action::New, busy)--
            @WidgetryListView::<ServerRow> {
                @source: source, @item_height: 96.0,
                @renderer: {WidgetryListViewRenderer::new(ServerRow::content)}
            }
            Name("McpServerList") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
        ]
    }
}

fn editor_panel(world: &World) -> Box<dyn Scene> {
    let editor = world.resource::<Editor>();
    let state = world.resource::<McpConfigState>();
    let busy = state.busy || !editor.ready;
    let status = state.status.clone();
    let draft = &editor.draft;
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
                @button("选择可执行文件", "PickMcpCommand", Action::Pick(Field::Command), busy) Node { align_self: AlignSelf::Start }--
                @field("参数（JSON 字符串数组，可留空）", &draft.args, Field::Args, "McpArgs")--
                @field("启动目录（绝对路径）", &draft.directory, Field::Directory, "McpDirectory")--
                @button("选择启动目录", "PickMcpDirectory", Action::Pick(Field::Directory), busy) Node { align_self: AlignSelf::Start }--
                @field("允许继承的环境变量（逗号分隔）", &draft.environment, Field::Environment, "McpEnvironment")
            ]
        })
    };
    let tools = selected.map(|_| tools_text(editor));
    let connection = selected_status(editor);
    let controls = selected.map(|id| {
        let enabled = draft.enabled;
        bsn! {
            Node { column_gap: px(ROW_GAP), row_gap: px(ROW_GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @enable_button(id, enabled, busy)--
                @button("移除服务器", "RemoveMcpServer", Action::Remove(id), busy)--
                @button("资源", "OpenMcpResources", Action::Materials(id, agentelier_agent_host::MaterialKind::Resources), busy)--
                @button("提示词", "OpenMcpPrompts", Action::Materials(id, agentelier_agent_host::MaterialKind::Prompts), busy)
            ]
        }
    });
    Box::new(bsn! {
        Name("McpServerSettings")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ROW_GAP), flex_shrink: 0.0 }
        Children [
            Text(status) WidgetryText Name("McpStatus") TextLayout::linebreak(LineBreak::AnyCharacter)--
            @controls--
            @{selected.map(|_| bsn! { SelectedStatus Name("McpConnectionStatus") Text(connection) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) })}--
            @{tools.map(|details| bsn! {
                @WidgetryCollapsible {
                    @header: bsn! { Text("Tools") WidgetryText },
                    @children: bsn_list! { ToolList Name("McpDiscoveredTools") Text(details) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) }
                } Name("McpTools") Node { width: percent(100) }
            })}--
            @field("服务器名称", &draft.name, Field::Name, "McpName")--
            @transport_picker(draft.http, busy)--
            @fields--
            @button("保存配置", "SaveMcpServer", Action::Save, busy)
        ]
    })
}
