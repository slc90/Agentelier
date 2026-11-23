use agentelier_agent_host::AgentHost;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::{ArchiveEntry, SessionId};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub(crate) struct SessionActions {
    target: Option<SessionId>,
    title: String,
    deleting: bool,
    pending: Option<u64>,
    error: Option<String>,
}
#[derive(Component, Clone, Default)]
struct RenamePanel;
#[derive(Component, Clone, Default)]
struct RenameInput;
#[derive(Component, Clone, Default)]
struct RenameControls;
#[derive(Component, Clone, Default)]
struct DeleteControls;
#[derive(Component, Clone, Default)]
struct DeletePrompt;
#[derive(Component, Clone, Default)]
struct RenameError;
#[derive(Component, Clone)]
struct RenameTarget(SessionId);
#[derive(Component, Clone, Copy)]
enum Action {
    Delete,
    ConfirmDelete,
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
        @AgentelierButton { @style: AgentelierButtonStyle::Icon, @children: {Some(Box::new(bsn_list! { Text("⋯") WidgetryText }) as Box<dyn SceneList>)} }
        Name(name) template(move |_| Ok(RenameTarget(session))) Node { align_self: AlignSelf::Center }
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
            RenameControls Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8) }
            Children [
                Text("重命名会话") WidgetryText--
                @WidgetryTextField { @value: {String::new()}, @multiline: false }
                RenameInput Name("SessionRenameInput") Node { width: percent(100) }--
                @action("保存名称", "SaveSessionName", Action::Save)--
                @action("删除会话", "DeleteSession", Action::Delete)
            ]--
            DeleteControls Node { display: Display::None, width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8) }
            Children [
                Text("") WidgetryText DeletePrompt Name("SessionDeletePrompt") TextLayout::linebreak(LineBreak::AnyCharacter)--
                @action("确认删除", "ConfirmSessionDelete", Action::ConfirmDelete)
            ]--
            Text("") WidgetryText RenameError TextLayout::linebreak(LineBreak::AnyCharacter)--
            @action("取消", "CancelSessionRename", Action::Cancel)
        ]
    }
}
fn action(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    let style = match action {
        Action::Save => AgentelierButtonStyle::Primary,
        Action::Delete | Action::ConfirmDelete => AgentelierButtonStyle::Danger,
        Action::Cancel => AgentelierButtonStyle::Secondary,
    };
    bsn! { @AgentelierButton { @style: style, @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} } Name(name) template(move |_| Ok(action)) on(activate) }
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
            title,
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
        if matches!(action, Action::Delete) {
            let mut state = world.resource_mut::<SessionActions>();
            state.deleting = true;
            state.error = None;
            return Ok(());
        }
        if matches!(action, Action::ConfirmDelete) {
            match world.resource::<AgentHost>().request_conversation(
                agentelier_conversation_bridge::ConversationRequest::Delete { session },
            ) {
                Ok(id) => {
                    let mut state = world.resource_mut::<SessionActions>();
                    state.pending = Some(id);
                    state.error = None;
                    tracing::info!(operation_id = id, session_id = %session, "开始删除本地会话");
                }
                Err(failure) => {
                    world.resource_mut::<SessionActions>().error =
                        Some(format!("会话未删除：{failure}"))
                }
            }
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
    mut panels: Query<
        (&mut Node, Has<RenamePanel>, Has<RenameControls>),
        Or<(
            With<RenamePanel>,
            With<RenameControls>,
            With<DeleteControls>,
        )>,
    >,
    inputs: Query<Entity, With<RenameInput>>,
    actions: Query<Entity, With<Action>>,
    mut texts: Query<&mut Text, (With<RenameError>, Without<DeletePrompt>)>,
    mut prompts: Query<&mut Text, (With<DeletePrompt>, Without<RenameError>)>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for (mut panel, root, rename) in &mut panels {
        let visible = if root {
            state.target.is_some()
        } else if rename {
            !state.deleting
        } else {
            state.deleting
        };
        panel.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
    for mut text in &mut prompts {
        text.set_if_neq(Text::new(format!(
            "确定删除“{}”？只删除此会话的本地记录和附件，不撤销已修改的项目文件。",
            state.title
        )));
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

pub(crate) fn deleted(world: &mut World, operation: u64, session: SessionId) -> bool {
    let state = world.resource::<SessionActions>();
    if !state.deleting || state.target != Some(session) || state.pending != Some(operation) {
        return false;
    }
    *world.resource_mut::<SessionActions>() = default();
    true
}
pub(crate) fn rejected(world: &mut World, operation: u64, reason: &str) -> bool {
    let mut state = world.resource_mut::<SessionActions>();
    if !state.deleting || state.pending != Some(operation) {
        return false;
    }
    state.pending = None;
    state.error = Some(reason.into());
    true
}
