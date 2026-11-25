use crate::session_actions::{SidebarMenu, sidebar_menu};
use agentelier_agent_host::AgentHost;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::{
    Project, ProjectCatalog, ProjectChange, ProjectId, SessionId,
};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_app_assets::SidebarIcon;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::theme::WidgetryTheme;
use bevy_widgetry::window::{
    WidgetryWindowBackground, owned_widgetry_window, widgetry_window_target,
};
use std::io;

#[derive(Reflect)]
struct Save {
    id: u64,
    menu: Option<Entity>,
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub(crate) struct ProjectState {
    #[reflect(ignore)]
    catalog: ProjectCatalog,
    pending: Option<Save>,
    error: Option<String>,
}

#[derive(Component, Clone, Default)]
struct ProjectPickerWindow;
#[derive(Component, Clone, Default)]
struct ProjectStatus;
#[derive(Component, Clone, Default)]
struct ProjectName;
#[derive(Component, Clone, Copy)]
struct ProjectTarget(ProjectId);

#[derive(Component, Clone, Copy)]
enum Action {
    Rename,
    Delete,
    SaveName,
    ConfirmDelete,
    Cancel,
}

enum Menu {
    Actions,
    Rename(String),
    ConfirmDelete,
}

pub(crate) fn install(app: &mut App) {
    agentelier_controls::install_owned_window_close::<With<ProjectPickerWindow>>(app);
    app.init_resource::<ProjectState>()
        .register_type::<ProjectState>()
        .add_systems(PostUpdate, update.before(UiSystems::Content));
}

pub(crate) fn project(catalog: &ProjectCatalog, id: ProjectId) -> Option<&Project> {
    catalog
        .projects
        .iter()
        .find(|view| view.project.id == id)
        .map(|view| &view.project)
}

pub(crate) fn library(world: &World) -> &ProjectCatalog {
    &world.resource::<ProjectState>().catalog
}

impl ProjectState {
    pub(crate) fn project(&self, id: ProjectId) -> Option<&Project> {
        project(&self.catalog, id)
    }

    pub(crate) fn session_project(&self, session: SessionId) -> Option<&Project> {
        let id = self.catalog.membership.get(&session)?;
        self.project(*id)
    }
}

pub(crate) fn forget_session(world: &mut World, session: SessionId) {
    world
        .resource_mut::<ProjectState>()
        .catalog
        .membership
        .remove(&session);
}

pub(crate) fn status() -> impl Scene {
    bsn! {
        ProjectStatus Name("ProjectStatus") Text::default() WidgetryText
        TextLayout::linebreak(LineBreak::AnyCharacter) Node { display: Display::None }
    }
}

pub(crate) fn create_button() -> impl Scene {
    bsn! {
        @crate::archives::reveal_button(SidebarIcon::Add)
        Name("CreateProject") on(open_picker)
    }
}

pub(crate) fn more(project: ProjectId) -> impl Scene {
    let name = format!("ProjectMore-{project}");
    bsn! {
        @crate::archives::reveal_button(SidebarIcon::More)
        Name(name) template(move |_| Ok(ProjectTarget(project)))
        on(open_menu) on(crate::archives::block_row_click)
    }
}

fn open_menu(
    event: On<Activate>,
    targets: Query<&ProjectTarget>,
    menus: Query<(Entity, &ChildOf), With<SidebarMenu>>,
    state: Res<ProjectState>,
    theme: Res<WidgetryTheme>,
    mut commands: Commands,
) -> Result {
    if state.pending.is_some() {
        return Ok(());
    }
    let anchor = event.entity;
    let project = targets.get(anchor)?.0;
    for (menu, parent) in &menus {
        commands.entity(menu).try_despawn();
        if parent.parent() == anchor {
            return Ok(());
        }
    }
    commands.spawn_scene_with_error_handler(menu(anchor, project, Menu::Actions, &theme));
    Ok(())
}

fn menu(
    anchor: Entity,
    project: ProjectId,
    view: Menu,
    theme: &WidgetryTheme,
) -> impl Scene + use<> {
    let (width, items): (f32, Box<dyn SceneList>) = match view {
        Menu::Actions => (
            156.0,
            Box::new(bsn_list! {
                @button("重命名项目", "RenameProject", Action::Rename)--
                @button("删除项目", "DeleteProject", Action::Delete)
            }),
        ),
        Menu::Rename(name) => (
            244.0,
            Box::new(bsn_list! {
                @WidgetryTextField { @value: name, @multiline: false }
                ProjectName Name("ProjectNameInput") Node { width: percent(100) }--
                Node { width: percent(100), column_gap: px(4) }
                Children [
                    @button("保存名称", "SaveProjectName", Action::SaveName)
                    Node { flex_basis: px(0), flex_grow: 1.0, min_width: px(0) }--
                    @button("取消", "CancelProjectRename", Action::Cancel)
                    Node { flex_basis: px(0), flex_grow: 1.0, min_width: px(0) }
                ]
            }),
        ),
        Menu::ConfirmDelete => (
            244.0,
            Box::new(bsn_list! {
                Text("项目会话将移到单独聊天。文件和历史会保留。") WidgetryText
                TextLayout::linebreak(LineBreak::AnyCharacter)--
                @button("确认删除", "ConfirmDeleteProject", Action::ConfirmDelete)--
                @button("取消", "CancelDeleteProject", Action::Cancel)
            }),
        ),
    };
    bsn! {
        @sidebar_menu(anchor, theme, width, items)
        Name("ProjectActionsMenu") template(move |_| Ok(ProjectTarget(project)))
    }
}

fn button(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    let style = match action {
        Action::SaveName => AgentelierButtonStyle::Primary,
        Action::Delete | Action::ConfirmDelete => AgentelierButtonStyle::FilledDanger,
        Action::Rename | Action::Cancel => AgentelierButtonStyle::Secondary,
    };
    bsn! {
        @AgentelierButton {
            @style: style,
            @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)},
        }
        Name(name) template(move |_| Ok(action)) on(activate)
    }
}

fn activate(
    event: On<Activate>,
    actions: Query<&Action>,
    parents: Query<&ChildOf>,
    menus: Query<(Entity, &ChildOf, &ProjectTarget), With<SidebarMenu>>,
    mut commands: Commands,
) -> Result {
    let action = *actions.get(event.entity)?;
    let Some((entity, parent, &ProjectTarget(id))) = parents
        .iter_ancestors(event.entity)
        .find_map(|ancestor| menus.get(ancestor).ok())
    else {
        return Ok(());
    };
    let anchor = parent.parent();
    commands.queue(move |world: &mut World| -> Result {
        let state = world.resource::<ProjectState>();
        if state.pending.is_some() {
            return Ok(());
        }
        let Some(project) = project(&state.catalog, id) else {
            world.commands().entity(entity).try_despawn();
            return Ok(());
        };
        let view = match action {
            Action::Rename => Menu::Rename(project.name.clone()),
            Action::Delete => Menu::ConfirmDelete,
            Action::SaveName => {
                let field = world
                    .query_filtered::<Entity, With<ProjectName>>()
                    .single(world)?;
                let name = WidgetryTextFieldHeadless::text(world, field)
                    .map_err(|failure| failure.into_bevy_error())?;
                submit(
                    world,
                    ProjectChange::Rename { project: id, name },
                    Some(entity),
                );
                return Ok(());
            }
            Action::ConfirmDelete => {
                submit(world, ProjectChange::Delete { project: id }, Some(entity));
                return Ok(());
            }
            Action::Cancel => {
                world.commands().entity(entity).try_despawn();
                world.resource_mut::<ProjectState>().error = None;
                return Ok(());
            }
        };
        let scene = menu(anchor, id, view, world.resource::<WidgetryTheme>());
        world.commands().entity(entity).try_despawn();
        world.commands().spawn_scene_with_error_handler(scene);
        Ok(())
    });
    Ok(())
}

fn open_picker(
    _: On<Activate>,
    state: Res<ProjectState>,
    pickers: Query<(), With<ProjectPickerWindow>>,
    mut commands: Commands,
) {
    if state.pending.is_some() || !pickers.is_empty() {
        return;
    }
    commands.spawn_scene_with_error_handler(bsn! {
        ProjectPickerWindow Name("ProjectPickerWindow")
        @owned_widgetry_window(
            Window { title: "选择项目目录".into(), resolution: (840, 580).into(), ..default() },
            agentelier_controls::FILE_DIALOG_WINDOW_CONTROLS,
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
                Name("ProjectDirectoryPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(chosen)
            },
        )
    });
}

fn chosen(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let root = world
            .query_filtered::<Entity, With<ProjectPickerWindow>>()
            .single(world)?;
        let window = widgetry_window_target(world, root)
            .ok_or_else(|| io::Error::other("项目目录选择窗口未绑定"))?;
        world.write_message(bevy::window::WindowCloseRequested { window });
        if let WidgetryFileDialogResult::Directory(directory) = result {
            submit(
                world,
                ProjectChange::Create {
                    directory,
                    name: None,
                },
                None,
            );
        }
        Ok(())
    });
}

fn submit(world: &mut World, change: ProjectChange, menu: Option<Entity>) {
    if world.resource::<ProjectState>().pending.is_some() {
        return;
    }
    let result = world.resource::<AgentHost>().change_project(change);
    let mut state = world.resource_mut::<ProjectState>();
    match result {
        Ok(id) => {
            state.pending = Some(Save { id, menu });
            state.error = None;
        }
        Err(failure) => state.error = Some(failure.to_string()),
    }
}

pub(crate) fn saved(world: &mut World, id: u64, result: Result<(), String>) {
    let mut state = world.resource_mut::<ProjectState>();
    if state
        .pending
        .as_ref()
        .is_none_or(|pending| pending.id != id)
    {
        return;
    }
    let pending = state.pending.take();
    state.error = result.err();
    if state.error.is_none()
        && let Some(menu) = pending.and_then(|pending| pending.menu)
    {
        world.commands().entity(menu).try_despawn();
    }
}

pub(crate) fn failed(world: &mut World, reason: &str) {
    let mut state = world.resource_mut::<ProjectState>();
    if state.pending.take().is_some() {
        state.error = Some(reason.into());
    }
}

pub(crate) fn catalog(world: &mut World, result: Result<ProjectCatalog, String>) -> Result {
    let catalog = match result {
        Ok(catalog) => catalog,
        Err(error) => {
            world.resource_mut::<ProjectState>().error = Some(format!("无法读取项目：{error}"));
            return Ok(());
        }
    };
    world.resource_mut::<ProjectState>().catalog = catalog;
    crate::archives::rebuild_model(world)
}

fn update(
    state: Res<ProjectState>,
    mut status: Query<(&mut Text, &mut Node), With<ProjectStatus>>,
    actions: Query<(Entity, Has<InteractionDisabled>), With<Action>>,
    mut commands: Commands,
) {
    let value = state
        .error
        .clone()
        .or_else(|| state.pending.as_ref().map(|_| "正在保存项目…".to_owned()))
        .or_else(|| state.catalog.issues.first().cloned())
        .or_else(|| {
            state.catalog.projects.iter().find_map(|view| {
                view.directory_error
                    .as_ref()
                    .map(|error| format!("{}：{error}", view.project.name))
            })
        })
        .unwrap_or_default();
    let display = if value.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    for (mut text, node) in &mut status {
        text.set_if_neq(Text(value.clone()));
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
    }
    for (entity, disabled) in &actions {
        match (state.pending.is_some(), disabled) {
            (true, false) => {
                commands.entity(entity).try_insert(InteractionDisabled);
            }
            (false, true) => {
                commands.entity(entity).try_remove::<InteractionDisabled>();
            }
            _ => {}
        }
    }
}
