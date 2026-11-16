use bevy::prelude::*;
use bevy::text::EditableTextSystems;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry_path_display::display_path;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub(crate) struct DirectoryState {
    pub(crate) input: String,
    error: Option<String>,
}
#[derive(Component, Clone, Default)]
struct DirectoryPanel;
#[derive(Component, Clone, Default)]
struct DirectoryInput;
#[derive(Component, Clone, Default)]
struct DirectoryStatus;
#[derive(Component, Clone, Default)]
struct DirectoryPicker;

pub(crate) fn install(app: &mut App) {
    let (input, error) = match std::env::current_dir() {
        Ok(path) => (display_path(&path), None),
        Err(failure) => (
            String::new(),
            Some(format!("无法读取启动工作目录：{failure}")),
        ),
    };
    app.insert_resource(DirectoryState { input, error })
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
                Text("本地工作目录") WidgetryText--
                @WidgetryTextField { @value: {input}, @hint_text: {Some("输入已存在的绝对目录路径".into())} }
                DirectoryInput Name("WorkingDirectoryInput") Node { flex_grow: 1.0, min_width: px(0) }--
                @WidgetryButton { @children: {Some(Box::new(bsn_list! {Text("选择目录") WidgetryText}) as Box<dyn SceneList>)} }
                Name("ChooseWorkingDirectory") on(choose)
            ]--
            Text(error) WidgetryText DirectoryStatus Name("WorkingDirectoryStatus")
        ]
    }
}

fn choose(
    _: On<Activate>,
    panels: Query<Entity, With<DirectoryPanel>>,
    pickers: Query<(), With<DirectoryPicker>>,
    mut commands: Commands,
) -> Result {
    if !pickers.is_empty() {
        return Ok(());
    }
    let panel = panels.single()?;
    commands.spawn_scene_with_error_handler(bsn! {
        @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
        DirectoryPicker Name("WorkingDirectoryPicker") ChildOf(panel)
        Node { width: percent(100), height: px(320), flex_shrink: 0.0 }
        on(selected)
    });
    Ok(())
}

fn selected(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let entity = event.entity;
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        if let WidgetryFileDialogResult::Directory(path) = result {
            let value = display_path(&path);
            world.resource_mut::<DirectoryState>().input = value.clone();
            world.resource_mut::<DirectoryState>().error = None;
            let mut fields = world.query_filtered::<Entity, With<DirectoryInput>>();
            let entities: Vec<_> = fields.iter(world).collect();
            for field in entities {
                WidgetryTextFieldHeadless::set_text_in_world(world, field, &value)
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
        world.entity_mut(entity).despawn();
        Ok(())
    });
}

fn sync(world: &mut World) -> Result {
    let mut fields = world.query_filtered::<Entity, With<DirectoryInput>>();
    let entities: Vec<_> = fields.iter(world).collect();
    for entity in entities {
        let value = WidgetryTextFieldHeadless::text(world, entity)
            .map_err(|failure| failure.into_bevy_error())?;
        if value != world.resource::<DirectoryState>().input {
            let mut state = world.resource_mut::<DirectoryState>();
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
