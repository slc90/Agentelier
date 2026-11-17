use agentelier_agent_host::AgentHost;
use agentelier_conversation_bridge::{ArchiveEntry, SessionId};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub(crate) struct SessionActions {
    target: Option<SessionId>,
    pending: Option<u64>,
    error: Option<String>,
}
#[derive(Component, Clone, Default)]
struct RenamePanel;
#[derive(Component, Clone, Default)]
struct RenameInput;
#[derive(Component, Clone, Default)]
struct RenameError;
#[derive(Component, Clone)]
struct RenameTarget(SessionId);
#[derive(Component, Clone, Copy)]
enum Action {
    Save,
    Cancel,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SessionActions>()
        .register_type::<SessionActions>()
        .add_systems(PostUpdate, update.before(bevy::ui::UiSystems::Content));
}

pub(crate) fn more(session: SessionId) -> impl Scene {
    let name = format!("SessionMore-{session}");
    bsn! {
        @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text("⋯") WidgetryText }) as Box<dyn SceneList>)} }
        Name(name) template(move |_| Ok(RenameTarget(session))) Node { flex_shrink: 0.0, height: px(32), width: px(32) }
        on(open) on(block_row_click)
    }
}
fn block_row_click(mut event: On<bevy::picking::events::PointerClick>) {
    event.propagate(false);
}

pub(crate) fn panel() -> impl Scene {
    bsn! {
        RenamePanel Name("SessionRenamePanel") Node { display: Display::None, width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
        Children [
            Text("重命名会话") WidgetryText--
            @WidgetryTextField { @value: {String::new()}, @multiline: false }
            RenameInput Name("SessionRenameInput") Node { width: percent(100) }--
            Text("") WidgetryText RenameError TextLayout::linebreak(LineBreak::AnyCharacter)--
            Node { column_gap: px(8) } Children [
                @action("保存名称", "SaveSessionName", Action::Save)--
                @action("取消", "CancelSessionRename", Action::Cancel)
            ]
        ]
    }
}
fn action(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    bsn! { @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} } Name(name) template(move |_| Ok(action)) on(activate) }
}
fn open(event: On<Activate>, targets: Query<&RenameTarget>, mut commands: Commands) -> Result {
    let session = targets.get(event.entity)?.0;
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<SessionActions>().pending.is_some() {
            return Ok(());
        }
        let Some(title) = world
            .resource::<crate::ArchiveState>()
            .entries
            .iter()
            .find(|entry| entry.id == session)
            .map(|entry| entry.title.clone())
        else {
            return Ok(());
        };
        let inputs = world
            .query_filtered::<Entity, With<RenameInput>>()
            .iter(world)
            .collect::<Vec<_>>();
        for input in inputs {
            WidgetryTextFieldHeadless::set_text_in_world(world, input, &title)
                .map_err(|failure| failure.into_bevy_error())?;
        }
        *world.resource_mut::<SessionActions>() = SessionActions {
            target: Some(session),
            ..default()
        };
        Ok(())
    });
    Ok(())
}
fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        let state = world.resource::<SessionActions>();
        if state.pending.is_some() {
            return Ok(());
        }
        let Some(session) = state.target else {
            return Ok(());
        };
        if matches!(action, Action::Cancel) {
            *world.resource_mut::<SessionActions>() = default();
            return Ok(());
        }
        let input = world
            .query_filtered::<Entity, With<RenameInput>>()
            .single(world)?;
        let title = WidgetryTextFieldHeadless::text(world, input)
            .map_err(|failure| failure.into_bevy_error())?;
        match world.resource::<AgentHost>().rename_session(session, title) {
            Ok(id) => {
                let mut state = world.resource_mut::<SessionActions>();
                state.pending = Some(id);
                state.error = None;
                tracing::info!(operation_id = id, session_id = %session, "开始提交会话名称");
            }
            Err(failure) => {
                world.resource_mut::<SessionActions>().error = Some(failure.to_string())
            }
        }
        Ok(())
    });
    Ok(())
}
pub(crate) fn complete(
    world: &mut World,
    operation: u64,
    session: SessionId,
    result: Result<ArchiveEntry, String>,
) -> Result {
    if world.resource::<SessionActions>().target != Some(session)
        || world.resource::<SessionActions>().pending != Some(operation)
    {
        return Ok(());
    }
    match result {
        Ok(entry) => {
            crate::archives::rename_entry(world, entry)?;
            *world.resource_mut::<SessionActions>() = default();
        }
        Err(failure) => {
            let mut state = world.resource_mut::<SessionActions>();
            state.pending = None;
            state.error = Some(failure);
        }
    }
    Ok(())
}
fn update(
    state: Res<SessionActions>,
    mut panels: Query<&mut Node, With<RenamePanel>>,
    inputs: Query<Entity, With<RenameInput>>,
    actions: Query<Entity, With<Action>>,
    mut texts: Query<&mut Text, With<RenameError>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for mut panel in &mut panels {
        panel.display = if state.target.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for entity in inputs.iter().chain(actions.iter()) {
        if state.pending.is_some() {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
    for mut text in &mut texts {
        text.set_if_neq(Text::new(state.error.clone().unwrap_or_default()));
    }
}

pub(crate) fn failed(world: &mut World, reason: &str) {
    let mut state = world.resource_mut::<SessionActions>();
    if state.pending.take().is_some() {
        state.error = Some(reason.into());
    }
}
