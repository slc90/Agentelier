use agentelier_agent_host::{
    AgentHost, HostFailed, ReceiveAgentEvents, SkillEntry, SkillRequest, SkillsEvent, UserSkillId,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::{
    prelude::*, text::LineBreak, ui::InteractionDisabled, ui_widgets::Activate,
    window::WindowCloseRequested,
};
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent,
};
use bevy_widgetry::list_view::{
    WidgetryListModel, WidgetryListView, WidgetryListViewAppExt, WidgetryListViewRenderer,
};
use bevy_widgetry::message_box::{
    WidgetryMessageBoxButtons, WidgetryMessageBoxPlugin, WidgetryMessageBoxResult,
    WidgetryMessageBoxResultEvent, widgetry_message_box,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::window::{
    WidgetryModalWindow, WidgetryWindowBackground, WidgetryWindowControlsConfig,
    WidgetryWindowTitle, owned_widgetry_window, widgetry_window_target,
};
use std::io;

#[derive(Component, Default, Clone)]
struct SkillsWindow;
#[derive(Component, Default, Clone)]
struct ImportPicker;
#[derive(Component, Default, Clone)]
struct StatusLabel;
#[derive(Component, Default, Clone)]
struct AddSkill;
#[derive(Component, Clone)]
struct DeleteSkill {
    id: UserSkillId,
    name: String,
}

#[derive(Resource)]
struct View {
    source: Entity,
    pending: Option<u64>,
    status: String,
}

pub(crate) fn install(app: &mut App) -> Result {
    if !app.is_plugin_added::<WidgetryMessageBoxPlugin>() {
        app.add_plugins(WidgetryMessageBoxPlugin);
    }
    app.register_widgetry_list_view::<Row>()
        .map_err(|failure| failure.into_bevy_error())?;
    let source = app
        .world_mut()
        .spawn(WidgetryListModel::<Row>::default())
        .id();
    app.insert_resource(View {
        source,
        pending: None,
        status: String::new(),
    })
    .add_observer(close_children)
    .add_systems(
        Update,
        (receive, refresh_controls)
            .chain()
            .after(ReceiveAgentEvents),
    );
    Ok(())
}

pub(crate) fn open(world: &mut World) -> Result {
    if world
        .query_filtered::<Entity, With<SkillsWindow>>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let source = world.resource::<View>().source;
    world
        .get_mut::<WidgetryListModel<Row>>(source)
        .ok_or_else(|| io::Error::other("Skill 列表未初始化"))?
        .clear();
    world
        .commands()
        .spawn_scene_with_error_handler(window(source));
    if world.resource::<View>().pending.is_none() {
        submit(world, SkillRequest::List);
    }
    Ok(())
}

fn submit(world: &mut World, request: SkillRequest) {
    let result = world.resource::<AgentHost>().manage_skills(request);
    let mut view = world.resource_mut::<View>();
    match result {
        Ok(operation) => {
            view.pending = Some(operation);
            view.status = "正在读取或更新 Skill 目录…".into();
        }
        Err(failure) => {
            view.pending = None;
            view.status = failure.to_string();
        }
    }
}

fn receive(
    mut events: MessageReader<SkillsEvent>,
    mut failures: MessageReader<HostFailed>,
    mut view: ResMut<View>,
    mut models: Query<&mut WidgetryListModel<Row>>,
) -> Result {
    for event in events.read() {
        if view.pending != Some(event.operation_id) {
            continue;
        }
        view.pending = None;
        match &event.result {
            Ok(snapshot) => {
                let mut model = models.get_mut(view.source)?;
                model.clear();
                for entry in &snapshot.entries {
                    model
                        .push(Row::from(entry))
                        .map_err(|failure| failure.into_bevy_error())?;
                }
                view.status = snapshot.diagnostics.first().cloned().unwrap_or_default();
                if snapshot.diagnostics.len() > 1 {
                    view.status
                        .push_str(&format!("（另有 {} 项）", snapshot.diagnostics.len() - 1));
                }
            }
            Err(failure) => view.status.clone_from(failure),
        }
    }
    for failure in failures.read() {
        if view.pending.take().is_some() {
            view.status.clone_from(&failure.reason);
        }
    }
    Ok(())
}

fn refresh_controls(
    view: Res<View>,
    mut labels: Query<&mut Text, With<StatusLabel>>,
    add: Query<(Entity, Has<InteractionDisabled>), With<AddSkill>>,
    mut commands: Commands,
) {
    if let Ok(mut text) = labels.single_mut()
        && text.0 != view.status
    {
        text.0.clone_from(&view.status);
    }
    if let Ok((button, disabled)) = add.single()
        && disabled != view.pending.is_some()
    {
        if disabled {
            commands.entity(button).remove::<InteractionDisabled>();
        } else {
            commands.entity(button).insert(InteractionDisabled);
        }
    }
}

struct Row {
    name: String,
    description: String,
    delete: Option<DeleteSkill>,
}
impl From<&SkillEntry> for Row {
    fn from(entry: &SkillEntry) -> Self {
        let name = entry.metadata.name.clone();
        Self {
            delete: entry.user.clone().map(|id| DeleteSkill {
                id,
                name: name.clone(),
            }),
            name,
            description: entry.metadata.description.clone(),
        }
    }
}
impl Row {
    fn content(&self) -> impl SceneList + use<> {
        let title = self.name.clone();
        let label = if self.delete.is_some() {
            "用户 Skill"
        } else {
            "内置 · 只读"
        };
        let entity = format!("SkillRow-{title}");
        let description = self.description.clone();
        let delete = self.delete.clone().map(|skill| {
            let entity = format!("DeleteSkill-{}", skill.name);
            bsn! {
                @button("删除") Name(entity)
                template(move |_| Ok(skill.clone()))
                on(confirm_delete)
            }
        });
        bsn_list! {
            Name(entity)
            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), row_gap: px(4), padding: UiRect::axes(px(10), px(6)) }
            Children [
                Node { width: percent(100), align_items: AlignItems::Center, column_gap: px(10) }
                Children [
                    Text(title) WidgetryText Node { flex_grow: 1.0, min_width: px(0) } TextLayout::no_wrap()--
                    Text(label) WidgetryText--
                    @delete
                ]--
                Text(description) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)
            ]
        }
    }
}

fn window(source: Entity) -> impl Scene {
    bsn! {
        SkillsWindow Name("SkillsWindow")
        @owned_widgetry_window(
            Window { title: "Skill 管理".into(), resolution: (840, 570).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                Node { width: percent(100), min_height: px(0), flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect::all(px(14)) }
                Children [
                    @button("添加 Skill") Name("AddUserSkill") AddSkill Node { align_self: AlignSelf::Start }
                    on(pick)--
                    @WidgetryListView::<Row> {
                        @source: source, @item_height: 128.0,
                        @renderer: {WidgetryListViewRenderer::new(Row::content)},
                    }
                    Name("SkillCatalog") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }--
                    StatusLabel Name("SkillManagementStatus") Text("") WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)
                ]
            },
        )
    }
}

fn button(label: &'static str) -> impl Scene {
    bsn! {
        @AgentelierButton { @style: AgentelierButtonStyle::Secondary, @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
        Node { height: px(34), flex_shrink: 0.0 }
    }
}

fn parent_window(world: &mut World) -> Result<Entity> {
    let root = world
        .query_filtered::<Entity, With<SkillsWindow>>()
        .single(world)?;
    widgetry_window_target(world, root).ok_or_else(|| io::Error::other("Skill 窗口未绑定").into())
}

fn pick(_: On<Activate>, mut commands: Commands) {
    commands.queue(|world: &mut World| -> Result {
        if world.resource::<View>().pending.is_some() || world.query_filtered::<Entity, With<ImportPicker>>().iter(world).next().is_some() {
            return Ok(());
        }
        let parent = parent_window(world)?;
        world.commands().spawn_scene_with_error_handler(bsn! {
            ImportPicker Name("SkillImportWindow")
            template(move |_| Ok(WidgetryModalWindow { parent }))
            @owned_widgetry_window(
                Window { title: "选择 Skill 文件夹".into(), resolution: (840, 580).into(), ..default() },
                WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
                WidgetryWindowBackground::Theme, Default::default(),
                bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
                bsn_list! {
                    @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory }
                    Name("SkillDirectoryPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                    on(picked)
                },
            )
        });
        Ok(())
    });
}

fn picked(event: On<WidgetryFileDialogResultEvent>, mut commands: Commands) {
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        let picker = world
            .query_filtered::<Entity, With<ImportPicker>>()
            .single(world)?;
        if let Some(window) = widgetry_window_target(world, picker) {
            world.write_message(WindowCloseRequested { window });
        }
        if let WidgetryFileDialogResult::Directory(path) = result {
            submit(world, SkillRequest::Import(path));
        }
        Ok(())
    });
}

fn confirm_delete(
    event: On<Activate>,
    buttons: Query<&DeleteSkill>,
    mut commands: Commands,
) -> Result {
    let choice = buttons.get(event.entity)?.clone();
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<View>().pending.is_some() {
            return Ok(());
        }
        let parent = parent_window(world)?;
        let prompt = format!("删除用户 Skill「{}」及其文件夹？", choice.name);
        world.commands().spawn_scene_with_error_handler(bsn! {
            Name("ConfirmSkillDeletion")
            @widgetry_message_box(parent, "确认删除 Skill", WidgetryMessageBoxButtons::YesNo, Default::default(),
                bsn_list! { Text(prompt) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) })
            template(move |_| Ok(choice.clone()))
            on(delete_confirmed)
        });
        Ok(())
    });
    Ok(())
}

fn delete_confirmed(
    event: On<WidgetryMessageBoxResultEvent>,
    choices: Query<&DeleteSkill>,
    mut commands: Commands,
) -> Result {
    if event.result == WidgetryMessageBoxResult::Yes {
        let id = choices.get(event.entity)?.id.clone();
        commands.queue(move |world: &mut World| submit(world, SkillRequest::Delete(id)));
    }
    Ok(())
}

fn close_children(
    _: On<Despawn<SkillsWindow>>,
    children: Query<Entity, With<ImportPicker>>,
    mut commands: Commands,
) {
    for child in &children {
        commands.entity(child).try_despawn();
    }
}
