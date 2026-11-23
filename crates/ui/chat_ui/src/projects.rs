use agentelier_agent_host::AgentHost;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::{ProjectCatalog, ProjectChange, ProjectId, SessionId};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent};
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowTitle,
    owned_widgetry_window, widgetry_window_target,
};
use std::io;

const GAP: f32 = 8.0;

#[derive(Clone, PartialEq, Reflect)]
struct Choice {
    id: Option<ProjectId>,
    name: String,
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
struct ProjectState {
    #[reflect(ignore)]
    catalog: ProjectCatalog,
    selected: Option<ProjectId>,
    pending: Option<u64>,
    confirm_delete: bool,
    error: Option<String>,
}

#[derive(Resource)]
struct Source(Entity);

#[derive(Component, Clone, Default)]
struct ProjectPickerWindow;
#[derive(Component, Clone, Default)]
struct ProjectEditor;
#[derive(Component, Clone, Default)]
struct ProjectConfirmation;
#[derive(Component, Clone, Default)]
struct ProjectName;
#[derive(Component, Clone, Default)]
struct ProjectPath;
#[derive(Component, Clone, Default)]
struct ProjectStatus;
#[derive(Component, Clone, Default)]
struct ProjectFilter;

#[derive(Component, Clone, Copy)]
enum Action {
    Create,
    Rename,
    AskDelete,
    CancelDelete,
    Delete,
}

pub(crate) fn install(app: &mut App) -> Result {
    app.register_widgetry_combo_box::<Choice>()
        .map_err(|failure| failure.into_bevy_error())?;
    app.init_resource::<ProjectState>()
        .register_type::<ProjectState>()
        .add_systems(Startup, initialize)
        .add_systems(
            PostUpdate,
            (update, restore_filter).before(UiSystems::Content),
        );
    Ok(())
}

fn initialize(mut commands: Commands) {
    let source = commands.spawn(WidgetryListModel::<Choice>::default()).id();
    commands.insert_resource(Source(source));
}

pub(crate) fn source(world: &World) -> Entity {
    world.resource::<Source>().0
}

pub(crate) fn visible(world: &World, session: SessionId) -> bool {
    let state = world.resource::<ProjectState>();
    state.catalog.membership.get(&session).copied() == state.selected
}

pub(crate) fn forget_session(world: &mut World, session: SessionId) {
    world
        .resource_mut::<ProjectState>()
        .catalog
        .membership
        .remove(&session);
}

fn choice_selector(source: Entity, hint: &'static str) -> impl Scene {
    bsn! {
        @WidgetryComboBox::<Choice> {
            @source: source,
            @placeholder: {WidgetryComboBoxContent::new(move || bsn_list! { Text(hint) WidgetryText })},
            @renderer: {WidgetryListViewRenderer::new(|choice: &Choice| {
                let label = choice.name.clone();
                bsn_list! { Text(label) WidgetryText TextLayout::no_wrap() }
            })},
        }
        Node { width: percent(100), flex_shrink: 0.0 }
    }
}

pub(crate) fn panel(world: &World) -> impl Scene + use<> {
    let source = source(world);
    let name = selected_name(world.resource::<ProjectState>());
    bsn! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            @button("新建项目", "CreateProject", Action::Create)--
            @choice_selector(source, "单独聊天")
            ProjectFilter Name("ProjectFilter") on(select)--
            ProjectEditor Name("ProjectEditor")
            Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP), display: Display::None }
            Children [
                ProjectPath Name("ProjectDirectory") Text::default() WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                @WidgetryTextField { @value: name, @hint_text: {Some("项目名称".into())} }
                ProjectName Name("ProjectNameInput") Node { width: percent(100), flex_shrink: 0.0 }--
                Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                Children [
                    @button("保存名称", "RenameProject", Action::Rename)--
                    @button("删除项目", "DeleteProject", Action::AskDelete)
                ]--
                ProjectConfirmation
                Node { flex_direction: FlexDirection::Column, row_gap: px(GAP), display: Display::None }
                Children [
                    Text("项目中的会话将转为单独聊天。原文件与历史保留。") WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                    Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                    Children [
                        @button("确认删除", "ConfirmDeleteProject", Action::Delete)--
                        @button("取消", "CancelDeleteProject", Action::CancelDelete)
                    ]
                ]
            ]--
            ProjectStatus Name("ProjectStatus") Text::default() WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)
        ]
    }
}

pub(crate) fn move_selector(source: Entity) -> impl Scene {
    bsn! {
        @choice_selector(source, "移动会话到…")
        Name("MoveSessionProject") on(move_session)
    }
}

fn button(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    let style = match action {
        Action::Create | Action::Rename => AgentelierButtonStyle::Primary,
        Action::AskDelete | Action::Delete => AgentelierButtonStyle::Danger,
        Action::CancelDelete => AgentelierButtonStyle::Secondary,
    };
    bsn! {
        @AgentelierButton {
            @style: style,
            @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)},
        }
        Name(name) template(move |_| Ok(action)) on(activate)
    }
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    sources: Res<Source>,
    choices: Query<&WidgetryListModel<Choice>>,
    mut commands: Commands,
) -> Result {
    let Some(item) = event.value else {
        return Ok(());
    };
    let choice = choices
        .get(sources.0)?
        .get_by_id(item)
        .ok_or_else(|| io::Error::other("项目选项已变化"))?;
    let project = choice.id;
    commands.queue(move |world: &mut World| {
        let mut state = world.resource_mut::<ProjectState>();
        if state.selected == project {
            return Ok(());
        }
        state.selected = project;
        state.confirm_delete = false;
        sync_name(world)?;
        crate::archives::rebuild_model(world)
    });
    Ok(())
}

fn move_session(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    sources: Res<Source>,
    choices: Query<&WidgetryListModel<Choice>>,
    mut commands: Commands,
) -> Result {
    let Some(item) = event.value else {
        return Ok(());
    };
    let choice = choices
        .get(sources.0)?
        .get_by_id(item)
        .ok_or_else(|| io::Error::other("项目选项已变化"))?;
    let project = choice.id;
    let selector = event.source;
    commands.queue(move |world: &mut World| {
        if let Some(session) = crate::session_actions::target(world) {
            submit(world, ProjectChange::Move { session, project });
        }
        WidgetryComboBox::<Choice>::clear_selection(&mut world.commands(), selector);
    });
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        let state = world.resource::<ProjectState>();
        if state.pending.is_some() {
            return Ok(());
        }
        let selected = state.selected;
        match action {
            Action::Create => pick_directory(world),
            Action::AskDelete => world.resource_mut::<ProjectState>().confirm_delete = true,
            Action::CancelDelete => world.resource_mut::<ProjectState>().confirm_delete = false,
            Action::Delete => {
                if let Some(project) = selected
                    && state.confirm_delete
                {
                    submit(world, ProjectChange::Delete { project });
                }
            }
            Action::Rename => {
                if let Some(project) = selected {
                    let field = world
                        .query_filtered::<Entity, With<ProjectName>>()
                        .single(world)?;
                    let name = WidgetryTextFieldHeadless::text(world, field)
                        .map_err(|failure| failure.into_bevy_error())?;
                    submit(world, ProjectChange::Rename { project, name });
                }
            }
        }
        Ok(())
    });
    Ok(())
}

fn pick_directory(world: &mut World) {
    if world
        .query_filtered::<Entity, With<ProjectPickerWindow>>()
        .iter(world)
        .next()
        .is_some()
    {
        return;
    }
    world.commands().spawn_scene_with_error_handler(bsn! {
        ProjectPickerWindow Name("ProjectPickerWindow")
        @owned_widgetry_window(
            Window { title: "选择项目目录".into(), resolution: (840, 580).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
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
            );
        }
        Ok(())
    });
}

fn submit(world: &mut World, change: ProjectChange) {
    if world.resource::<ProjectState>().pending.is_some() {
        return;
    }
    let result = world.resource::<AgentHost>().change_project(change);
    let mut state = world.resource_mut::<ProjectState>();
    match result {
        Ok(id) => {
            state.pending = Some(id);
            state.error = None;
        }
        Err(failure) => state.error = Some(failure.to_string()),
    }
}

pub(crate) fn saved(world: &mut World, id: u64, result: Result<(), String>) {
    let mut state = world.resource_mut::<ProjectState>();
    if state.pending == Some(id) {
        state.pending = None;
        state.error = result.err();
        if state.error.is_none() {
            state.confirm_delete = false;
        }
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
    let mut choices = vec![Choice {
        id: None,
        name: "单独聊天".into(),
    }];
    choices.extend(catalog.projects.iter().map(|view| Choice {
        id: Some(view.project.id),
        name: view.project.name.clone(),
    }));
    let (selected, name_changed) = {
        let mut state = world.resource_mut::<ProjectState>();
        let previous_name = selected_name(&state);
        state.catalog = catalog;
        if !choices.iter().any(|choice| choice.id == state.selected) {
            state.selected = None;
            state.confirm_delete = false;
        }
        (state.selected, previous_name != selected_name(&state))
    };
    let source = source(world);
    let mut model = world
        .get_mut::<WidgetryListModel<Choice>>(source)
        .ok_or_else(|| io::Error::other("项目列表数据源不存在"))?;
    if model.len() != choices.len()
        || choices
            .iter()
            .enumerate()
            .any(|(index, choice)| model.get(index) != Some(choice))
    {
        model.clear();
        for choice in &choices {
            model
                .push(choice.clone())
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    if let Some(index) = choices.iter().position(|choice| choice.id == selected) {
        let filters = world
            .query_filtered::<Entity, With<ProjectFilter>>()
            .iter(world)
            .collect::<Vec<_>>();
        for filter in filters {
            WidgetryComboBox::<Choice>::select_index(&mut world.commands(), filter, index);
        }
    }
    if name_changed {
        sync_name(world)?;
    }
    crate::archives::rebuild_model(world)
}

fn selected_name(state: &ProjectState) -> String {
    state
        .catalog
        .projects
        .iter()
        .find(|view| Some(view.project.id) == state.selected)
        .map(|view| view.project.name.clone())
        .unwrap_or_default()
}

fn sync_name(world: &mut World) -> Result {
    let name = selected_name(world.resource::<ProjectState>());
    let names = world
        .query_filtered::<Entity, With<ProjectName>>()
        .iter(world)
        .collect::<Vec<_>>();
    for field in names {
        WidgetryTextFieldHeadless::set_text_in_world(world, field, &name)
            .map_err(|failure| failure.into_bevy_error())?;
    }
    Ok(())
}

fn restore_filter(
    state: Res<ProjectState>,
    source: Res<Source>,
    models: Query<&WidgetryListModel<Choice>>,
    filters: Query<Entity, Added<ProjectFilter>>,
    mut commands: Commands,
) -> Result {
    if filters.is_empty() {
        return Ok(());
    }
    let model = models.get(source.0)?;
    if let Some(index) = (0..model.len()).find(|&index| {
        model
            .get(index)
            .is_some_and(|choice| choice.id == state.selected)
    }) {
        for entity in &filters {
            WidgetryComboBox::<Choice>::select_index(&mut commands, entity, index);
        }
    }
    Ok(())
}

fn update(
    state: Res<ProjectState>,
    mut nodes: Query<
        (&mut Node, Has<ProjectEditor>, Has<ProjectConfirmation>),
        Or<(With<ProjectEditor>, With<ProjectConfirmation>)>,
    >,
    mut texts: Query<
        (&mut Text, Has<ProjectPath>, Has<ProjectStatus>),
        Or<(With<ProjectPath>, With<ProjectStatus>)>,
    >,
    actions: Query<(Entity, Has<InteractionDisabled>), With<Action>>,
    mut commands: Commands,
) {
    let project = state
        .catalog
        .projects
        .iter()
        .find(|view| Some(view.project.id) == state.selected);
    for (mut node, editor, confirmation) in &mut nodes {
        let visible = editor && project.is_some() || confirmation && state.confirm_delete;
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (mut text, path, status) in &mut texts {
        if path {
            let value = project.map_or_else(String::new, |view| {
                let path = bevy_widgetry_path_display::display_path(&view.project.directory);
                view.directory_error
                    .as_ref()
                    .map_or_else(|| path.clone(), |error| format!("{path}\n{error}"))
            });
            text.set_if_neq(Text(value));
        }
        if status {
            let value = state
                .error
                .clone()
                .or_else(|| state.pending.map(|_| "正在保存项目…".to_owned()))
                .or_else(|| state.catalog.issues.first().cloned())
                .unwrap_or_default();
            text.set_if_neq(Text(value));
        }
    }
    for (entity, disabled) in &actions {
        match (state.pending.is_some(), disabled) {
            (true, false) => {
                commands.entity(entity).insert(InteractionDisabled);
            }
            (false, true) => {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            _ => {}
        }
    }
}
