use super::SettingsWindow;
use crate::config::{AvatarSettings, ConfigStore};
use agentelier_controls::{
    AgentelierButton, AgentelierButtonStyle, AvatarCrop, AvatarCropper, AvatarCropperState,
    UserAvatar,
};
use agentelier_login_ui::DataDirectoryReady;
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy::window::WindowCloseRequested;
use bevy_app_assets::user_avatar::{AvatarFile, AvatarOriginal, LoadedAvatar};
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::job::{JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::slider::{WidgetrySlider, WidgetrySliderHeadless, WidgetrySliderPlugin};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowTitle,
    owned_widgetry_window, widgetry_window_target,
};
use std::io;
use std::path::PathBuf;
use tracing::warn;

#[derive(Resource, Default)]
struct Editor {
    directory: Option<PathBuf>,
    applied: Option<Applied>,
    configured: bool,
    target: Option<Entity>,
    draft: Option<Draft>,
    pending: Option<Pending>,
    message: Option<String>,
}

#[derive(Clone)]
struct Applied {
    settings: AvatarSettings,
    image: Handle<Image>,
}

#[derive(Clone)]
struct Draft {
    original: Original,
    image: Handle<Image>,
}

#[derive(Clone)]
enum Original {
    Chosen(AvatarOriginal),
    Stored(AvatarFile),
}

struct Pending {
    id: JobId,
    receiver: JobReceiver<io::Result<Outcome>>,
    received: bool,
}

enum Outcome {
    Restored(AvatarSettings, Box<LoadedAvatar>),
    Chosen(Entity, Box<LoadedAvatar>),
    Saved(Applied),
    Removed,
}

#[derive(Component, Clone, Default)]
struct AvatarPicker;

#[derive(Component, Clone, Default)]
struct AvatarZoom;

#[derive(Component, Clone, Default)]
struct AvatarMessage;

#[derive(Component, Clone, Copy)]
enum Action {
    Choose,
    Save,
    Remove,
}

pub(super) fn install(app: &mut App) {
    agentelier_controls::install_owned_window_close::<With<AvatarPicker>>(app);
    if !app.is_plugin_added::<WidgetrySliderPlugin>() {
        app.add_plugins(WidgetrySliderPlugin);
    }
    app.init_resource::<Editor>()
        .add_observer(initialize)
        .add_observer(close_picker)
        .add_systems(Update, receive)
        .add_systems(PostUpdate, sync.before(UiSystems::Content));
}

fn initialize(event: On<DataDirectoryReady>, mut commands: Commands) {
    let directory = event.environment.root().to_path_buf();
    commands.queue(move |world: &mut World| -> Result {
        world.resource_mut::<Editor>().directory = Some(directory.clone());
        if let Some(settings) = world.resource::<ConfigStore>().snapshot()?.user_avatar {
            world.resource_mut::<Editor>().configured = true;
            start(world, "正在读取用户头像…", move || {
                LoadedAvatar::restore(&directory, settings.original)
                    .map(|loaded| Outcome::Restored(settings, Box::new(loaded)))
            });
        }
        Ok(())
    });
}

pub(super) fn section() -> impl Scene {
    bsn! {
        Node { width: percent(100), margin: UiRect::top(px(8)), flex_direction: FlexDirection::Column, row_gap: px(10), flex_shrink: 0.0 }
        Children [
            Text("用户头像") WidgetryText TextFont { font_size: bevy::text::FontSize::Px(22.0) }--
            Node { width: percent(100), min_width: px(0), align_items: AlignItems::Center, column_gap: px(18), flex_wrap: FlexWrap::Wrap, row_gap: px(10) }
            Children [
                @AvatarCropper Name("SettingsAvatarCropper")--
                Node { flex_grow: 1.0, min_width: px(220), flex_direction: FlexDirection::Column, row_gap: px(8) }
                Children [
                    Text("缩放") WidgetryText--
                    @WidgetrySlider { @min: 1.0, @max: AvatarCrop::MAX_ZOOM, @value: {Some(1.0)} }
                    AvatarZoom Name("SettingsAvatarZoom")
                    Node { width: percent(100) }
                    on(zoom)--
                    Node { flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8) }
                    Children [
                        @button("更换图片", "SettingsChooseAvatar", Action::Choose)--
                        @button("保存头像", "SettingsSaveAvatar", Action::Save)--
                        @button("移除头像", "SettingsRemoveAvatar", Action::Remove)
                    ]
                ]
            ]--
            Text::default() WidgetryText AvatarMessage Name("SettingsAvatarFeedback")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
    }
}

fn button(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    bsn! {
        @AgentelierButton {
            @style: AgentelierButtonStyle::Secondary,
            @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)}
        }
        template(move |_| Ok(action)) Name(name) on(activate)
    }
}

fn zoom(
    event: On<ValueChange<f32>>,
    sliders: Query<(), With<AvatarZoom>>,
    state: Res<Editor>,
    mut croppers: Query<&mut AvatarCropperState>,
) {
    if state.pending.is_none()
        && sliders.contains(event.source)
        && let Some(target) = state.target
        && let Ok(mut cropper) = croppers.get_mut(target)
        && cropper.crop().zoom != event.value
    {
        cropper.set_zoom(event.value);
    }
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    match *actions.get(event.entity)? {
        Action::Choose => commands.queue(open_picker),
        Action::Save => commands.queue(save),
        Action::Remove => commands.queue(remove),
    }
    Ok(())
}

fn open_picker(world: &mut World) -> Result {
    if world.resource::<Editor>().pending.is_some()
        || world
            .query_filtered::<Entity, With<AvatarPicker>>()
            .iter(world)
            .next()
            .is_some()
    {
        return Ok(());
    }
    world.commands().spawn_scene_with_error_handler(bsn! {
        AvatarPicker Name("SettingsAvatarPickerWindow")
        @owned_widgetry_window(
            Window { title: "选择方形头像图片".into(), resolution: (840, 580).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickFile }
                Name("SettingsAvatarPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(chosen)
            },
        )
    });
    Ok(())
}

fn close_picker(
    _event: On<Despawn<SettingsWindow>>,
    query: Query<Entity, With<AvatarPicker>>,
    mut commands: Commands,
) {
    for entity in &query {
        commands.entity(entity).try_despawn();
    }
}

fn chosen(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let root = world
            .query_filtered::<Entity, With<AvatarPicker>>()
            .single(world)?;
        if let Some(window) = widgetry_window_target(world, root) {
            world.write_message(WindowCloseRequested { window });
        }
        let state = world.resource::<Editor>();
        if let WidgetryFileDialogResult::File(path) = result
            && let Some(target) = state.target.filter(|_| state.pending.is_none())
        {
            start(world, "正在读取头像图片…", move || {
                LoadedAvatar::read(&path).map(|loaded| Outcome::Chosen(target, Box::new(loaded)))
            });
        }
        Ok(())
    });
}

fn save(world: &mut World) -> Result {
    let state = world.resource::<Editor>();
    let Some((draft, directory, crop)) = state
        .target
        .filter(|_| state.pending.is_none())
        .and_then(|target| world.get::<AvatarCropperState>(target))
        .and_then(|cropper| {
            Some((
                state.draft.clone()?,
                state.directory.clone()?,
                cropper.crop(),
            ))
        })
    else {
        return Ok(());
    };
    let store = world.resource::<ConfigStore>().clone();
    start(world, "正在保存头像…", move || {
        let previous = store
            .snapshot()?
            .user_avatar
            .map(|settings| settings.original);
        let (file, created) = match draft.original {
            Original::Chosen(original) => (original.store(&directory)?, true),
            Original::Stored(file) => (file, false),
        };
        let settings = AvatarSettings::new(file, crop);
        if let Err(error) = store.save_avatar(Some(settings)) {
            if created {
                discard(file, &directory);
            }
            return Err(error);
        }
        if let Some(old) = previous.filter(|old| *old != file) {
            discard(old, &directory);
        }
        Ok(Outcome::Saved(Applied {
            settings,
            image: draft.image,
        }))
    });
    Ok(())
}

fn remove(world: &mut World) -> Result {
    let state = world.resource::<Editor>();
    let Some(directory) = state
        .directory
        .clone()
        .filter(|_| state.pending.is_none() && state.configured)
    else {
        return Ok(());
    };
    let store = world.resource::<ConfigStore>().clone();
    start(world, "正在移除用户头像…", move || {
        let previous = store.snapshot()?.user_avatar;
        store.save_avatar(None)?;
        if let Some(settings) = previous {
            discard(settings.original, &directory);
        }
        Ok(Outcome::Removed)
    });
    Ok(())
}

fn discard(file: AvatarFile, directory: &std::path::Path) {
    if let Err(failure) = file.discard(directory) {
        warn!(error = %failure, "清理不再引用的头像文件失败");
    }
}

fn start(
    world: &mut World,
    message: &'static str,
    work: impl FnOnce() -> io::Result<Outcome> + Send + 'static,
) {
    let outcome = world.resource::<JobManager>().spawn_thread(
        JobOptions {
            label: Some("用户头像文件操作".into()),
            ..default()
        },
        move |context| {
            if context.is_cancelled() {
                context.acknowledge_cancel();
                return Ok(());
            }
            drop(context.send_blocking(work()));
            Ok(())
        },
    );
    let mut state = world.resource_mut::<Editor>();
    match outcome {
        Ok((id, receiver)) => {
            state.pending = Some(Pending {
                id,
                receiver,
                received: false,
            });
            state.message = Some(message.into());
        }
        Err(failure) => state.message = Some(failure.to_string()),
    }
}

fn receive(world: &mut World) -> Result {
    let Some(mut pending) = world.resource_mut::<Editor>().pending.take() else {
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
                match result.as_ref() {
                    Ok(result) => apply_result(world, result),
                    Err(failure) => {
                        world.resource_mut::<Editor>().message = Some(failure.to_string())
                    }
                }
            }
            JobRead::Finished(outcome) => {
                world
                    .resource::<JobManager>()
                    .remove(pending.id)
                    .map_err(|failure| failure.into_bevy_error())?;
                if !pending.received {
                    world.resource_mut::<Editor>().message = Some(match outcome {
                        JobOutcome::Canceled => "头像操作已取消".into(),
                        JobOutcome::Completed => "头像操作未返回结果".into(),
                        JobOutcome::Failed(failure) => failure.to_string(),
                    });
                }
                return Ok(());
            }
            JobRead::Empty => {
                world.resource_mut::<Editor>().pending = Some(pending);
                return Ok(());
            }
            JobRead::Closed => return Ok(()),
        }
    }
}

fn apply_result(world: &mut World, outcome: &Outcome) {
    match outcome {
        Outcome::Restored(settings, loaded) => {
            let image = world
                .resource_mut::<Assets<Image>>()
                .add(loaded.image.clone());
            publish(
                world,
                Some(Applied {
                    settings: *settings,
                    image,
                }),
            );
            world.resource_mut::<Editor>().message = None;
        }
        Outcome::Chosen(target, loaded) => {
            if world.get::<AvatarCropperState>(*target).is_none() {
                world.resource_mut::<Editor>().message = None;
                return;
            }
            let image = world
                .resource_mut::<Assets<Image>>()
                .add(loaded.image.clone());
            world.resource_mut::<Editor>().draft = Some(Draft {
                original: Original::Chosen(loaded.original.clone()),
                image: image.clone(),
            });
            set_cropper(world, *target, Some(image), AvatarCrop::default());
            world.resource_mut::<Editor>().message = None;
        }
        Outcome::Saved(applied) => {
            publish(world, Some(applied.clone()));
            world.resource_mut::<Editor>().message = Some("已保存用户头像".into());
        }
        Outcome::Removed => {
            publish(world, None);
            world.resource_mut::<Editor>().message = Some("已恢复默认用户头像".into());
        }
    }
}

fn publish(world: &mut World, applied: Option<Applied>) {
    world.resource_mut::<UserAvatar>().0 = applied
        .as_ref()
        .map(|active| (active.image.clone(), active.settings.crop()));
    {
        let mut state = world.resource_mut::<Editor>();
        state.configured = applied.is_some();
        state.applied = applied;
    }
    reset_draft(world);
}

fn reset_draft(world: &mut World) {
    let state = world.resource::<Editor>();
    let target = state.target;
    let crop = state
        .applied
        .as_ref()
        .map_or_else(AvatarCrop::default, |applied| applied.settings.crop());
    let draft = state
        .applied
        .as_ref()
        .filter(|_| target.is_some())
        .map(|applied| Draft {
            original: Original::Stored(applied.settings.original),
            image: applied.image.clone(),
        });
    let image = draft.as_ref().map(|draft| draft.image.clone());
    world.resource_mut::<Editor>().draft = draft;
    if let Some(target) = target {
        set_cropper(world, target, image, crop);
    }
}

fn set_cropper(world: &mut World, target: Entity, image: Option<Handle<Image>>, crop: AvatarCrop) {
    if let Some(mut cropper) = world.get_mut::<AvatarCropperState>(target) {
        cropper.replace(image, crop);
    }
}

fn sync(world: &mut World) -> Result {
    let target = world
        .query_filtered::<Entity, With<AvatarCropperState>>()
        .single(world)
        .ok();
    if world.resource::<Editor>().target != target {
        world.resource_mut::<Editor>().target = target;
        reset_draft(world);
    }
    let Some(target) = target else {
        return Ok(());
    };
    let state = world.resource::<Editor>();
    let message = state.message.clone().unwrap_or_default();
    let has_draft = state.draft.is_some();
    let allowed = state.pending.is_none();
    let configured = state.configured;
    let zoom = world
        .get::<AvatarCropperState>(target)
        .map_or(1.0, |cropper| cropper.crop().zoom);
    if let Ok(slider) = world
        .query_filtered::<Entity, With<AvatarZoom>>()
        .single(world)
    {
        let shown = WidgetrySliderHeadless::value(world, slider)
            .map_err(|failure| failure.into_bevy_error())?;
        if (shown - zoom).abs() > 0.001 {
            WidgetrySliderHeadless::set_value(&mut world.commands(), slider, zoom);
        }
        set_enabled(world, slider, allowed && has_draft);
    }
    let buttons = world
        .query::<(Entity, &Action)>()
        .iter(world)
        .map(|(entity, action)| (entity, *action))
        .collect::<Vec<_>>();
    for (entity, action) in buttons {
        let ready = match action {
            Action::Choose => true,
            Action::Save => has_draft,
            Action::Remove => configured,
        };
        set_enabled(world, entity, allowed && ready);
    }
    for mut text in world
        .query_filtered::<&mut Text, With<AvatarMessage>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(message.clone()));
    }
    Ok(())
}

fn set_enabled(world: &mut World, entity: Entity, enabled: bool) {
    let mut entity = world.entity_mut(entity);
    if enabled && entity.contains::<InteractionDisabled>() {
        entity.remove::<InteractionDisabled>();
    } else if !enabled && !entity.contains::<InteractionDisabled>() {
        entity.insert(InteractionDisabled);
    }
}
