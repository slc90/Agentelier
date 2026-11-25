use super::SettingsWindow;
use crate::config::{AvatarSettings, ConfigStore};
use agentelier_controls::{
    AgentelierAvatar, AgentelierButton, AgentelierButtonStyle, AvatarCrop, AvatarCropperState,
    UserAvatar, avatar_cropper,
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
use bevy_widgetry::icon::WidgetryIcon;
use bevy_widgetry::job::{JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::slider::{WidgetrySlider, WidgetrySliderPlugin};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::window::{
    WidgetryWindowBackground, owned_widgetry_window, widgetry_window_target,
};
use bevy_widgetry_asset::BuiltinIcon;
use std::io;
use std::path::PathBuf;
use tracing::warn;

#[derive(Resource, Default)]
struct Editor {
    directory: Option<PathBuf>,
    configured: bool,
    pending: Option<Pending>,
    message: Option<String>,
}

#[derive(Clone)]
struct Applied {
    settings: AvatarSettings,
    image: Handle<Image>,
}

#[derive(Component, Clone)]
struct Draft {
    original: AvatarOriginal,
    image: Handle<Image>,
}

struct Pending {
    id: JobId,
    receiver: JobReceiver<io::Result<Outcome>>,
    received: bool,
}

enum Outcome {
    Restored(AvatarSettings, Box<LoadedAvatar>),
    Chosen(Entity, Box<LoadedAvatar>),
    Saved(Entity, Applied),
    Removed,
}

#[derive(Component, Clone, Default)]
struct AvatarEditorWindow;

#[derive(Component, Clone, Copy)]
struct AvatarPicker(Entity);

#[derive(Component, Clone, Default)]
struct AvatarZoom;

#[derive(Component, Clone, Default)]
struct AvatarMessage;

#[derive(Component, Clone, Copy)]
enum Action {
    Choose,
    Save,
    Remove,
    Cancel,
}

pub(super) fn install(app: &mut App) {
    agentelier_controls::install_owned_window_close::<
        Or<(With<AvatarEditorWindow>, With<AvatarPicker>)>,
    >(app);
    if !app.is_plugin_added::<WidgetrySliderPlugin>() {
        app.add_plugins(WidgetrySliderPlugin);
    }
    app.init_resource::<Editor>()
        .add_observer(initialize)
        .add_observer(close_windows_with_settings)
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
    let avatar_size = 88.0_f32;
    let remove_size = 24.0_f32;
    let remove_gap = 7.0_f32;
    bsn! {
        Node { width: percent(100), margin: UiRect::top(px(8)), flex_direction: FlexDirection::Column, row_gap: px(10), flex_shrink: 0.0 }
        Children [
            Text("用户头像") WidgetryText TextFont { font_size: bevy::text::FontSize::Px(22.0) }--
            Node { align_items: AlignItems::Start, column_gap: px(remove_gap) }
            Children [
                @AgentelierButton {
                    @style: AgentelierButtonStyle::Icon,
                    @children: bsn_list! { @AgentelierAvatar { @size: avatar_size } },
                }
                Name("SettingsChooseAvatar") Node { width: px(avatar_size), height: px(avatar_size) }
                template(|_| Ok(Action::Choose)) on(activate)--
                @AgentelierButton {
                    @style: AgentelierButtonStyle::DangerIcon,
                    @children: bsn_list! {
                        @WidgetryIcon { @path: {BuiltinIcon::WindowClose.path()}, @max_size: {Some(UVec2::splat(16))} }
                        Pickable::IGNORE
                    },
                }
                Name("SettingsRemoveAvatar")
                Node { display: Display::None, width: px(remove_size), height: px(remove_size), min_height: px(0) }
                template(|_| Ok(Action::Remove)) on(activate)
            ]--
            Text::default() WidgetryText AvatarMessage Name("SettingsAvatarFeedback")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
    }
}

fn editor(draft: Draft, pixels: UVec2) -> impl Scene {
    let cropper = avatar_cropper(draft.image.clone(), pixels, 300.0);
    bsn! {
        AvatarEditorWindow Name("SettingsAvatarEditorWindow")
        template(move |_| Ok(draft.clone()))
        @owned_widgetry_window(
            Window { title: "编辑头像".into(), resolution: (650, 590).into(), resizable: false, ..default() },
            agentelier_controls::MANAGEMENT_WINDOW_CONTROLS,
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(12), padding: UiRect::all(px(18)) }
                Children [
                    @cropper--
                    Text("拖动圆形选区，调整裁剪范围") WidgetryText--
                    @WidgetrySlider { @min: 1.0, @max: AvatarCrop::MAX_ZOOM, @value: {Some(1.0)} }
                    AvatarZoom Name("SettingsAvatarZoom") Node { width: percent(100) } on(zoom)--
                    Text::default() WidgetryText AvatarMessage Name("AvatarEditorFeedback")
                    TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)--
                    Node { justify_content: JustifyContent::End, column_gap: px(8) }
                    Children [
                        @button("取消", "CancelAvatarEdit", Action::Cancel, AgentelierButtonStyle::Secondary)--
                        @button("保存", "SettingsSaveAvatar", Action::Save, AgentelierButtonStyle::Primary)
                    ]
                ]
            },
        )
    }
}

fn button(
    label: &'static str,
    name: &'static str,
    action: Action,
    style: AgentelierButtonStyle,
) -> impl Scene {
    bsn! {
        @AgentelierButton { @style: style, @children: bsn_list! { Text(label) WidgetryText } }
        template(move |_| Ok(action)) Name(name) on(activate)
    }
}

fn zoom(
    event: On<ValueChange<f32>>,
    state: Res<Editor>,
    mut croppers: Query<&mut AvatarCropperState>,
) {
    if state.pending.is_none()
        && let Ok(mut cropper) = croppers.single_mut()
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
        Action::Cancel => commands.queue(|world: &mut World| {
            if world.resource::<Editor>().pending.is_none() {
                close_editor(world);
                world.resource_mut::<Editor>().message = None;
            }
        }),
    }
    Ok(())
}

fn open_picker(world: &mut World) -> Result {
    if world.resource::<Editor>().pending.is_some()
        || world
            .query_filtered::<(), Or<(With<AvatarEditorWindow>, With<AvatarPicker>)>>()
            .iter(world)
            .next()
            .is_some()
    {
        return Ok(());
    }
    let settings = world
        .query_filtered::<Entity, With<SettingsWindow>>()
        .single(world)?;
    world.commands().spawn_scene_with_error_handler(bsn! {
        Name("SettingsAvatarPickerWindow") template(move |_| Ok(AvatarPicker(settings)))
        @owned_widgetry_window(
            Window { title: "选择头像图片".into(), resolution: (840, 580).into(), ..default() },
            agentelier_controls::FILE_DIALOG_WINDOW_CONTROLS,
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickFile }
                Name("SettingsAvatarPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(chosen)
            },
        )
    });
    Ok(())
}

fn chosen(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let (root, &AvatarPicker(settings)) =
            world.query::<(Entity, &AvatarPicker)>().single(world)?;
        close_window(world, root);
        if let WidgetryFileDialogResult::File(path) = result
            && world.resource::<Editor>().pending.is_none()
        {
            start(world, "正在读取头像图片…", move || {
                LoadedAvatar::read(&path).map(|loaded| Outcome::Chosen(settings, Box::new(loaded)))
            });
        }
        Ok(())
    });
}

fn close_windows_with_settings(
    _: On<Despawn<SettingsWindow>>,
    windows: Query<Entity, Or<(With<AvatarEditorWindow>, With<AvatarPicker>)>>,
    mut commands: Commands,
) {
    for root in &windows {
        commands.queue(move |world: &mut World| close_window(world, root));
    }
}

fn close_editor(world: &mut World) {
    if let Ok(root) = world
        .query_filtered::<Entity, With<AvatarEditorWindow>>()
        .single(world)
    {
        close_window(world, root);
    }
}

fn close_window(world: &mut World, root: Entity) {
    if let Some(window) = widgetry_window_target(world, root) {
        world.write_message(WindowCloseRequested { window });
    }
}

fn save(world: &mut World) -> Result {
    let state = world.resource::<Editor>();
    if state.pending.is_some() {
        return Ok(());
    }
    let Some(directory) = state.directory.clone() else {
        return Ok(());
    };
    let Ok((root, draft)) = world.query::<(Entity, &Draft)>().single(world) else {
        return Ok(());
    };
    let draft = draft.clone();
    let crop = world.query::<&AvatarCropperState>().single(world)?.crop();
    let store = world.resource::<ConfigStore>().clone();
    start(world, "正在保存头像…", move || {
        let previous = store
            .snapshot()?
            .user_avatar
            .map(|settings| settings.original);
        let file = draft.original.store(&directory)?;
        let settings = AvatarSettings::new(file, crop);
        if let Err(error) = store.save_avatar(Some(settings)) {
            discard(file, &directory);
            return Err(error);
        }
        if let Some(old) = previous {
            discard(old, &directory);
        }
        Ok(Outcome::Saved(
            root,
            Applied {
                settings,
                image: draft.image,
            },
        ))
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
        Outcome::Chosen(settings, loaded) => {
            world.resource_mut::<Editor>().message = None;
            if world.get::<SettingsWindow>(*settings).is_none() {
                return;
            }
            let pixels = loaded.image.size();
            let image = world
                .resource_mut::<Assets<Image>>()
                .add(loaded.image.clone());
            let draft = Draft {
                original: loaded.original.clone(),
                image,
            };
            world
                .commands()
                .spawn_scene_with_error_handler(editor(draft, pixels));
        }
        Outcome::Saved(root, applied) => {
            publish(world, Some(applied.clone()));
            close_window(world, *root);
            world.resource_mut::<Editor>().message = None;
        }
        Outcome::Removed => {
            publish(world, None);
            world.resource_mut::<Editor>().message = None;
        }
    }
}

fn publish(world: &mut World, applied: Option<Applied>) {
    world.resource_mut::<UserAvatar>().0 = applied
        .as_ref()
        .map(|active| (active.image.clone(), active.settings.crop()));
    world.resource_mut::<Editor>().configured = applied.is_some();
}

fn sync(
    state: Res<Editor>,
    actions: Query<(Entity, &Action, Has<InteractionDisabled>)>,
    sliders: Query<(Entity, Has<InteractionDisabled>), With<AvatarZoom>>,
    mut buttons: Query<&mut Node, With<Action>>,
    mut labels: Query<(&mut Node, &mut Text), (With<AvatarMessage>, Without<Action>)>,
    mut commands: Commands,
) {
    let allowed = state.pending.is_none();
    for (entity, action, disabled) in &actions {
        if matches!(action, Action::Remove)
            && let Ok(node) = buttons.get_mut(entity)
        {
            node.map_unchanged(|node| &mut node.display)
                .set_if_neq(if state.configured {
                    Display::Flex
                } else {
                    Display::None
                });
        }
        set_enabled(&mut commands, entity, allowed, disabled);
    }
    for (entity, disabled) in &sliders {
        set_enabled(&mut commands, entity, allowed, disabled);
    }
    let message = state.message.as_deref().unwrap_or_default();
    for (node, mut label) in &mut labels {
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(if message.is_empty() {
                Display::None
            } else {
                Display::Flex
            });
        if label.0 != message {
            label.0 = message.into();
        }
    }
}

fn set_enabled(commands: &mut Commands, entity: Entity, enabled: bool, disabled: bool) {
    if enabled && disabled {
        commands.entity(entity).remove::<InteractionDisabled>();
    } else if !enabled && !disabled {
        commands.entity(entity).insert(InteractionDisabled);
    }
}
