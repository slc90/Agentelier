use agentelier_agent_host::AgentHost;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::{
    ArchiveEntry, ConversationRequest, SessionId, SessionMetadataChange,
};
use bevy::picking::events::PointerPress;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, OverrideClip};
use bevy::ui_widgets::{
    Activate,
    popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide},
};
use bevy_app_assets::SidebarIcon;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::style::z_index;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{WidgetryTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::theme::WidgetryTheme;

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub(crate) struct SessionActions {
    target: Option<SessionId>,
    title: String,
    view: Option<ActionView>,
    pending: Option<u64>,
    error: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq, Reflect)]
enum ActionView {
    Rename,
    Archive,
    Delete,
}

#[derive(Component, Clone, Default)]
pub(crate) struct SessionMenu;

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
    Edit(ActionView),
    ConfirmDelete,
    Save,
    Cancel,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SessionActions>()
        .register_type::<SessionActions>()
        .add_systems(Update, dismiss_menu)
        .add_systems(PostUpdate, update.before(bevy::ui::UiSystems::Content));
}

pub(crate) fn more(session: SessionId) -> impl Scene {
    let name = format!("SessionMore-{session}");
    bsn! {
        @crate::archives::reveal_button(SidebarIcon::More)
        Name(name) template(move |_| Ok(RenameTarget(session)))
        on(open_menu) on(crate::archives::block_row_click)
    }
}

fn open_menu(
    event: On<Activate>,
    targets: Query<&RenameTarget>,
    menus: Query<(Entity, &ChildOf), With<SessionMenu>>,
    state: Res<SessionActions>,
    theme: Res<WidgetryTheme>,
    mut commands: Commands,
) -> Result {
    if state.pending.is_some() {
        return Ok(());
    }
    let anchor = event.entity;
    let session = targets.get(anchor)?.0;
    for (menu, parent) in &menus {
        commands.entity(menu).try_despawn();
        if parent.parent() == anchor {
            return Ok(());
        }
    }
    let background = theme.window.frame.normal.background;
    let border = theme.window.frame.normal.border;
    commands.spawn_scene_with_error_handler(bsn! {
        SessionMenu Name("SessionActionsMenu") ChildOf(anchor) OverrideClip GlobalZIndex(z_index::POPUP)
        Popover {
            positions: {vec![
                PopoverPlacement { side: PopoverSide::Bottom, align: PopoverAlign::End, gap: 4.0 },
                PopoverPlacement { side: PopoverSide::Top, align: PopoverAlign::End, gap: 4.0 },
            ]},
            window_margin: 8.0,
        }
        BackgroundColor(background) BorderColor::all(border)
        Node { position_type: PositionType::Absolute, width: px(126), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::all(px(4)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)) }
        on(crate::archives::block_row_click)
        Children [
            @menu_item("重命名", "RenameSession", session, ActionView::Rename)--
            @menu_item("归档", "ArchiveSession", session, ActionView::Archive)--
            @menu_item("删除", "DeleteSession", session, ActionView::Delete)
        ]
    });
    Ok(())
}

fn menu_item(
    label: &'static str,
    name: &'static str,
    session: SessionId,
    view: ActionView,
) -> impl Scene {
    bsn! {
        @action(label, name, Action::Edit(view))
        template(move |_| Ok(RenameTarget(session)))
    }
}

fn dismiss_menu(
    mut presses: MessageReader<PointerPress>,
    menus: Query<(Entity, &ChildOf), With<SessionMenu>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for press in presses.read() {
        for (menu, parent) in &menus {
            let anchor = parent.parent();
            if press.entity != anchor
                && !parents
                    .iter_ancestors(press.entity)
                    .any(|entity| entity == anchor)
            {
                commands.entity(menu).try_despawn();
            }
        }
    }
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
                @action("保存名称", "SaveSessionName", Action::Save)
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
        Action::Edit(ActionView::Delete) | Action::ConfirmDelete => AgentelierButtonStyle::Danger,
        Action::Edit(_) | Action::Cancel => AgentelierButtonStyle::Secondary,
    };
    bsn! { @AgentelierButton { @style: style, @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} } Name(name) template(move |_| Ok(action)) on(activate) }
}
fn activate(
    event: On<Activate>,
    actions: Query<(&Action, Option<&RenameTarget>)>,
    menus: Query<Entity, With<SessionMenu>>,
    mut commands: Commands,
) -> Result {
    let (&action, target) = actions.get(event.entity)?;
    let session = target.map(|target| target.0);
    if matches!(action, Action::Edit(_)) {
        for menu in &menus {
            commands.entity(menu).try_despawn();
        }
    }
    commands.queue(move |world: &mut World| -> Result {
        let state = world.resource::<SessionActions>();
        if state.pending.is_some() {
            return Ok(());
        }
        match action {
            Action::Edit(view) => {
                let Some(session) = session else {
                    return Ok(());
                };
                edit_session(world, session, view)?;
            }
            Action::Cancel => *world.resource_mut::<SessionActions>() = default(),
            Action::Save | Action::ConfirmDelete => {
                if let Some(session) = state.target {
                    submit(world, session, action)?;
                }
            }
        }
        Ok(())
    });
    Ok(())
}

fn edit_session(world: &mut World, session: SessionId, view: ActionView) -> Result {
    let Some(title) = world
        .resource::<crate::ArchiveState>()
        .entries
        .iter()
        .find(|entry| entry.id == session && !entry.archived)
        .map(|entry| entry.title.clone())
    else {
        return Ok(());
    };
    *world.resource_mut::<SessionActions>() = SessionActions {
        target: Some(session),
        view: Some(view),
        title: title.clone(),
        ..default()
    };
    match view {
        ActionView::Rename => {
            let input = world
                .query_filtered::<Entity, With<RenameInput>>()
                .single(world)?;
            WidgetryTextFieldHeadless::set_text_in_world(world, input, &title)
                .map_err(|failure| failure.into_bevy_error())?;
        }
        ActionView::Archive => submit(world, session, Action::Edit(ActionView::Archive))?,
        ActionView::Delete => {}
    }
    Ok(())
}

fn submit(world: &mut World, session: SessionId, action: Action) -> Result {
    let result = match action {
        Action::Edit(ActionView::Archive) => world
            .resource::<AgentHost>()
            .change_session_metadata(session, SessionMetadataChange::Archive),
        Action::ConfirmDelete => world
            .resource::<AgentHost>()
            .request_conversation(ConversationRequest::Delete { session }),
        Action::Save => {
            let input = world
                .query_filtered::<Entity, With<RenameInput>>()
                .single(world)?;
            let title = WidgetryTextFieldHeadless::text(world, input)
                .map_err(|failure| failure.into_bevy_error())?;
            world
                .resource::<AgentHost>()
                .change_session_metadata(session, SessionMetadataChange::Rename(title))
        }
        _ => return Ok(()),
    };
    let mut state = world.resource_mut::<SessionActions>();
    match result {
        Ok(id) => {
            state.pending = Some(id);
            state.error = None;
        }
        Err(failure) => state.error = Some(failure.to_string()),
    }
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
            crate::archives::update_entry(world, entry)?;
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
    actions: Query<Entity, (With<Action>, Without<RenameTarget>)>,
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
            state.view == Some(ActionView::Rename)
        } else {
            state.view == Some(ActionView::Delete)
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
    if state.view != Some(ActionView::Delete)
        || state.target != Some(session)
        || state.pending != Some(operation)
    {
        return false;
    }
    *world.resource_mut::<SessionActions>() = default();
    true
}
pub(crate) fn rejected(world: &mut World, operation: u64, reason: &str) -> bool {
    let mut state = world.resource_mut::<SessionActions>();
    if state.view != Some(ActionView::Delete) || state.pending != Some(operation) {
        return false;
    }
    state.pending = None;
    state.error = Some(reason.into());
    true
}
