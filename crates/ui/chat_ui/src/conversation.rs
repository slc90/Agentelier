use crate::preparation::ModelState;
use agentelier_agent_host::{
    AgentHost, ConversationEvent as HostConversationEvent, HostActivity, HostFailed,
    ReceiveAgentEvents,
};
use agentelier_conversation_bridge::{
    ConversationEvent, ConversationRequest, SessionSettings, SessionView, TurnStatus,
};
use agentelier_login_ui::{AccountState, CredentialState};
use agentelier_markdown::{MarkdownSource, MarkdownView};
use agentelier_prompts::InstructionSnapshot;
use bevy::ecs::message::MessageCursor;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input_focus::{InputFocus, InputFocusSystems};
use bevy::prelude::*;
use bevy::text::{EditableTextSystems, LineBreak};
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy::window::Ime;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryTextField, WidgetryTextFieldHeadless};
use std::io;
use std::sync::Arc;
use tracing::{error, info};

const CONTENT_GAP: f32 = 12.0;
const BUTTON_HEIGHT: f32 = 40.0;
const COMPOSER_MIN_ROWS: usize = 2;
const COMPOSER_MAX_ROWS: usize = 6;

#[derive(Clone, Reflect)]
pub struct ChatMessage {
    pub id: String,
    pub output_index: usize,
    pub phase: Option<String>,
    pub text: String,
}
#[derive(Clone, Reflect)]
pub enum ChatTurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
}
#[derive(Clone, Reflect)]
pub enum ChatHistoryItem {
    User,
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone, Reflect)]
pub struct ChatTurn {
    pub id: u64,
    pub operation_id: u64,
    pub input_id: u64,
    pub is_retry: bool,
    pub input: String,
    pub messages: Vec<ChatMessage>,
    pub status: ChatTurnStatus,
    pub request_items: Vec<ChatHistoryItem>,
    pub request_instructions: String,
}
#[derive(Clone, Reflect)]
pub struct ChatSession {
    pub id: u64,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub account_subject: String,
    pub instruction_snapshot: String,
    pub turn: Option<ChatTurn>,
    pub previous_turns: Vec<ChatTurn>,
    pub committed_items: Vec<ChatHistoryItem>,
}
#[derive(Clone, Reflect)]
pub struct ChatReplacement {
    pub reset_session: Option<u64>,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub account_subject: String,
    #[reflect(ignore)]
    pub instructions: Arc<InstructionSnapshot>,
}
#[derive(Resource, Clone, Reflect, Default)]
#[reflect(Resource)]
pub struct ChatState {
    pub session: Option<ChatSession>,
    pub operation: Option<u64>,
    pub cancelling: bool,
    pub replacement: Option<ChatReplacement>,
    pub draft: String,
    pub error: Option<String>,
}
impl ChatState {
    fn can_send(
        &self,
        model: &ModelState,
        account: &AccountState,
        activity: &HostActivity,
    ) -> bool {
        self.operation.is_none()
            && self.replacement.is_none()
            && !activity.model_busy
            && model.can_request(account)
            && !self.draft.trim().is_empty()
            && self.session.as_ref().is_some_and(|session| {
                session
                    .turn
                    .as_ref()
                    .is_none_or(|turn| !matches!(turn.status, ChatTurnStatus::Running))
                    && account
                        .account
                        .as_ref()
                        .is_some_and(|account| account.subject == session.account_subject)
            })
    }
}

#[derive(Resource, Default)]
struct InputIntents {
    send: bool,
    clear_accepted: Option<String>,
}
#[derive(Component, Clone, Default)]
struct Composer;
#[derive(Component, Clone, Default)]
struct MessageList;
#[derive(Component, Clone, Default)]
struct SessionSummary;
#[derive(Component, Clone, Default)]
struct MessageLabel;
#[derive(Component, Clone, Default)]
struct ChatStatus;
#[derive(Component, Clone, Default)]
struct ReplacementPrompt;
#[derive(Component, Clone, Copy)]
enum ChatAction {
    Create,
    Send,
    Stop,
    Retry,
    Reset,
    ConfirmReplacement,
    KeepSession,
}
#[derive(Component, Clone, PartialEq, Eq)]
struct ChatRow {
    session: u64,
    turn: u64,
    output_index: Option<usize>,
    item_id: Option<String>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ChatState>()
        .init_resource::<InputIntents>()
        .register_type::<ChatState>()
        .add_observer(mount_messages)
        .add_systems(
            PreUpdate,
            capture_enter
                .after(bevy::input::InputSystems)
                .before(InputFocusSystems::Dispatch),
        )
        .add_systems(
            Update,
            receive
                .after(ReceiveAgentEvents)
                .after(crate::preparation::receive),
        )
        .add_systems(
            PostUpdate,
            process_input
                .after(EditableTextSystems)
                .before(UiSystems::PostLayout),
        )
        .add_systems(PostUpdate, update.before(UiSystems::Content));
}

pub(crate) fn panel(state: &ChatState) -> impl Scene {
    let draft = state.draft.clone();
    let summary = summary(state);
    let status = status(state);
    bsn! {
        Name("ChatPanel")
        Node { width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(CONTENT_GAP) }
        Children [
            Node { column_gap: px(CONTENT_GAP), align_items: AlignItems::Center, flex_shrink: 0.0 }
            Children [
                @button("新建会话", "CreateChatSession", ChatAction::Create)--
                @button("重置当前会话", "ResetChatSession", ChatAction::Reset)--
                Text(summary) WidgetryText SessionSummary Name("ChatSessionSummary")
            ]--
            Node { column_gap: px(CONTENT_GAP), flex_shrink: 0.0, display: Display::None } ReplacementPrompt
            Children [
                Text("当前对话仅在内存中，确认后将清除全部记录和上下文；输入草稿保留。") WidgetryText--
                @button("确认清空", "ConfirmChatReplacement", ChatAction::ConfirmReplacement)--
                @button("保留当前会话", "KeepChatSession", ChatAction::KeepSession)
            ]--
            Text(status) WidgetryText ChatStatus Name("ChatStatus")--
            @WidgetryScrollArea {
                @children: {Some(Box::new(bsn_list! {
                    MessageList Name("ChatMessages")
                    Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(CONTENT_GAP) }
                }) as Box<dyn SceneList>)}
            }
            Name("ChatMessageScroll") crate::reading::ChatReadingScroll Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }--
            @WidgetryTextField {
                @value: {draft}, @multiline: true,
                @auto_size: {Some(AutoSize { min_rows: COMPOSER_MIN_ROWS, max_rows: COMPOSER_MAX_ROWS })},
                @hint_text: {Some("输入消息；Enter 发送，Shift+Enter 换行".into())},
            }
            Composer Name("ChatComposer") Node { width: percent(100), flex_shrink: 0.0 }--
            Node { column_gap: px(CONTENT_GAP), flex_shrink: 0.0 }
            Children [
                @button("发送", "SendChatMessage", ChatAction::Send)--
                @button("停止", "StopChatGeneration", ChatAction::Stop)--
                @button("重试最近尝试", "RetryChatTurn", ChatAction::Retry)--
                @crate::reading::bottom_button()
            ]
        ]
    }
}

fn button(label: &'static str, name: &'static str, action: ChatAction) -> impl Scene {
    bsn! {
        @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
        Name(name) template(move |_| Ok(action))
        Node { min_width: px(128), height: px(BUTTON_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0 }
        on(activate)
    }
}

fn activate(
    event: On<Activate>,
    actions: Query<&ChatAction>,
    mut intents: ResMut<InputIntents>,
    mut commands: Commands,
) -> Result {
    match actions.get(event.entity)? {
        ChatAction::Create => commands.queue(create_session),
        ChatAction::Send => intents.send = true,
        ChatAction::Stop => commands.queue(stop),
        ChatAction::Retry => commands.queue(retry),
        ChatAction::Reset => commands.queue(prepare_reset),
        ChatAction::ConfirmReplacement => commands.queue(confirm_replacement),
        ChatAction::KeepSession => commands.queue(|world: &mut World| {
            world.resource_mut::<ChatState>().replacement = None;
        }),
    }
    Ok(())
}

fn create_session(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    let model = world.resource::<ModelState>();
    let account = world.resource::<AccountState>();
    if state.operation.is_some()
        || state.replacement.is_some()
        || !model.can_verify(account)
        || world.resource::<AgentHost>().model_busy()?
    {
        return Ok(());
    }
    let choice = model
        .selected_model()
        .ok_or_else(|| io::Error::other("聊天准备区缺少所选模型"))?;
    let subject = account
        .account
        .as_ref()
        .ok_or_else(|| io::Error::other("创建会话缺少当前账号"))?
        .subject
        .clone();
    let settings = SessionSettings {
        slug: choice.slug.clone(),
        display_name: choice.display_name.clone(),
        effort: model.reasoning.effort(),
        account_subject: subject,
        instructions: Arc::new(crate::prompts::snapshot(world)?),
    };
    if state.session.is_some() {
        world.resource_mut::<ChatState>().replacement = Some(ChatReplacement {
            reset_session: None,
            slug: settings.slug,
            display_name: settings.display_name,
            effort: settings.effort,
            account_subject: settings.account_subject,
            instructions: settings.instructions,
        });
        return Ok(());
    }
    let result = world
        .resource::<AgentHost>()
        .request_conversation(ConversationRequest::Create(settings));
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
            info!(operation_id = id, "开始创建聊天会话");
        }
        Err(failure) => {
            error!(error = %failure, "无法创建聊天会话");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn prepare_reset(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if state.operation.is_some()
        || state.replacement.is_some()
        || world.resource::<AgentHost>().model_busy()?
    {
        return Ok(());
    }
    let Some(session) = &state.session else {
        return Ok(());
    };
    let pending = ChatReplacement {
        reset_session: Some(session.id),
        slug: session.slug.clone(),
        display_name: session.display_name.clone(),
        effort: session.effort.clone(),
        account_subject: session.account_subject.clone(),
        instructions: Arc::new(serde_json::from_str(&session.instruction_snapshot)?),
    };
    world.resource_mut::<ChatState>().replacement = Some(pending);
    Ok(())
}
fn confirm_replacement(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if state.operation.is_some() || world.resource::<AgentHost>().model_busy()? {
        return Ok(());
    }
    let Some(pending) = state.replacement.clone() else {
        return Ok(());
    };
    let account = world.resource::<AccountState>();
    if !world.resource::<ModelState>().can_request(account)
        || account
            .account
            .as_ref()
            .is_none_or(|account| account.subject != pending.account_subject)
    {
        return Ok(());
    }
    let request = match pending.reset_session {
        Some(session) => ConversationRequest::Reset { session },
        None => ConversationRequest::Create(SessionSettings {
            slug: pending.slug,
            display_name: pending.display_name,
            effort: pending.effort,
            account_subject: pending.account_subject,
            instructions: pending.instructions,
        }),
    };
    let result = world.resource::<AgentHost>().request_conversation(request);
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
        }
        Err(failure) => {
            error!(error=%failure, "替换聊天会话失败");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn can_retry(
    state: &ChatState,
    model: &ModelState,
    account: &AccountState,
    activity: &HostActivity,
) -> bool {
    state.operation.is_none()
        && state.replacement.is_none()
        && !activity.model_busy
        && model.can_request(account)
        && state.session.as_ref().is_some_and(|session| {
            account
                .account
                .as_ref()
                .is_some_and(|account| account.subject == session.account_subject)
                && session.turn.as_ref().is_some_and(|turn| {
                    matches!(
                        turn.status,
                        ChatTurnStatus::Failed(_) | ChatTurnStatus::Cancelled
                    )
                })
        })
}
fn retry(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if !can_retry(
        state,
        world.resource::<ModelState>(),
        world.resource::<AccountState>(),
        world.resource::<HostActivity>(),
    ) || world.resource::<AgentHost>().model_busy()?
    {
        return Ok(());
    }
    let Some(session) = &state.session else {
        return Ok(());
    };
    let Some(turn) = &session.turn else {
        return Ok(());
    };
    let result = world
        .resource::<AgentHost>()
        .request_conversation(ConversationRequest::Retry {
            session: session.id,
            turn: turn.id,
        });
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
            info!(operation_id = id, "提交聊天重试");
        }
        Err(failure) => {
            error!(error=%failure, "聊天重试提交失败");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn stop(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if state.cancelling {
        return Ok(());
    }
    let Some(session) = &state.session else {
        return Ok(());
    };
    let Some(turn) = &session.turn else {
        return Ok(());
    };
    if state.operation != Some(turn.operation_id) || !matches!(turn.status, ChatTurnStatus::Running)
    {
        return Ok(());
    }
    let (session, turn, operation) = (session.id, turn.id, turn.operation_id);
    match world
        .resource::<AgentHost>()
        .cancel_conversation(session, turn, operation)
    {
        Ok(true) => {
            world.resource_mut::<ChatState>().cancelling = true;
            info!(
                session_id = session,
                turn_id = turn,
                operation_id = operation,
                "已请求停止聊天，等待任务回收"
            );
        }
        Ok(false) => {}
        Err(failure) => {
            error!(error=%failure, "停止聊天请求失败");
            world.resource_mut::<ChatState>().error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn send(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if !state.can_send(
        world.resource::<ModelState>(),
        world.resource::<AccountState>(),
        world.resource::<HostActivity>(),
    ) || world.resource::<AgentHost>().model_busy()?
    {
        return Ok(());
    }
    let session = state
        .session
        .as_ref()
        .ok_or_else(|| io::Error::other("发送消息缺少聊天会话"))?
        .id;
    let input = state.draft.clone();
    let result = world
        .resource::<AgentHost>()
        .request_conversation(ConversationRequest::Send { session, input });
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
            info!(operation_id = id, session_id = session, "开始提交聊天消息");
        }
        Err(failure) => {
            error!(error = %failure, "无法提交聊天消息");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn capture_enter(
    world: &mut World,
    mut events: Local<MessageCursor<KeyboardInput>>,
    mut ime: Local<MessageCursor<Ime>>,
) -> Result {
    let ime_active = ime.read(world.resource::<Messages<Ime>>()).any(|event| {
        matches!(event, Ime::Commit { .. })
            || matches!(event, Ime::Preedit { value, .. } if !value.is_empty())
    });
    let focus = world.resource::<InputFocus>().get();
    let mut fields = world.query_filtered::<(Entity, &WidgetryTextFieldHeadless), With<Composer>>();
    let root = fields
        .iter(world)
        .find(|(_, field)| Some(field.editor) == focus)
        .map(|(entity, _)| entity);
    let composing = if let Some(root) = root {
        WidgetryTextFieldHeadless::editor(world, root)
            .map_err(|failure| failure.into_bevy_error())?
            .is_composing()
    } else {
        false
    };
    let pressed = world.resource::<ButtonInput<KeyCode>>();
    let shift = pressed.pressed(KeyCode::ShiftLeft) || pressed.pressed(KeyCode::ShiftRight);
    let shortcut = [
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]
    .iter()
    .any(|key| pressed.pressed(*key));
    let mut send = false;
    for event in events.read_mut(&mut world.resource_mut::<Messages<KeyboardInput>>()) {
        if root.is_none()
            || ime_active
            || composing
            || shortcut
            || !matches!(event.key_code, KeyCode::Enter | KeyCode::NumpadEnter)
        {
            continue;
        }
        // Remap before focused dispatch, retaining Bevy's normal selection and text-edit path.
        if shift {
            event.logical_key = Key::Character("\n".into());
            event.text = (event.state == ButtonState::Pressed).then(|| "\n".into());
        } else {
            event.logical_key = Key::Unidentified(NativeKey::Unidentified);
            event.text = None;
            send |= event.state == ButtonState::Pressed && !event.repeat;
        }
    }
    world.resource_mut::<InputIntents>().send |= send;
    Ok(())
}

fn process_input(world: &mut World) -> Result {
    let clear = world.resource_mut::<InputIntents>().clear_accepted.take();
    let requested = world.resource::<InputIntents>().send;
    let mut fields = world.query_filtered::<Entity, With<Composer>>();
    let roots: Vec<_> = fields.iter(world).collect();
    for root in roots {
        let editor = WidgetryTextFieldHeadless::editor_entity(world, root)
            .map_err(|failure| failure.into_bevy_error())?;
        let mut editors = world.query::<Ref<bevy::text::EditableText>>();
        let changed = editors.get(world, editor)?.is_changed();
        if !changed && clear.is_none() && !requested {
            continue;
        }
        let mut value = WidgetryTextFieldHeadless::text(world, root)
            .map_err(|failure| failure.into_bevy_error())?;
        if clear.as_ref() == Some(&value) {
            WidgetryTextFieldHeadless::set_text_in_world(world, root, "")
                .map_err(|failure| failure.into_bevy_error())?;
            value.clear();
        }
        if world.resource::<ChatState>().draft != value {
            world.resource_mut::<ChatState>().draft = value;
        }
    }
    if std::mem::take(&mut world.resource_mut::<InputIntents>().send) {
        send(world)?;
    }
    Ok(())
}

fn history_item(item: agentelier_conversation_bridge::HistoryItem) -> ChatHistoryItem {
    match item {
        agentelier_conversation_bridge::HistoryItem::User => ChatHistoryItem::User,
        agentelier_conversation_bridge::HistoryItem::Assistant { phase } => {
            ChatHistoryItem::Assistant { phase }
        }
        agentelier_conversation_bridge::HistoryItem::Reasoning { encrypted } => {
            ChatHistoryItem::Reasoning { encrypted }
        }
    }
}
fn project_turn(turn: agentelier_conversation_bridge::TurnView) -> Result<ChatTurn> {
    Ok(ChatTurn {
        id: turn.id,
        input_id: turn.input_id,
        is_retry: turn.is_retry,
        operation_id: turn.operation_id,
        input: turn.input,
        messages: turn
            .messages
            .into_iter()
            .map(|message| ChatMessage {
                id: message.id,
                output_index: message.output_index,
                phase: message.phase,
                text: message.text,
            })
            .collect(),
        status: match turn.status {
            TurnStatus::Running => ChatTurnStatus::Running,
            TurnStatus::Completed => ChatTurnStatus::Completed,
            TurnStatus::Cancelled => ChatTurnStatus::Cancelled,
            TurnStatus::Failed(reason) => ChatTurnStatus::Failed(reason),
        },
        request_items: turn.request_items.into_iter().map(history_item).collect(),
        request_instructions: serde_json::to_string_pretty(&turn.request_instructions)?,
    })
}
fn project(session: SessionView) -> Result<ChatSession> {
    Ok(ChatSession {
        id: session.id,
        slug: session.settings.slug,
        display_name: session.settings.display_name,
        effort: session.settings.effort,
        account_subject: session.settings.account_subject,
        instruction_snapshot: serde_json::to_string_pretty(session.settings.instructions.as_ref())?,
        turn: session.turn.map(project_turn).transpose()?,
        previous_turns: session
            .previous_turns
            .into_iter()
            .map(project_turn)
            .collect::<Result<Vec<_>>>()?,
        committed_items: session
            .committed_items
            .into_iter()
            .map(history_item)
            .collect(),
    })
}

fn receive(world: &mut World, mut failures: Local<MessageCursor<HostFailed>>) -> Result {
    let account = world.resource::<AccountState>();
    let invalid = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .is_some_and(|session| {
            account
                .account
                .as_ref()
                .is_none_or(|account| account.subject != session.account_subject)
                || matches!(
                    account.credentials,
                    CredentialState::SignedOut
                        | CredentialState::NoRecord
                        | CredentialState::HostOnly
                )
        });
    let mut changed = invalid;
    if invalid {
        *world.resource_mut::<ChatState>() = ChatState {
            draft: world.resource::<ChatState>().draft.clone(),
            ..default()
        };
    }
    let events: Vec<_> = world
        .resource_mut::<Messages<HostConversationEvent>>()
        .drain()
        .collect();
    for event in events {
        if world.resource::<ChatState>().operation != Some(event.operation_id) {
            continue;
        }
        changed = true;
        let terminal = event.kind.is_terminal();
        match &event.kind {
            ConversationEvent::Created(view) => info!(
                operation_id = event.operation_id,
                session_id = view.id,
                "聊天会话已创建"
            ),
            ConversationEvent::Accepted(view) => info!(
                operation_id = event.operation_id,
                session_id = view.id,
                "聊天消息已接纳"
            ),
            ConversationEvent::Finished(view) => {
                if let Some(turn) = &view.turn {
                    match &turn.status {
                        TurnStatus::Completed => info!(
                            operation_id = event.operation_id,
                            session_id = view.id,
                            turn_id = turn.id,
                            "聊天轮次已完成并回收"
                        ),
                        TurnStatus::Cancelled => info!(
                            operation_id = event.operation_id,
                            session_id = view.id,
                            turn_id = turn.id,
                            "聊天轮次已实际取消"
                        ),
                        TurnStatus::Failed(reason) => error!(
                            operation_id = event.operation_id,
                            session_id = view.id,
                            turn_id = turn.id,
                            error = reason,
                            "聊天轮次失败，保留部分正文"
                        ),
                        TurnStatus::Running => {
                            return Err(io::Error::other("聊天终态仍标记为生成中").into());
                        }
                    }
                }
            }
            ConversationEvent::Rejected(reason) => error!(
                operation_id = event.operation_id,
                error = reason,
                "聊天操作未被接纳"
            ),
            ConversationEvent::Updated(_) => {}
        }
        match event.kind {
            ConversationEvent::Rejected(reason) => {
                world.resource_mut::<ChatState>().error = Some(reason);
            }
            ConversationEvent::Created(view) => {
                let mut state = world.resource_mut::<ChatState>();
                state.session = Some(project(view)?);
                state.replacement = None;
            }
            ConversationEvent::Updated(view) | ConversationEvent::Finished(view) => {
                world.resource_mut::<ChatState>().session = Some(project(view)?);
            }
            ConversationEvent::Accepted(view) => {
                world.resource_mut::<InputIntents>().clear_accepted = view
                    .turn
                    .as_ref()
                    .filter(|turn| !turn.is_retry)
                    .map(|turn| turn.input.clone());
                world.resource_mut::<ChatState>().session = Some(project(view)?);
            }
        }
        if terminal {
            world.resource_mut::<ChatState>().cancelling = false;
            world.resource_mut::<ChatState>().operation = None;
        }
    }
    if let Some(reason) = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|failure| failure.reason.clone())
    {
        let mut state = world.resource_mut::<ChatState>();
        state.operation = None;
        state.cancelling = false;
        state.error = Some(reason);
        changed = true;
    }
    if changed {
        reconcile(world)?;
    }
    Ok(())
}

fn mount_messages(_event: On<Add<MessageList>>, mut commands: Commands) {
    commands.queue(reconcile);
}

fn reconcile(world: &mut World) -> Result {
    let session = world.resource::<ChatState>().session.clone();
    let mut lists = world.query_filtered::<Entity, With<MessageList>>();
    let lists: Vec<_> = lists.iter(world).collect();
    for list in lists {
        let mut rows = world.query::<(Entity, &ChatRow, &ChildOf)>();
        let existing: Vec<_> = rows
            .iter(world)
            .filter(|(_, _, parent)| parent.parent() == list)
            .map(|(entity, row, _)| (entity, row.clone()))
            .collect();
        let mut desired = Vec::new();
        if let Some(session) = &session {
            for turn in session.previous_turns.iter().chain(session.turn.iter()) {
                let status = match &turn.status {
                    ChatTurnStatus::Cancelled => " · 已取消，未进入上下文",
                    ChatTurnStatus::Failed(_) => " · 失败，未进入上下文",
                    _ => "",
                };
                let user_key = ChatRow {
                    session: session.id,
                    turn: turn.input_id,
                    output_index: None,
                    item_id: None,
                };
                if !desired.iter().any(|(key, _, _)| key == &user_key) {
                    desired.push((user_key, format!("你\n{}", turn.input), None));
                }
                if turn.messages.is_empty() {
                    desired.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            output_index: None,
                            item_id: Some("attempt".into()),
                        },
                        format!(
                            "助手 · 尝试 {}{}{}",
                            turn.id,
                            if turn.is_retry { " · 重试" } else { "" },
                            status
                        ),
                        None,
                    ));
                }
                for message in &turn.messages {
                    let phase = message.phase.as_deref().unwrap_or("回答");
                    desired.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            output_index: Some(message.output_index),
                            item_id: Some(message.id.clone()),
                        },
                        format!("助手 · {phase}{status}"),
                        Some(message.text.clone()),
                    ));
                }
            }
        }
        for (entity, key) in &existing {
            if !desired.iter().any(|(wanted, _, _)| wanted == key) {
                world.entity_mut(*entity).despawn();
            }
        }
        for (key, text, markdown) in desired {
            if let Some((entity, _)) = existing.iter().find(|(_, row)| *row == key) {
                if let Some(markdown) = markdown {
                    let children = world
                        .get::<Children>(*entity)
                        .ok_or_else(|| io::Error::other("助手消息缺少内容区域"))?
                        .to_vec();
                    for child in children {
                        if let Some(mut source) = world.get_mut::<MarkdownSource>(child) {
                            source.set_if_neq(MarkdownSource(markdown.clone()));
                        }
                        if world.get::<MessageLabel>(child).is_some()
                            && let Some(mut label) = world.get_mut::<Text>(child)
                        {
                            label.set_if_neq(Text(text.clone()));
                        }
                    }
                } else {
                    world
                        .get_mut::<Text>(*entity)
                        .ok_or_else(|| io::Error::other("聊天消息缺少正文组件"))?
                        .set_if_neq(Text::new(text));
                }
            } else {
                let name = format!(
                    "ChatMessage-{}-{}-{}",
                    key.session,
                    key.turn,
                    key.output_index
                        .map(|index| index.to_string())
                        .unwrap_or_else(|| key.item_id.clone().unwrap_or_else(|| "user".into()))
                );
                if let Some(source) = markdown {
                    world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                        Node { width: percent(100), min_width: px(0), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(6) }
                        Children [Text(text) WidgetryText MessageLabel-- @MarkdownView { @source: source }]
                    });
                } else {
                    world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) Text(text) WidgetryText
                        template(move |_| Ok(key.clone()))
                        TextLayout::linebreak(LineBreak::AnyCharacter)
                        Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
                    });
                }
            }
        }
    }
    Ok(())
}

fn summary(state: &ChatState) -> String {
    state
        .session
        .as_ref()
        .map(|session| {
            format!(
                "{} · {} · 推理：{}",
                session.display_name,
                session.slug,
                session.effort.as_deref().unwrap_or("服务默认")
            )
        })
        .unwrap_or_else(|| "选择模型与推理设置后创建会话".into())
}
fn status(state: &ChatState) -> String {
    if state.cancelling {
        return "正在停止…等待任务结束与回收".into();
    }
    if let Some(error) = &state.error {
        return error.clone();
    }
    if let Some(turn) = state
        .session
        .as_ref()
        .and_then(|session| session.turn.as_ref())
    {
        return match &turn.status {
            ChatTurnStatus::Running => "正在生成…".into(),
            ChatTurnStatus::Completed => "本轮已完成".into(),
            ChatTurnStatus::Cancelled => "本轮已取消；部分回复未进入上下文".into(),
            ChatTurnStatus::Failed(reason) => format!("本轮失败；部分回复未进入上下文：{reason}"),
        };
    }
    if state.operation.is_some() {
        "正在接纳操作…".into()
    } else {
        String::new()
    }
}

fn update(
    state: Res<ChatState>,
    model: Res<ModelState>,
    account: Res<AccountState>,
    activity: Res<HostActivity>,
    buttons: Query<(Entity, &ChatAction)>,
    mut texts: Query<(&mut Text, Has<SessionSummary>, Has<ChatStatus>)>,
    mut prompts: Query<&mut Node, With<ReplacementPrompt>>,
    mut commands: Commands,
) {
    if !state.is_changed() && !model.is_changed() && !account.is_changed() && !activity.is_changed()
    {
        return;
    }
    for mut node in &mut prompts {
        node.display = if state.replacement.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (entity, action) in &buttons {
        let enabled = match action {
            ChatAction::Create => {
                state.replacement.is_none()
                    && state.operation.is_none()
                    && !activity.model_busy
                    && model.can_verify(&account)
            }
            ChatAction::Reset => {
                state.session.is_some()
                    && state.replacement.is_none()
                    && state.operation.is_none()
                    && !activity.model_busy
                    && model.can_request(&account)
            }
            ChatAction::ConfirmReplacement | ChatAction::KeepSession => {
                state.replacement.is_some() && state.operation.is_none() && !activity.model_busy
            }
            ChatAction::Retry => can_retry(&state, &model, &account, &activity),
            ChatAction::Send => state.can_send(&model, &account, &activity),
            ChatAction::Stop => {
                !state.cancelling
                    && state
                        .session
                        .as_ref()
                        .and_then(|session| session.turn.as_ref())
                        .is_some_and(|turn| {
                            state.operation == Some(turn.operation_id)
                                && matches!(turn.status, ChatTurnStatus::Running)
                        })
            }
        };
        if enabled {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
    for (mut text, is_summary, is_status) in &mut texts {
        if is_summary {
            text.set_if_neq(Text::new(summary(&state)));
        } else if is_status {
            text.set_if_neq(Text::new(status(&state)));
        }
    }
}
