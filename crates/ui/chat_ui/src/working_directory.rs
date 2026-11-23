use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{ProjectId, SessionId, SessionLocation};
use bevy::prelude::*;
use bevy::text::EditableTextSystems;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry_path_display::display_path;
use std::path::PathBuf;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub(crate) struct DirectoryState {
    input: String,
    #[reflect(ignore)]
    path: PathBuf,
    #[reflect(ignore)]
    standalone: PathBuf,
    #[reflect(ignore)]
    project: Option<(ProjectId, PathBuf)>,
    #[reflect(ignore)]
    submitted_override: Option<(ProjectId, PathBuf)>,
    error: Option<String>,
}

impl DirectoryState {
    pub(crate) fn location(&self) -> SessionLocation {
        match &self.project {
            Some((project, default)) => SessionLocation::InProject {
                project: *project,
                directory_override: (self.path != *default).then(|| self.path.clone()),
            },
            None => SessionLocation::Standalone(self.path.clone()),
        }
    }

    fn select_path(&mut self, path: PathBuf) {
        self.input = display_path(&path);
        self.path = path;
        self.error = None;
    }
}

pub(crate) fn sync_project(world: &mut World) -> Result {
    let project = crate::projects::selected_project(world)
        .map(|project| (project.id, project.directory.clone()));
    let mut state = world.resource_mut::<DirectoryState>();
    if state.project == project {
        return Ok(());
    }
    if state.project.is_none() {
        state.standalone = state.path.clone();
    }
    let path = match &project {
        Some((id, directory)) => match &state.project {
            Some((previous, default)) if previous == id && state.path != *default => {
                state.path.clone()
            }
            _ => directory.clone(),
        },
        None => state.standalone.clone(),
    };
    state.project = project;
    state.select_path(path);
    sync_fields(world)
}

pub(crate) fn submitted(world: &mut World) {
    let mut state = world.resource_mut::<DirectoryState>();
    state.submitted_override = state
        .project
        .as_ref()
        .and_then(|(id, default)| (state.path != *default).then(|| (*id, state.path.clone())));
}

pub(crate) fn completed(world: &mut World, created: bool) -> Result {
    let mut state = world.resource_mut::<DirectoryState>();
    let Some((id, submitted)) = state.submitted_override.take() else {
        return Ok(());
    };
    if created
        && state.path == submitted
        && let Some((current, default)) = &state.project
        && *current == id
    {
        let default = default.clone();
        state.select_path(default);
        sync_fields(world)?;
    }
    Ok(())
}

#[derive(Component, Clone, Default)]
struct DirectoryPanel;
#[derive(Component, Clone, Default)]
struct DirectoryInput;
#[derive(Component, Clone, Default)]
struct DirectoryStatus;
#[derive(Component, Clone, Copy)]
enum DirectoryPicker {
    NewSession(Option<ProjectId>),
    OpenSession(SessionId),
}

pub(crate) fn install(app: &mut App) {
    let (path, error) = match std::env::current_dir() {
        Ok(path) => (path, None),
        Err(failure) => (
            PathBuf::new(),
            Some(format!("无法读取启动工作目录：{failure}")),
        ),
    };
    app.insert_resource(DirectoryState {
        input: display_path(&path),
        standalone: path.clone(),
        path,
        project: None,
        submitted_override: None,
        error,
    })
    .register_type::<DirectoryState>()
    .add_systems(
        PostUpdate,
        sync.after(EditableTextSystems)
            .before(UiSystems::PostLayout),
    );
}

pub(crate) fn panel(state: &DirectoryState) -> impl Scene {
    let input = state.input.clone();
    let error = state.error.clone().unwrap_or_default();
    bsn! {
        DirectoryPanel Name("WorkingDirectoryPanel")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
        Children [
            Node { width: percent(100), column_gap: px(8), align_items: AlignItems::Center }
            Children [
                Text("新会话工作目录") WidgetryText--
                @WidgetryTextField { @value: {input}, @hint_text: {Some("输入已存在的绝对目录路径".into())} }
                DirectoryInput Name("WorkingDirectoryInput") Node { flex_grow: 1.0, min_width: px(0) }--
                @AgentelierButton { @children: {Some(Box::new(bsn_list! {Text("选择目录") WidgetryText}) as Box<dyn SceneList>)} }
                Name("ChooseWorkingDirectory") on(choose)
            ]--
            Text(error) WidgetryText DirectoryStatus Name("WorkingDirectoryStatus")
        ]
    }
}

fn choose(_: On<Activate>, mut commands: Commands) {
    commands.queue(|world: &mut World| -> Result {
        let project = crate::projects::selected_project(world).map(|project| project.id);
        pick(world, DirectoryPicker::NewSession(project))
    });
}

pub(crate) fn choose_for_session(world: &mut World, session: SessionId) -> Result {
    pick(world, DirectoryPicker::OpenSession(session))
}

fn pick(world: &mut World, target: DirectoryPicker) -> Result {
    if world
        .query::<&DirectoryPicker>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let panel = world
        .query_filtered::<Entity, With<DirectoryPanel>>()
        .single(world)?;
    world.commands().spawn_scene_with_error_handler(bsn! {
        @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
        Name("WorkingDirectoryPicker") ChildOf(panel) template(move |_| Ok(target))
        Node { width: percent(100), height: px(320), flex_shrink: 0.0 }
        on(selected)
    });
    Ok(())
}

fn selected(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let entity = event.entity;
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let target = world.get::<DirectoryPicker>(entity).copied();
        world.entity_mut(entity).despawn();
        if let WidgetryFileDialogResult::Directory(path) = result {
            match target {
                Some(DirectoryPicker::NewSession(project))
                    if project
                        == crate::projects::selected_project(world).map(|project| project.id) =>
                {
                    world.resource_mut::<DirectoryState>().select_path(path);
                    sync_fields(world)?;
                }
                Some(DirectoryPicker::OpenSession(session)) => {
                    crate::conversation::set_working_directory(world, session, path)?;
                }
                _ => {}
            }
        }
        Ok(())
    });
}

fn sync_fields(world: &mut World) -> Result {
    let value = world.resource::<DirectoryState>().input.clone();
    let mut fields = world.query_filtered::<Entity, With<DirectoryInput>>();
    let entities: Vec<_> = fields.iter(world).collect();
    for field in entities {
        WidgetryTextFieldHeadless::set_text_in_world(world, field, &value)
            .map_err(|failure| failure.into_bevy_error())?;
    }
    Ok(())
}

fn sync(world: &mut World) -> Result {
    let mut fields = world.query_filtered::<Entity, With<DirectoryInput>>();
    let entities: Vec<_> = fields.iter(world).collect();
    for entity in entities {
        let value = WidgetryTextFieldHeadless::text(world, entity)
            .map_err(|failure| failure.into_bevy_error())?;
        if value != world.resource::<DirectoryState>().input {
            let mut state = world.resource_mut::<DirectoryState>();
            state.path = match &state.project {
                Some((_, default)) if value == display_path(default) => default.clone(),
                _ => PathBuf::from(&value),
            };
            state.input = value;
            state.error = None;
        }
    }
    let status = world
        .resource::<DirectoryState>()
        .error
        .clone()
        .unwrap_or_default();
    let mut texts = world.query_filtered::<&mut Text, With<DirectoryStatus>>();
    for mut text in texts.iter_mut(world) {
        text.0.clone_from(&status);
    }
    Ok(())
}
