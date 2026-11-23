use crate::config::{ALLOWED_SCALE_FACTORS, ConfigStore};
use agentelier_chat_ui::{ModelState, OpenSettings, SessionModelDefaults};
use agentelier_login::validate_data_directory;
use agentelier_login_ui::DataDirectoryReady;
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::ValueChange;
use bevy::window::WindowCloseRequested;
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt};
use bevy_widgetry::file_dialog::{WidgetryFileDialogResult, WidgetryFileDialogResultEvent};
use bevy_widgetry::job::{JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text_field::WidgetryTextFieldHeadless;
use bevy_widgetry::ui_scale::WidgetryUiScale;
use bevy_widgetry::window::widgetry_window_target;
use bevy_widgetry_path_display::{display_path, simplify_path};
use std::path::PathBuf;

mod avatar;
mod view;

#[derive(Resource, Default)]
struct Settings {
    current_directory: Option<PathBuf>,
    draft: Option<SessionModelDefaults>,
    pending_directory: Option<PendingDirectory>,
    message: Option<String>,
}

struct PendingDirectory {
    id: JobId,
    receiver: JobReceiver<Result<(), String>>,
    received: bool,
}

#[derive(Component, Clone, Default)]
struct SettingsWindow;

#[derive(Component, Clone, Default)]
struct DirectoryPicker;

#[derive(Component, Clone, Copy)]
enum DirectoryPath {
    Current,
    Next,
}

#[derive(Component, Clone, Default)]
struct SettingsMessage;

#[derive(Component, Clone, Default)]
struct DefaultsSummary;

#[derive(Clone, Copy)]
enum Field {
    Scale,
    Model,
    Effort,
}

#[derive(Component, Clone, Copy)]
struct Selector {
    field: Field,
    source: Entity,
}

#[derive(Clone, PartialEq)]
enum Choice {
    Scale(f32),
    Model { slug: String, label: String },
    Effort(String),
}

impl Choice {
    fn label(&self) -> String {
        match self {
            Self::Scale(scale) => format!("{scale:.2}"),
            Self::Model { label, .. } | Self::Effort(label) => label.clone(),
        }
    }
}

pub(crate) fn install(app: &mut App) -> Result {
    avatar::install(app);
    app.register_widgetry_combo_box::<Choice>()
        .map_err(|failure| failure.into_bevy_error())?;
    app.init_resource::<Settings>()
        .add_observer(capture_directory)
        .add_observer(open)
        .add_observer(close_picker_with_settings)
        .add_systems(Update, receive_directory)
        .add_systems(PostUpdate, sync.before(UiSystems::Content));
    Ok(())
}

fn capture_directory(event: On<DataDirectoryReady>, mut state: ResMut<Settings>) {
    state.current_directory = Some(event.environment.root().to_path_buf());
}

fn open(
    _event: On<OpenSettings>,
    settings: Query<(), With<SettingsWindow>>,
    config: Res<ConfigStore>,
    mut state: ResMut<Settings>,
    mut commands: Commands,
) -> Result {
    if !settings.is_empty() || state.current_directory.is_none() {
        return Ok(());
    }
    let saved = config.session_model_defaults()?;
    state.draft = Some(saved);
    if state.pending_directory.is_none() {
        state.message = None;
    }
    let selectors = [Field::Scale, Field::Model, Field::Effort].map(|field| Selector {
        field,
        source: commands.spawn(WidgetryListModel::<Choice>::default()).id(),
    });
    let root = commands
        .spawn_scene_with_error_handler(view::window(selectors))
        .id();
    for selector in selectors {
        commands.entity(selector.source).insert(ChildOf(root));
    }
    Ok(())
}

fn close_picker_with_settings(
    _event: On<Despawn<SettingsWindow>>,
    pickers: Query<Entity, With<DirectoryPicker>>,
    mut commands: Commands,
) {
    for picker in &pickers {
        commands.entity(picker).try_despawn();
    }
}

fn sync(world: &mut World) -> Result {
    if world
        .query_filtered::<Entity, With<SettingsWindow>>()
        .single(world)
        .is_err()
    {
        return Ok(());
    }
    let config = world.resource::<ConfigStore>().snapshot()?;
    let state = world.resource::<Settings>();
    let Some(draft) = state.draft.clone() else {
        return Ok(());
    };
    let current = state.current_directory.clone();
    let next = config
        .data_directory
        .as_deref()
        .filter(|path| Some(simplify_path(path)) != current.as_deref().map(simplify_path));
    let message = state.message.clone().unwrap_or_default();
    let pending_directory = state.pending_directory.is_some();
    let models = world.resource::<ModelState>();
    let choices = [
        scale_choices(),
        model_choices(models),
        effort_choices(models, &draft.slug),
    ];
    let selectors = world
        .query::<(Entity, &Selector)>()
        .iter(world)
        .map(|(entity, selector)| (entity, *selector))
        .collect::<Vec<_>>();
    for (entity, selector) in selectors {
        let options = match selector.field {
            Field::Scale => &choices[0],
            Field::Model => &choices[1],
            Field::Effort => &choices[2],
        };
        sync_choices(
            world,
            entity,
            selector.source,
            options,
            |choice| match choice {
                Choice::Scale(value) => *value == config.scale_factor,
                Choice::Model { slug, .. } => *slug == draft.slug,
                Choice::Effort(effort) => *effort == draft.effort,
            },
        )?;
    }
    let paths = world
        .query::<(Entity, &DirectoryPath)>()
        .iter(world)
        .map(|(entity, path)| (entity, *path))
        .collect::<Vec<_>>();
    for (entity, path) in paths {
        let path = match path {
            DirectoryPath::Current => current.as_deref(),
            DirectoryPath::Next => next,
        };
        let value = path.map(display_path).unwrap_or_default();
        let visible = path.is_some();
        let shown = WidgetryTextFieldHeadless::text(world, entity)
            .map_err(|failure| failure.into_bevy_error())?;
        if shown != value {
            WidgetryTextFieldHeadless::set_text_in_world(world, entity, &value)
                .map_err(|failure| failure.into_bevy_error())?;
        }
        if let Some(mut node) = world.get_mut::<Node>(entity) {
            node.display = if visible {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
    for mut text in world
        .query_filtered::<&mut Text, With<SettingsMessage>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(message.clone()));
    }
    let summary = format!(
        "已保存 {} · {}",
        config.default_model_slug, config.default_reasoning_effort
    );
    for mut text in world
        .query_filtered::<&mut Text, With<DefaultsSummary>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(summary.clone()));
    }
    view::set_directory_busy(world, pending_directory, next.is_some());
    Ok(())
}

fn scale_choices() -> Vec<Choice> {
    ALLOWED_SCALE_FACTORS
        .into_iter()
        .map(Choice::Scale)
        .collect()
}

fn model_choices(catalog: &ModelState) -> Vec<Choice> {
    catalog
        .models
        .iter()
        .map(|model| Choice::Model {
            slug: model.slug.clone(),
            label: format!("{} · {}", model.display_name, model.slug),
        })
        .collect()
}

fn effort_choices(catalog: &ModelState, slug: &str) -> Vec<Choice> {
    catalog
        .models
        .iter()
        .find(|model| model.slug == slug)
        .map(|model| {
            model
                .supported_reasoning_levels
                .iter()
                .cloned()
                .map(Choice::Effort)
                .collect()
        })
        .unwrap_or_default()
}

fn sync_choices(
    world: &mut World,
    entity: Entity,
    source: Entity,
    choices: &[Choice],
    matches: impl Fn(&Choice) -> bool,
) -> Result {
    let selected = WidgetryComboBox::<Choice>::selected_id(world, entity)
        .map_err(|failure| failure.into_bevy_error())?;
    let desired = {
        let mut model = world
            .query::<&mut WidgetryListModel<Choice>>()
            .get_mut(world, source)?;
        if model.len() != choices.len()
            || choices
                .iter()
                .enumerate()
                .any(|(index, choice)| model.get(index) != Some(choice))
        {
            model.clear();
            for choice in choices {
                model
                    .push(choice.clone())
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
        choices
            .iter()
            .position(matches)
            .and_then(|index| model.id(index))
    };
    if selected != desired {
        if let Some(id) = desired {
            WidgetryComboBox::<Choice>::select_id(&mut world.commands(), entity, id);
        } else {
            WidgetryComboBox::<Choice>::clear_selection(&mut world.commands(), entity);
        }
    }
    Ok(())
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    selectors: Query<&Selector>,
    sources: Query<&WidgetryListModel<Choice>>,
    mut commands: Commands,
) {
    let Ok(selector) = selectors.get(event.source) else {
        return;
    };
    let choice = event
        .value
        .and_then(|id| sources.get(selector.source).ok()?.get_by_id(id))
        .cloned();
    if let Some(choice) = choice {
        commands.queue(move |world: &mut World| apply_choice(world, choice));
    }
}

fn apply_choice(world: &mut World, choice: Choice) -> Result {
    match choice {
        Choice::Scale(scale) => {
            let store = world.resource::<ConfigStore>();
            if store.snapshot()?.scale_factor != scale {
                match store.save_scale(scale) {
                    Ok(()) => {
                        world
                            .resource_mut::<WidgetryUiScale>()
                            .set(scale)
                            .map_err(|failure| failure.into_bevy_error())?;
                        world.resource_mut::<Settings>().message = None;
                    }
                    Err(failure) => {
                        world.resource_mut::<Settings>().message = Some(failure.to_string());
                    }
                }
            }
        }
        Choice::Model { slug, .. } => {
            let current = world.resource::<Settings>();
            if let Some(mut draft) = current.draft.clone()
                && draft.slug != slug
            {
                draft.slug = slug;
                let message = validate_defaults(world.resource::<ModelState>(), &draft).err();
                let mut state = world.resource_mut::<Settings>();
                state.draft = Some(draft);
                state.message = message;
            }
        }
        Choice::Effort(effort) => {
            let mut state = world.resource_mut::<Settings>();
            if let Some(draft) = &mut state.draft
                && draft.effort != effort
            {
                draft.effort = effort;
                state.message = None;
            }
        }
    }
    Ok(())
}

fn save_defaults(world: &mut World) -> Result {
    let Some(draft) = world.resource::<Settings>().draft.clone() else {
        return Ok(());
    };
    let result = validate_defaults(world.resource::<ModelState>(), &draft).and_then(|()| {
        world
            .resource::<ConfigStore>()
            .save_session_defaults(draft.slug, draft.effort)
            .map_err(|failure| failure.to_string())
    });
    world.resource_mut::<Settings>().message = Some(match result {
        Ok(()) => "已保存新会话默认设置".into(),
        Err(failure) => failure,
    });
    Ok(())
}

fn validate_defaults(catalog: &ModelState, draft: &SessionModelDefaults) -> Result<(), String> {
    let model = catalog
        .models
        .iter()
        .find(|model| model.slug == draft.slug)
        .ok_or_else(|| "所选模型不在当前模型目录中".to_owned())?;
    if !model.supported_reasoning_levels.contains(&draft.effort) {
        return Err(format!(
            "{} 不支持推理强度 {}，请选择支持的档位",
            model.slug, draft.effort
        ));
    }
    Ok(())
}

fn open_picker(world: &mut World) -> Result {
    let state = world.resource::<Settings>();
    if state.pending_directory.is_some() {
        return Ok(());
    }
    let current = state.current_directory.clone();
    if world
        .query_filtered::<Entity, With<DirectoryPicker>>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let initial = world
        .resource::<ConfigStore>()
        .snapshot()?
        .data_directory
        .or(current);
    world
        .commands()
        .spawn_scene_with_error_handler(view::picker(initial));
    Ok(())
}

fn chosen(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let root = world
            .query_filtered::<Entity, With<DirectoryPicker>>()
            .single(world)?;
        if let Some(window) = widgetry_window_target(world, root) {
            world.write_message(WindowCloseRequested { window });
        }
        if let WidgetryFileDialogResult::Directory(path) = result {
            save_directory(world, path)?;
        }
        Ok(())
    });
}

fn save_directory(world: &mut World, path: PathBuf) -> Result {
    let store = world.resource::<ConfigStore>().clone();
    let submission = world.resource::<JobManager>().spawn_thread(
        JobOptions {
            label: Some("保存下次启动数据目录".into()),
            ..default()
        },
        move |context| {
            if context.is_cancelled() {
                context.acknowledge_cancel();
                return Ok(());
            }
            let result = validate_data_directory(&path)
                .and_then(|path| store.remember_data_directory(&path))
                .map_err(|failure| failure.to_string());
            drop(context.send_blocking(result));
            Ok(())
        },
    );
    let mut state = world.resource_mut::<Settings>();
    match submission {
        Ok((id, receiver)) => {
            state.pending_directory = Some(PendingDirectory {
                id,
                receiver,
                received: false,
            });
            state.message = Some("正在检查并保存下次启动目录…".into());
        }
        Err(failure) => state.message = Some(failure.to_string()),
    }
    Ok(())
}

fn receive_directory(mut state: ResMut<Settings>, jobs: Res<JobManager>) -> Result {
    let Some(mut pending) = state.pending_directory.take() else {
        return Ok(());
    };
    loop {
        match pending
            .receiver
            .try_recv()
            .map_err(|failure| failure.into_bevy_error())?
        {
            JobRead::Item(result) => {
                pending.received = true;
                state.message = Some(match result.as_ref() {
                    Ok(()) => "已保存下次启动使用的数据目录".into(),
                    Err(failure) => failure.clone(),
                });
            }
            JobRead::Finished(outcome) => {
                jobs.remove(pending.id)
                    .map_err(|failure| failure.into_bevy_error())?;
                if !pending.received {
                    state.message = Some(match outcome {
                        JobOutcome::Canceled => "已取消目录检查".into(),
                        JobOutcome::Failed(failure) => failure.to_string(),
                        JobOutcome::Completed => "目录检查未返回结果".into(),
                    });
                }
                return Ok(());
            }
            JobRead::Empty => {
                state.pending_directory = Some(pending);
                return Ok(());
            }
            JobRead::Closed => {
                state.message = Some("目录检查提前结束".into());
                return Ok(());
            }
        }
    }
}
