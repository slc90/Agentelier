use crate::active_input::{ChatSteer, ChatSteerState, PendingDraft};
use crate::preparation::ModelState;
use agentelier_agent_host::{
    AgentHost, ConversationEvent as HostConversationEvent, HostActivity, HostFailed,
    ReceiveAgentEvents,
};
use agentelier_conversation_bridge::SessionId;
use agentelier_conversation_bridge::{
    ConversationEvent, ConversationRequest, SessionSettings, SessionView, TurnStatus,
};
use agentelier_login_ui::AccountState;
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
    pub request_index: u32,
    pub phase: Option<String>,
    pub text: String,
}
#[derive(Clone, Reflect)]
pub enum ChatTurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
    Limited,
}
#[derive(Clone, Reflect)]
pub enum ChatHistoryItem {
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone, Reflect, PartialEq)]
pub struct ChatTool {
    pub request_index: u32,
    pub id: String,
    pub output_index: usize,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub arguments_truncated: bool,
    pub status: String,
    pub output: Option<String>,
    pub retained: bool,
}
#[derive(Clone, Reflect)]
pub struct ChatProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
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
    pub request_index: u32,
    pub committed_steps: u32,
    pub tools: Vec<ChatTool>,
    pub phase: String,
    pub steers: Vec<ChatSteer>,
    pub request_instructions: String,
    pub process_cleanup: Vec<ChatProcessCleanup>,
}
#[derive(Clone, Reflect)]
pub struct ChatSession {
    pub notice: Option<String>,
    pub id: SessionId,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub account_subject: String,
    pub working_directory: String,
    pub instruction_snapshot: String,
    pub turn: Option<ChatTurn>,
    pub previous_turns: Vec<ChatTurn>,
    pub committed_items: Vec<ChatHistoryItem>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<crate::queue::ChatQueuedInput>,
    pub queue_paused: bool,
}
#[derive(Clone, Reflect)]
pub struct ChatReplacement {
    pub reset_session: Option<SessionId>,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub account_subject: String,
    pub working_directory: String,
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
    pub expanded_tools: Vec<String>,
    pub draft_revision: u64,
    pub(crate) pending_inputs: Vec<PendingDraft>,
    pub input_feedback: Option<String>,
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
                !session.execution_blocked
                    && session
                        .turn
                        .as_ref()
                        .is_none_or(|turn| !matches!(turn.status, ChatTurnStatus::Running))
                    && account
                        .account
                        .as_ref()
                        .is_some_and(|account| account.subject == session.account_subject)
            })
    }
    fn can_steer(&self, account: &AccountState) -> bool {
        !self.cancelling
            && self.replacement.is_none()
            && !account.runtime_failed
            && !self.draft.trim().is_empty()
            && !self
                .pending_inputs
                .iter()
                .any(|input| input.revision == self.draft_revision && input.text == self.draft)
            && self.session.as_ref().is_some_and(|session| {
                !session.execution_blocked
                    && session.turn.as_ref().is_some_and(|turn| {
                        self.operation == Some(turn.operation_id)
                            && matches!(turn.status, ChatTurnStatus::Running)
                    })
            })
    }
}

#[derive(Resource, Default)]
struct InputIntents {
    send: bool,
    clear_accepted: Option<PendingDraft>,
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
struct SendLabel;
#[derive(Component, Clone, Default)]
struct ReplacementPrompt;
#[derive(Component, Clone, Copy)]
enum ChatAction {
    Create,
    Send,
    Queue,
    Stop,
    Retry,
    Reset,
    ConfirmReplacement,
    KeepSession,
}
#[derive(Component, Clone, PartialEq, Eq)]
struct ChatRow {
    request_index: u32,
    session: SessionId,
    turn: u64,
    output_index: Option<usize>,
    item_id: Option<String>,
}

pub(crate) fn install(app: &mut App) {
    crate::tool_timeline::install(app);
    crate::queue::install(app);
    app.init_resource::<SessionUiStates>()
        .init_resource::<ChatState>()
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
                @button("重置当前会话", "ResetChatSession", ChatAction::Reset)--
                Text(summary) WidgetryText SessionSummary Name("ChatSessionSummary")
            ]--
            Node { column_gap: px(CONTENT_GAP), flex_shrink: 0.0, display: Display::None } ReplacementPrompt
            Children [
                Text("确认后创建空白会话；原会话的记录、待办和草稿保留。") WidgetryText--
                @button("确认新建", "ConfirmChatReplacement", ChatAction::ConfirmReplacement)--
                @button("保留当前会话", "KeepChatSession", ChatAction::KeepSession)
            ]--
            Text(status) WidgetryText ChatStatus Name("ChatStatus")--
            @WidgetryScrollArea {
                @children: {Some(Box::new(bsn_list! {
                    MessageList Name("ChatMessages")
                    Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(CONTENT_GAP) }
                }) as Box<dyn SceneList>)}
            }
            Name("ChatMessageScroll") crate::reading::ChatReadingScroll Node { width: percent(100), height: px(0), flex_basis: px(0), flex_grow: 1.0, flex_shrink: 1.0, min_height: px(0) }--
            @crate::queue::panel()--
            @WidgetryTextField {
                @value: {draft}, @multiline: true,
                @auto_size: {Some(AutoSize { min_rows: COMPOSER_MIN_ROWS, max_rows: COMPOSER_MAX_ROWS })},
                @hint_text: {Some("输入消息；Enter 提交，Shift+Enter 换行".into())},
            }
            Composer Name("ChatComposer") Node { width: percent(100), flex_shrink: 0.0 }--
            Node { column_gap: px(CONTENT_GAP), flex_shrink: 0.0 }
            Children [
                @button("发送", "SendChatMessage", ChatAction::Send)--
                @button("排队", "QueueChatMessage", ChatAction::Queue)--
                @button("停止", "StopChatGeneration", ChatAction::Stop)--
                @button("重试最近尝试", "RetryChatTurn", ChatAction::Retry)--
                @crate::reading::bottom_button()
            ]
        ]
    }
}

pub(crate) fn create_button() -> impl Scene {
    button("新建会话", "CreateChatSession", ChatAction::Create)
}

#[derive(Default)]
struct SessionUiState {
    draft: String,
    revision: u64,
    expanded_tools: Vec<String>,
}
#[derive(Resource, Default)]
struct SessionUiStates(std::collections::HashMap<SessionId, SessionUiState>);

fn switch_view(world: &mut World, view: SessionView) -> Result {
    let next = project(view)?;
    let previous = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|session| session.id);
    if previous != Some(next.id) {
        let saved = {
            let mut state = world.resource_mut::<ChatState>();
            let saved = SessionUiState {
                draft: std::mem::take(&mut state.draft),
                revision: state.draft_revision,
                expanded_tools: std::mem::take(&mut state.expanded_tools),
            };
            state.pending_inputs.clear();
            state.input_feedback = None;
            saved
        };
        let restored = {
            let mut states = world.resource_mut::<SessionUiStates>();
            if let Some(previous) = previous {
                states.0.insert(previous, saved);
            }
            states.0.remove(&next.id).unwrap_or_default()
        };
        let roots = world
            .query_filtered::<Entity, With<Composer>>()
            .iter(world)
            .collect::<Vec<_>>();
        for root in roots {
            WidgetryTextFieldHeadless::set_text_in_world(world, root, &restored.draft)
                .map_err(|failure| failure.into_bevy_error())?;
        }
        let mut state = world.resource_mut::<ChatState>();
        state.draft = restored.draft;
        state.draft_revision = restored.revision;
        state.expanded_tools = restored.expanded_tools;
        crate::reading::switch(world, previous, next.id);
    }
    world.resource_mut::<ChatState>().session = Some(next);
    Ok(())
}

fn forget_session(world: &mut World, session: SessionId) -> Result {
    world.resource_mut::<SessionUiStates>().0.remove(&session);
    let current = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .is_some_and(|view| view.id == session);
    crate::reading::forget(world, session, current);
    if current {
        let roots = world
            .query_filtered::<Entity, With<Composer>>()
            .iter(world)
            .collect::<Vec<_>>();
        for root in roots {
            WidgetryTextFieldHeadless::set_text_in_world(world, root, "")
                .map_err(|failure| failure.into_bevy_error())?;
        }
        *world.resource_mut::<ChatState>() = default();
    }
    Ok(())
}

fn button(label: &'static str, name: &'static str, action: ChatAction) -> impl Scene {
    let children: Box<dyn SceneList> = if matches!(action, ChatAction::Send) {
        Box::new(bsn_list! {Text(label) WidgetryText SendLabel})
    } else {
        Box::new(bsn_list! {Text(label) WidgetryText})
    };
    bsn! {
        @WidgetryButton { @children: {Some(children)} }
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
        ChatAction::Queue => commands.queue(crate::queue::enqueue),
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
    if world.resource::<crate::ArchiveState>().loading {
        return Ok(());
    }
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
        image_input: choice.image_input,
        account_subject: subject,
        instructions: Arc::new(crate::prompts::snapshot(world)?),
        working_directory: world
            .resource::<crate::working_directory::DirectoryState>()
            .input
            .clone()
            .into(),
    };
    if state.session.is_some() {
        world.resource_mut::<ChatState>().replacement = Some(ChatReplacement {
            reset_session: None,
            slug: settings.slug,
            display_name: settings.display_name,
            effort: settings.effort,
            image_input: settings.image_input,
            account_subject: settings.account_subject,
            instructions: settings.instructions,
            working_directory: settings.working_directory.to_string_lossy().into_owned(),
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
    if world.resource::<crate::ArchiveState>().loading {
        return Ok(());
    }
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
        image_input: session.image_input,
        account_subject: session.account_subject.clone(),
        instructions: Arc::new(serde_json::from_str(&session.instruction_snapshot)?),
        working_directory: session.working_directory.clone(),
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
            image_input: pending.image_input,
            account_subject: pending.account_subject,
            instructions: pending.instructions,
            working_directory: pending.working_directory.into(),
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
            !session.execution_blocked
                && account
                    .account
                    .as_ref()
                    .is_some_and(|account| account.subject == session.account_subject)
                && session.turn.as_ref().is_some_and(|turn| {
                    turn.committed_steps == 0
                        && !turn
                            .steers
                            .iter()
                            .any(|input| matches!(input.state, ChatSteerState::Applied))
                        && matches!(
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
                session_id = %session,
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
    if world.resource::<ChatState>().operation.is_some() {
        return steer(world);
    }
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
    let submitted = PendingDraft {
        id: 0,
        text: input.clone(),
        revision: state.draft_revision,
    };
    let result = world
        .resource::<AgentHost>()
        .request_conversation(ConversationRequest::Send { session, input });
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.pending_inputs.push(PendingDraft { id, ..submitted });
            state.input_feedback = None;
            state.error = None;
            info!(operation_id = id, session_id = %session, "开始提交聊天消息");
        }
        Err(failure) => {
            error!(error = %failure, "无法提交聊天消息");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn steer(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    if !state.can_steer(world.resource::<AccountState>()) {
        return Ok(());
    }
    let session = state
        .session
        .as_ref()
        .ok_or_else(|| io::Error::other("补充输入缺少会话"))?;
    let turn = session
        .turn
        .as_ref()
        .ok_or_else(|| io::Error::other("补充输入缺少任务"))?;
    let submitted = PendingDraft {
        id: 0,
        text: state.draft.clone(),
        revision: state.draft_revision,
    };
    let result = world.resource::<AgentHost>().steer_conversation(
        session.id,
        turn.id,
        turn.operation_id,
        submitted.text.clone(),
    );
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.pending_inputs.push(PendingDraft { id, ..submitted });
            state.input_feedback = Some("正在提交补充输入…".into());
        }
        Err(failure) => {
            state.input_feedback = Some(format!("补充输入未提交：{failure}；草稿保留"));
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
        if clear.as_ref().is_some_and(|submitted| {
            submitted.text == value
                && submitted.revision == world.resource::<ChatState>().draft_revision
        }) {
            WidgetryTextFieldHeadless::set_text_in_world(world, root, "")
                .map_err(|failure| failure.into_bevy_error())?;
            value.clear();
        }
        if world.resource::<ChatState>().draft != value {
            let mut state = world.resource_mut::<ChatState>();
            state.draft = value;
            state.draft_revision = state
                .draft_revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("草稿修订已耗尽"))?;
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
        agentelier_conversation_bridge::HistoryItem::FunctionCall { call_id, name } => {
            ChatHistoryItem::FunctionCall { call_id, name }
        }
        agentelier_conversation_bridge::HistoryItem::FunctionResult { call_id } => {
            ChatHistoryItem::FunctionResult { call_id }
        }
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
        steers: turn
            .steers
            .into_iter()
            .map(|input| ChatSteer {
                id: input.id,
                text: input.text,
                applied_request: input.applied_request,
                state: match input.state {
                    agentelier_conversation_bridge::SteerState::Accepted => {
                        ChatSteerState::Accepted
                    }
                    agentelier_conversation_bridge::SteerState::Applied => ChatSteerState::Applied,
                    agentelier_conversation_bridge::SteerState::NotApplied => {
                        ChatSteerState::NotApplied
                    }
                },
            })
            .collect(),
        request_index: turn.request_index,
        committed_steps: turn.committed_steps,
        phase: turn.phase,
        tools: turn
            .tools
            .into_iter()
            .map(|tool| ChatTool {
                request_index: tool.request_index,
                id: tool.id,
                output_index: tool.output_index,
                call_id: tool.call_id,
                name: tool.name,
                arguments: tool.arguments,
                arguments_truncated: tool.arguments_truncated,
                status: tool.status,
                output: tool.output,
                retained: tool.retained,
            })
            .collect(),
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
                request_index: message.request_index,
                phase: message.phase,
                text: message.text,
            })
            .collect(),
        status: match turn.status {
            TurnStatus::Limited => ChatTurnStatus::Limited,
            TurnStatus::Running => ChatTurnStatus::Running,
            TurnStatus::Completed => ChatTurnStatus::Completed,
            TurnStatus::Cancelled => ChatTurnStatus::Cancelled,
            TurnStatus::Failed(reason) => ChatTurnStatus::Failed(reason),
        },
        request_items: turn.request_items.into_iter().map(history_item).collect(),
        request_instructions: serde_json::to_string_pretty(&turn.request_instructions)?,
        process_cleanup: turn
            .process_cleanup
            .into_iter()
            .map(|record| ChatProcessCleanup {
                session_id: record.session_id,
                cleanup_confirmed: record.cleanup_confirmed,
                output: record.output,
            })
            .collect(),
    })
}
fn project(session: SessionView) -> Result<ChatSession> {
    Ok(ChatSession {
        notice: session.notice,
        execution_blocked: session.execution_blocked,
        tool_catalog: session.tool_catalog,
        queue: session
            .queue
            .into_iter()
            .map(|input| crate::queue::ChatQueuedInput {
                operation_id: input.operation_id,
                input_id: input.input_id,
                text: input.text,
            })
            .collect(),
        queue_paused: session.queue_paused,
        id: session.id,
        slug: session.settings.slug,
        display_name: session.settings.display_name,
        effort: session.settings.effort,
        image_input: session.settings.image_input,
        account_subject: session.settings.account_subject,
        working_directory: bevy_widgetry_path_display::display_path(
            &session.settings.working_directory,
        ),
        instruction_snapshot: serde_json::to_string_pretty(session.settings.instructions.as_ref())?,
        turn: session.turn.map(|turn| project_turn(*turn)).transpose()?,
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
    let mut changed = false;
    let events: Vec<_> = world
        .resource_mut::<Messages<HostConversationEvent>>()
        .drain()
        .collect();
    for event in events {
        if let ConversationEvent::ArchiveCleanup(errors) = event.kind {
            world.resource_mut::<crate::ArchiveState>().cleanup_errors = errors;
            continue;
        }
        if let ConversationEvent::Deleted {
            session,
            cleanup_error,
            ..
        } = event.kind
        {
            if crate::session_actions::deleted(world, event.operation_id, session) {
                crate::archives::remove_entry(world, session, cleanup_error)?;
                forget_session(world, session)?;
                changed = true;
            }
            continue;
        }
        if let ConversationEvent::Rejected(reason) = &event.kind
            && crate::session_actions::rejected(world, event.operation_id, reason)
        {
            continue;
        }

        if let ConversationEvent::Renamed { session, result } = event.kind {
            crate::session_actions::complete(world, event.operation_id, session, result)?;
            continue;
        }
        if let ConversationEvent::Archives(entries) = event.kind {
            crate::archives::catalog(world, entries)?;
            changed = true;
            continue;
        }
        if let ConversationEvent::Opened(view) = event.kind {
            let id = view.id;
            let different = world
                .resource::<ChatState>()
                .session
                .as_ref()
                .is_none_or(|session| session.id != id);
            let opening = world.resource::<crate::ArchiveState>().opening;
            if event.operation_id != 0 && opening != Some((event.operation_id, id)) {
                continue;
            }
            if event.operation_id == 0
                && !different
                && world.resource::<ChatState>().operation.is_some()
            {
                continue;
            }
            if event.operation_id == 0
                && different
                && world.resource::<ChatState>().session.is_some()
            {
                continue;
            }
            switch_view(world, view)?;
            if event.operation_id != 0 {
                let mut state = world.resource_mut::<ChatState>();
                state.operation = None;
                state.replacement = None;
                state.error = None;
                world.resource_mut::<crate::ArchiveState>().opening = None;
            }
            let waiting = world.resource::<ChatState>().operation.is_some();
            {
                let mut state = world.resource_mut::<crate::ArchiveState>();
                state.selected = Some(id);
                // Account rebinding must not dismiss a failed open of another session.
                if different || event.operation_id != 0 {
                    state.error = None;
                }
                if !waiting {
                    state.loading = false;
                }
            }
            crate::archives::sync_selection(world)?;
            changed = true;
            continue;
        }
        if let ConversationEvent::ArchiveFailed(reason) = event.kind {
            let matching = world.resource::<ChatState>().operation == Some(event.operation_id);
            if event.operation_id != 0 && !matching {
                continue;
            }
            if matching {
                world.resource_mut::<crate::ArchiveState>().opening = None;
            }
            if matching {
                world.resource_mut::<ChatState>().operation = None;
            }
            let waiting = world.resource::<ChatState>().operation.is_some();
            {
                let mut state = world.resource_mut::<crate::ArchiveState>();
                state.error = Some(reason);
                state.refreshing = false;
                if !waiting {
                    state.loading = false;
                }
            }
            crate::archives::sync_selection(world)?;
            changed = true;
            continue;
        }
        if let ConversationEvent::Queue(reply) = event.kind {
            let clear = crate::queue::receive(&mut world.resource_mut::<ChatState>(), reply);
            if clear.is_some() {
                world.resource_mut::<InputIntents>().clear_accepted = clear;
            }
            changed = true;
            continue;
        }
        if let ConversationEvent::Started(view) = event.kind {
            if world
                .resource::<ChatState>()
                .session
                .as_ref()
                .is_none_or(|session| session.id != view.id)
            {
                continue;
            }
            let mut state = world.resource_mut::<ChatState>();
            state.session = Some(project(view)?);
            state.operation = Some(event.operation_id);
            state.cancelling = false;
            state.error = None;
            state.input_feedback = None;
            info!(operation_id = event.operation_id, "排队消息已开始执行");
            changed = true;
            continue;
        }
        if let ConversationEvent::Rejected(reason) = &event.kind
            && world.resource::<ChatState>().operation != Some(event.operation_id)
            && world
                .resource::<ChatState>()
                .pending_inputs
                .iter()
                .any(|input| input.id == event.operation_id)
        {
            let mut state = world.resource_mut::<ChatState>();
            state
                .pending_inputs
                .retain(|input| input.id != event.operation_id);
            state.input_feedback = Some(format!("发送未接纳：{reason}；草稿保留"));
            changed = true;
            continue;
        }
        if let ConversationEvent::InputReceipt(receipt) = event.kind {
            let clear =
                crate::active_input::receipt(&mut world.resource_mut::<ChatState>(), receipt);
            if clear.is_some() {
                world.resource_mut::<InputIntents>().clear_accepted = clear;
            }
            changed = true;
            continue;
        }
        if world.resource::<ChatState>().operation != Some(event.operation_id) {
            continue;
        }
        changed = true;
        let terminal = event.kind.is_terminal();
        match &event.kind {
            ConversationEvent::Created(view) => info!(
                operation_id = event.operation_id,
                session_id = %view.id,
                "聊天会话已创建"
            ),
            ConversationEvent::Accepted(view) => info!(
                operation_id = event.operation_id,
                session_id = %view.id,
                "聊天消息已接纳"
            ),
            ConversationEvent::Finished(view) => {
                if let Some(turn) = &view.turn {
                    match &turn.status {
                        TurnStatus::Completed => info!(
                            operation_id = event.operation_id,
                            session_id = %view.id,
                            turn_id = turn.id,
                            "聊天轮次已完成并回收"
                        ),
                        TurnStatus::Cancelled => info!(
                            operation_id = event.operation_id,
                            session_id = %view.id,
                            turn_id = turn.id,
                            "聊天轮次已实际取消"
                        ),
                        TurnStatus::Failed(reason) => error!(
                            operation_id = event.operation_id,
                            session_id = %view.id,
                            turn_id = turn.id,
                            error = reason,
                            "聊天轮次失败，保留部分正文"
                        ),
                        TurnStatus::Limited => info!(
                            operation_id = event.operation_id,
                            "聊天任务已触限，保留完整步骤"
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
            ConversationEvent::Updated(_)
            | ConversationEvent::Archives(_)
            | ConversationEvent::Opened(_)
            | ConversationEvent::ArchiveFailed(_)
            | ConversationEvent::InputReceipt(_)
            | ConversationEvent::Queue(_)
            | ConversationEvent::ArchiveCleanup(_)
            | ConversationEvent::Deleted { .. }
            | ConversationEvent::Renamed { .. }
            | ConversationEvent::Started(_) => {}
        }
        match event.kind {
            ConversationEvent::Rejected(reason) => {
                let mut state = world.resource_mut::<ChatState>();
                state
                    .pending_inputs
                    .retain(|input| input.id != event.operation_id);
                state.error = Some(reason);
            }
            ConversationEvent::Created(view) => {
                let selected = view.id;
                switch_view(world, view)?;
                world.resource_mut::<ChatState>().replacement = None;
                let mut archives = world.resource_mut::<crate::ArchiveState>();
                archives.selected = Some(selected);
                archives.loading = false;
            }
            ConversationEvent::Updated(view) | ConversationEvent::Finished(view) => {
                world.resource_mut::<ChatState>().session = Some(project(view)?);
            }
            ConversationEvent::Accepted(view) => {
                let clear = {
                    let mut state = world.resource_mut::<ChatState>();
                    state
                        .pending_inputs
                        .iter()
                        .position(|input| input.id == event.operation_id)
                        .map(|index| state.pending_inputs.remove(index))
                };
                world.resource_mut::<InputIntents>().clear_accepted = clear;
                world.resource_mut::<ChatState>().session = Some(project(view)?);
            }
            ConversationEvent::InputReceipt(_)
            | ConversationEvent::Archives(_)
            | ConversationEvent::Opened(_)
            | ConversationEvent::ArchiveFailed(_)
            | ConversationEvent::Queue(_)
            | ConversationEvent::ArchiveCleanup(_)
            | ConversationEvent::Deleted { .. }
            | ConversationEvent::Renamed { .. }
            | ConversationEvent::Started(_) => {}
        }
        if terminal {
            world.resource_mut::<ChatState>().cancelling = false;
            world.resource_mut::<ChatState>().operation = None;
            world.resource_mut::<crate::ArchiveState>().loading = false;
            world.resource_mut::<crate::ArchiveState>().opening = None;
            match world.resource::<AgentHost>().refresh_archives() {
                Ok(()) => world.resource_mut::<crate::ArchiveState>().refreshing = true,
                Err(failure) => {
                    world.resource_mut::<crate::ArchiveState>().error = Some(failure.to_string())
                }
            }
        }
    }
    if let Some(reason) = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|failure| failure.reason.clone())
    {
        world.resource_mut::<crate::ArchiveState>().loading = false;
        world.resource_mut::<crate::ArchiveState>().refreshing = false;
        crate::session_actions::failed(world, &reason);
        let mut state = world.resource_mut::<ChatState>();
        state.operation = None;
        state.pending_inputs.clear();
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

enum RowContent {
    Plain(String),
    Assistant { label: String, source: String },
    Tool(ChatTool),
}

fn reconcile(world: &mut World) -> Result {
    crate::queue::reconcile(world)?;
    let state = world.resource::<ChatState>().clone();
    let mut lists = world.query_filtered::<Entity, With<MessageList>>();
    let lists: Vec<_> = lists.iter(world).collect();
    for list in lists {
        let mut rows = world.query::<(Entity, &ChatRow, &ChildOf)>();
        let existing: Vec<_> = rows
            .iter(world)
            .filter(|(_, _, parent)| parent.parent() == list)
            .map(|(entity, row, _)| (entity, row.clone()))
            .collect();
        let mut desired: Vec<(ChatRow, RowContent)> = Vec::new();
        if let Some(session) = &state.session {
            for turn in session.previous_turns.iter().chain(session.turn.iter()) {
                let user_key = ChatRow {
                    session: session.id,
                    turn: turn.input_id,
                    request_index: 0,
                    output_index: None,
                    item_id: None,
                };
                if !desired.iter().any(|(key, _)| key == &user_key) {
                    desired.push((user_key, RowContent::Plain(format!("你\n{}", turn.input))));
                }
                if turn.messages.is_empty() && turn.tools.is_empty() {
                    desired.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            request_index: 0,
                            output_index: None,
                            item_id: Some("attempt".into()),
                        },
                        RowContent::Plain(format!(
                            "助手 · 尝试 {}{}",
                            turn.id,
                            if turn.is_retry { " · 重试" } else { "" }
                        )),
                    ));
                }
                let mut output = Vec::new();
                for message in &turn.messages {
                    let retained = if message.request_index <= turn.committed_steps {
                        "已保留"
                    } else if matches!(turn.status, ChatTurnStatus::Running) {
                        "接收中"
                    } else {
                        "未纳入后续上下文"
                    };
                    output.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            request_index: message.request_index,
                            output_index: Some(message.output_index),
                            item_id: Some(message.id.clone()),
                        },
                        RowContent::Assistant {
                            label: format!(
                                "助手 · {} · 请求 {} · {retained}",
                                message.phase.as_deref().unwrap_or("回答"),
                                message.request_index
                            ),
                            source: message.text.clone(),
                        },
                    ));
                }
                for tool in &turn.tools {
                    output.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            request_index: tool.request_index,
                            output_index: Some(tool.output_index),
                            item_id: Some(tool.id.clone()),
                        },
                        RowContent::Tool(tool.clone()),
                    ));
                }
                for input in &turn.steers {
                    let label = match input.state {
                        ChatSteerState::Accepted => "已接纳，等待生效",
                        ChatSteerState::Applied => "已用于模型请求",
                        ChatSteerState::NotApplied => "未生效",
                    };
                    output.push((
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            request_index: input.applied_request.unwrap_or(u32::MAX),
                            output_index: None,
                            item_id: Some(format!("steer-{}", input.id)),
                        },
                        RowContent::Plain(format!(
                            "你 · 补充 {} · {label}\n{}",
                            input.id, input.text
                        )),
                    ));
                }
                output.sort_by_key(|(key, _)| (key.request_index, key.output_index));
                desired.extend(output);
            }
        }
        for (entity, key) in &existing {
            if !desired.iter().any(|(wanted, _)| wanted == key) {
                world.entity_mut(*entity).despawn();
            }
        }
        let mut ordered = Vec::new();
        for (key, content) in desired {
            if let Some((entity, _)) = existing.iter().find(|(_, row)| *row == key) {
                ordered.push(*entity);
                match content {
                    RowContent::Tool(tool) => crate::tool_timeline::set_tool(world, *entity, tool)?,
                    RowContent::Plain(text) => {
                        world
                            .get_mut::<Text>(*entity)
                            .ok_or_else(|| io::Error::other("聊天消息缺少正文"))?
                            .set_if_neq(Text(text));
                    }
                    RowContent::Assistant { label, source } => {
                        let children = world
                            .get::<Children>(*entity)
                            .ok_or_else(|| io::Error::other("助手消息缺少内容区域"))?
                            .to_vec();
                        for child in children {
                            if let Some(mut markdown) = world.get_mut::<MarkdownSource>(child) {
                                markdown.set_if_neq(MarkdownSource(source.clone()));
                            }
                            if world.get::<MessageLabel>(child).is_some() {
                                world
                                    .get_mut::<Text>(child)
                                    .ok_or_else(|| io::Error::other("助手消息缺少标题"))?
                                    .set_if_neq(Text(label.clone()));
                            }
                        }
                    }
                }
            } else {
                let name = format!(
                    "ChatMessage-{}-{}-{}-{}",
                    key.session,
                    key.turn,
                    key.request_index,
                    key.output_index
                        .map(|index| index.to_string())
                        .unwrap_or_else(|| key.item_id.clone().unwrap_or_else(|| "user".into()))
                );
                let entity = match content {
                    RowContent::Tool(tool) => {
                        let expanded = state.expanded_tools.contains(&name);
                        let card_key = name.clone();
                        world.commands().spawn_scene_with_error_handler(bsn! {
                            @crate::tool_timeline::card(card_key,tool,expanded)
                            Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                        }).id()
                    }
                    RowContent::Assistant {label,source} => world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                        Node {width:percent(100),min_width:px(0),flex_shrink:0.0,flex_direction:FlexDirection::Column,row_gap:px(CONTENT_GAP)}
                        Children [Text(label) WidgetryText MessageLabel-- @MarkdownView {@source:source}]
                    }).id(),
                    RowContent::Plain(text) => world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) Text(text) WidgetryText template(move |_| Ok(key.clone()))
                        TextLayout::linebreak(LineBreak::AnyCharacter)
                        Node {width:percent(100),min_width:px(0),flex_shrink:0.0}
                    }).id(),
                };
                ordered.push(entity);
            }
        }
        let unchanged = world
            .get::<Children>(list)
            .is_some_and(|children| children.iter().eq(ordered.iter().copied()));
        if !unchanged {
            world.commands().entity(list).replace_children(&ordered);
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
                "{} · {} · 推理：{}\n本地工作目录：{}",
                session.display_name,
                session.slug,
                session.effort.as_deref().unwrap_or("服务默认"),
                session.working_directory
            )
        })
        .unwrap_or_else(|| "选择模型与推理设置后创建会话".into())
}
fn status(state: &ChatState) -> String {
    let mut value = task_status(state);
    if let Some(notice) = state
        .session
        .as_ref()
        .and_then(|session| session.notice.as_ref())
    {
        value.push('\n');
        value.push_str(notice);
    }
    if let Some(turn) = state
        .session
        .as_ref()
        .and_then(|session| session.turn.as_ref())
        && !turn.process_cleanup.is_empty()
    {
        let confirmed = turn
            .process_cleanup
            .iter()
            .filter(|record| record.cleanup_confirmed)
            .count();
        value.push_str(&format!(
            "\n进程清理：{confirmed}/{} 已回收；详情见会话与请求核对",
            turn.process_cleanup.len()
        ));
    }
    if let Some(feedback) = &state.input_feedback {
        value.push('\n');
        value.push_str(feedback);
    }
    value
}
fn task_status(state: &ChatState) -> String {
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
            ChatTurnStatus::Running => format!(
                "{} · 模型请求 {}",
                if turn.phase == "tool" {
                    "正在执行工具…"
                } else if turn.phase == "cleanup" {
                    "正在回收进程…"
                } else {
                    "正在请求模型…"
                },
                turn.request_index
            ),
            ChatTurnStatus::Limited => {
                format!("任务触限；已保留 {} 个完整步骤", turn.committed_steps)
            }
            ChatTurnStatus::Completed => "本轮已完成".into(),
            ChatTurnStatus::Cancelled => format!(
                "本轮已取消；保留 {} 个完整步骤，未完成响应未进入上下文",
                turn.committed_steps
            ),
            ChatTurnStatus::Failed(reason) => format!(
                "本轮失败；保留 {} 个完整步骤，未完成响应未进入上下文：{reason}",
                turn.committed_steps
            ),
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
    archives: Res<crate::ArchiveState>,
    activity: Res<HostActivity>,
    buttons: Query<(Entity, &ChatAction)>,
    mut texts: Query<(
        &mut Text,
        Has<SessionSummary>,
        Has<ChatStatus>,
        Has<SendLabel>,
    )>,
    mut prompts: Query<&mut Node, With<ReplacementPrompt>>,
    mut commands: Commands,
) {
    if !state.is_changed()
        && !model.is_changed()
        && !account.is_changed()
        && !activity.is_changed()
        && !archives.is_changed()
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
            ChatAction::Queue => !account.runtime_failed && crate::queue::can_enqueue(&state),
            ChatAction::Send => {
                state.can_send(&model, &account, &activity) || state.can_steer(&account)
            }
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
    for (mut text, is_summary, is_status, is_send) in &mut texts {
        if is_send {
            text.set_if_neq(Text::new(if state.operation.is_some() {
                "补充当前任务"
            } else {
                "发送"
            }));
        }
        if is_summary {
            let title = state
                .session
                .as_ref()
                .and_then(|session| archives.entries.iter().find(|entry| entry.id == session.id))
                .map(|entry| entry.title.as_str())
                .unwrap_or("新会话");
            text.set_if_neq(Text::new(format!("{title} · {}", summary(&state))));
        } else if is_status {
            text.set_if_neq(Text::new(status(&state)));
        }
    }
}

pub(crate) fn request_details(state: &ChatState) -> String {
    let Some(session) = &state.session else {
        return "尚未创建会话".into();
    };
    let mut text = format!(
        "Session {}\n模型：{}\n推理强度：{}\n本地工作目录：{}\n固定指令快照：\n{}\n固定工具目录：\n",
        session.id,
        session.slug,
        session.effort.as_deref().unwrap_or("服务默认"),
        session.working_directory,
        session.instruction_snapshot
    );
    for (name, description) in &session.tool_catalog {
        text.push_str(&format!("{name}: {description}\n"));
    }
    for turn in session.previous_turns.iter().chain(session.turn.iter()) {
        for record in &turn.process_cleanup {
            text.push_str(&format!(
                "\n任务 {} 自动进程清理 {}：\n{}\n",
                turn.id, record.session_id, record.output
            ));
        }
        text.push_str(&format!(
            "\n任务 {} / 操作 {}；模型请求序号：{}；已保留步骤：{}\n最近请求指令：\n{}\n",
            turn.id,
            turn.operation_id,
            turn.request_index,
            turn.committed_steps,
            turn.request_instructions
        ));
        for tool in &turn.tools {
            text.push_str(&format!(
                "\n请求 {} / 输出 {} / {} / {}\n工具 {}；{}；已保留 {}\n参数{}：{}\n结果：{}\n",
                tool.request_index,
                tool.output_index,
                tool.id,
                tool.call_id,
                tool.name,
                tool.status,
                tool.retained,
                if tool.arguments_truncated {
                    "（超限，仅显示前缀）"
                } else {
                    ""
                },
                tool.arguments,
                tool.output.as_deref().unwrap_or("尚无结果")
            ));
        }
    }
    text
}
