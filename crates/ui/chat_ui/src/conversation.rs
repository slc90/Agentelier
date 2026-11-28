use crate::ArchiveState;
use crate::active_input::{ChatSteer, ChatSteerState, PendingDraft};
use crate::composer::{self, Boundary, TextSegment};
use crate::preparation::ModelState;
use agentelier_agent_host::{
    AgentHost, ConversationEvent as HostConversationEvent, HostFailed, ReceiveAgentEvents,
};
use agentelier_controls::{
    AgentelierAvatar, AgentelierButton, AgentelierButtonLoading, AgentelierButtonStyle,
    AvatarSource,
};
use agentelier_conversation_bridge::{
    ComposerContent, ComposerElement, ComposerSubmission, ConversationEvent, ConversationRequest,
    SessionAction, SessionCreate, SessionView, TurnStatus,
};
use agentelier_conversation_bridge::{ProjectId, SessionId, SessionLocation};
use agentelier_login_ui::AccountState;
use agentelier_markdown::{MarkdownSource, MarkdownView};
use bevy::camera::RenderTarget;
use bevy::ecs::message::MessageCursor;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input_focus::{InputFocus, InputFocusSystems};
use bevy::prelude::*;
use bevy::text::{EditableTextSystems, Justify, LineBreak};
use bevy::ui::{ComputedUiTargetCamera, InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy::window::{Ime, WindowRef};
use bevy_app_assets::SidebarIcon;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::{ScrollbarPolicy, ScrollbarVisibility, WidgetryScrollArea};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::WidgetryTextFieldHeadless;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use tracing::{error, info};

const MESSAGE_GAP: f32 = 4.0;
const CONTENT_GAP: f32 = 12.0;
const CONTENT_PADDING: f32 = 8.0;
const BUTTON_HEIGHT: f32 = 40.0;
const REPLY_SPINNER_SIZE: f32 = 16.0;

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
    AgentResult,
    Project { publication: u64 },
    Compaction,
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone, Reflect, PartialEq)]
pub struct ChatTool {
    pub collaboration: bool,
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
    pub(super) generated_image: Option<crate::generated_image::Artifact>,
    pub published_audio: Option<crate::audio_attachment::ChatAudio>,
    #[reflect(ignore)]
    pub(super) published_scene: Option<agentelier_conversation_bridge::PublishedScene>,
}
#[derive(Clone, Reflect)]
pub struct ChatProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
}
#[derive(Clone, Reflect)]
pub struct ChatRequestMemory {
    pub publication_id: String,
    pub hash: String,
    pub text: String,
    pub role: String,
    pub input_index: usize,
}

#[derive(Clone, Reflect)]
pub struct ChatRequestBehavior {
    pub hash: String,
    pub text: String,
    pub input_index: usize,
}

#[derive(Clone, Reflect)]
pub struct ChatRequest {
    pub fast: bool,
    pub behavior: Option<ChatRequestBehavior>,
    pub transcript: Option<(usize, crate::transcription::ChatTranscriptReference)>,
    pub response_accepted: Option<bool>,
    pub additional_tools_in_input: bool,
    pub memory: Option<ChatRequestMemory>,
    pub skills: Vec<(usize, String)>,
    pub sending_started: bool,
    pub response_status: Option<u16>,
    pub request: u32,
    pub window: u64,
    pub publication: Option<u64>,
    pub items: Vec<ChatHistoryItem>,
    pub base_hashes: Vec<String>,
    pub payload_hash: String,
    pub payload_bytes: usize,
}
#[derive(Clone, Reflect)]
pub struct ChatTurn {
    pub result_continuation: bool,
    pub compaction_continuation: Option<u32>,
    pub requests: Vec<ChatRequest>,
    pub id: u64,
    pub operation_id: u64,
    pub input_id: u64,
    pub is_retry: bool,
    pub input: String,
    #[reflect(ignore)]
    input_elements: Vec<ComposerElement>,
    pub audio: Vec<crate::audio_attachment::ChatAudio>,
    pub transcript: Option<crate::transcription::ChatTranscriptReference>,
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
    pub codex_identity: Option<(String, String)>,
    pub project_publications: Vec<crate::request_inspection::ChatPublication>,
    pub project_context: crate::project_context::ProjectContext,
    pub notice: Option<String>,
    pub id: SessionId,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub fast: bool,
    pub requires_websocket_model: bool,
    pub image_input: Option<bool>,
    #[reflect(ignore)]
    pub context_limits: Box<agentelier_conversation_bridge::ContextLimits>,
    pub context: String,
    pub context_details: String,
    pub working_directory: String,
    #[reflect(ignore)]
    pub(crate) working_path: std::path::PathBuf,
    pub instruction_snapshot: String,
    pub turn: Option<ChatTurn>,
    pub previous_turns: Vec<ChatTurn>,
    pub audio_draft: Option<crate::audio_attachment::ChatAudio>,
    pub transcriptions: Vec<crate::transcription::ChatTranscription>,
    pub committed_items: Vec<ChatHistoryItem>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
    #[reflect(ignore)]
    pub(crate) mcp_capabilities: Vec<agentelier_conversation_bridge::McpCapability>,
    pub queue: Vec<crate::queue::ChatQueuedInput>,
    pub queue_paused: bool,
}
impl ChatSession {
    fn binding(&self) -> Option<(&str, &str)> {
        self.codex_identity
            .as_ref()
            .map(|(subject, workspace)| (subject.as_str(), workspace.as_str()))
    }

    pub(crate) fn matches_account(&self, account: &AccountState) -> bool {
        self.binding()
            .is_some_and(|binding| account.model_identity() == Some(binding))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub(crate) enum NewChatTarget {
    Project(ProjectId),
    Standalone,
}

#[derive(Clone)]
pub struct SessionModelDefaults {
    pub slug: String,
    pub effort: String,
    pub fast: bool,
}

#[derive(Resource)]
pub(crate) struct SessionDefaultsSource(
    pub(crate) Box<dyn Fn() -> io::Result<SessionModelDefaults> + Send + Sync>,
);

#[derive(Resource, Clone, Reflect, Default)]
#[reflect(Resource)]
pub struct ChatState {
    pub selected_request: Option<(u64, u32)>,
    pub inspection_sources_expanded: bool,
    pub project_rules_expanded: bool,
    pub session: Option<ChatSession>,
    new_chat: Option<NewChatTarget>,
    pub task_revision: Option<u64>,
    pub operation: Option<u64>,
    pub cancelling: bool,
    #[reflect(ignore)]
    pub(crate) composer: composer::Draft,
    pub error: Option<String>,
    pub draft_revision: u64,
    pub(crate) pending_inputs: Vec<PendingDraft>,
    pub(crate) queue_controls: Vec<u64>,
    pub input_feedback: Option<String>,
    #[reflect(ignore)]
    pub(super) draft_model: Option<SessionModelDefaults>,
    #[reflect(ignore)]
    pub(super) pending_model: Option<super::composer_footer::PendingModel>,
}

impl ChatState {
    pub(crate) fn draft_target(&self) -> Option<NewChatTarget> {
        self.new_chat
    }

    fn can_send(&self, model: &ModelState, account: &AccountState) -> bool {
        self.operation.is_none()
            && self.pending_model.is_none()
            && model.can_request(account)
            && self.composer.has_input()
            && (self.new_chat.is_some() && model.can_create_session(account)
                || self.session.as_ref().is_some_and(|session| {
                    !session.execution_blocked
                        && session.matches_account(account)
                        && session
                            .turn
                            .as_ref()
                            .is_none_or(|turn| !matches!(turn.status, ChatTurnStatus::Running))
                }))
    }
    fn can_steer(&self, account: &AccountState) -> bool {
        !self.cancelling
            && !account.runtime_failed
            && account.model_unavailable_reason().is_none()
            && self.composer.has_input()
            && !self
                .pending_inputs
                .iter()
                .any(|input| input.revision == self.draft_revision)
            && self.session.as_ref().is_some_and(|session| {
                !session.execution_blocked
                    && session.matches_account(account)
                    && session.turn.as_ref().is_some_and(|turn| {
                        self.operation == Some(turn.operation_id)
                            && matches!(turn.status, ChatTurnStatus::Running)
                    })
            })
    }
}

#[derive(Resource, Default)]
struct InputIntents {
    send: Option<Entity>,
    boundary: Option<Boundary>,
    completion: Option<KeyCode>,
    clear_accepted: Option<PendingDraft>,
    warmup: Option<agentelier_agent_host::ChatWarmupTarget>,
}
#[derive(Component, Clone, Default)]
struct MessageList;
#[derive(Component, Clone, Default)]
struct MessageLabel;
#[derive(Component, Clone, Default)]
struct ReplyWaiting;
#[derive(Component, Clone, Default)]
struct ReplyBody;
#[derive(Component, Clone, PartialEq, Eq, Hash)]
struct ReplyItemKey {
    request_index: u32,
    output_index: usize,
    id: ReplyItemId,
}
#[derive(Clone, PartialEq, Eq, Hash)]
enum ReplyItemId {
    Text(String),
    Image(String),
    Audio {
        call_id: String,
        attachment: SessionId,
    },
    NativeScene(SessionId),
}
#[derive(Component, Clone, Default)]
struct ComposerFeedback;
#[derive(Component, Clone, Copy)]
enum ChatAction {
    NewChat(NewChatTarget),
    Stop,
}
#[derive(Component, Clone, PartialEq, Eq, Hash)]
struct ChatRow {
    request_index: u32,
    session: SessionId,
    turn: u64,
    output_index: Option<usize>,
    item_id: Option<String>,
}

pub(crate) fn install(app: &mut App) {
    crate::project_context::install(app);
    crate::request_inspection::install(app);
    crate::audio_attachment::install(app);
    crate::transcription::install(app);
    crate::queue::install(app);
    composer::install(app);
    crate::composer_media::install(app);
    crate::composer_completion::install(app);
    crate::native_scene::install(app);
    app.init_resource::<SessionUiStates>()
        .init_resource::<ChatState>()
        .init_resource::<InputIntents>()
        .register_type::<ChatState>()
        .add_observer(mount_messages)
        .add_systems(
            PreUpdate,
            capture_input
                .after(bevy::input::InputSystems)
                .before(InputFocusSystems::Dispatch),
        )
        .add_systems(
            Update,
            receive
                .after(ReceiveAgentEvents)
                .after(crate::preparation::receive),
        )
        .add_systems(Update, crate::generated_image::receive.after(receive))
        .add_systems(
            Update,
            crate::generated_image::export::receive.after(receive),
        )
        .add_systems(
            PostUpdate,
            process_input
                .after(EditableTextSystems)
                .before(UiSystems::PostLayout),
        )
        .add_systems(PostUpdate, update.before(UiSystems::Content));
}

pub(crate) fn panel(sources: [Entity; 2]) -> impl Scene {
    bsn! {
        Name("ChatPanel") crate::background_tasks::Bounds
        Node { width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column }
        Children [
            @crate::queue::panel()--
            @conversation_view()--
            @chat_composer(sources)--
            @crate::background_tasks::panel()
        ]
    }
}

fn chat_composer(sources: [Entity; 2]) -> impl Scene {
    bsn! {
        Name("ChatComposerPanel")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(CONTENT_GAP), flex_shrink: 0.0, padding: UiRect::all(px(CONTENT_PADDING)) }
        Children [
            Text::default() WidgetryText ComposerFeedback Name("ChatComposerFeedback")
            TextLayout::linebreak(LineBreak::AnyCharacter) Node { display: Display::None }--
            @crate::audio_attachment::draft_panel()--
            @crate::transcription::panel(crate::transcription::Target::Draft)--
            @crate::transcription::selection_panel()--
            @composer::body()--
            @super::composer_footer::scene(sources)
        ]
    }
}

fn conversation_view() -> impl Scene {
    bsn! {
        @WidgetryScrollArea {
            @scrollbar_visibility: {ScrollbarVisibility {
                vertical: ScrollbarPolicy::Always,
                ..default()
            }},
            @children: {Some(Box::new(bsn_list! {
                MessageList Name("ChatMessages")
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(MESSAGE_GAP) }
            }) as Box<dyn SceneList>)}
        }
        Name("ConversationView") crate::reading::ChatReadingScroll
        Node { width: percent(100), height: px(0), flex_basis: px(0), flex_grow: 1.0, flex_shrink: 1.0, min_height: px(0) }
    }
}

pub(crate) fn new_chat_button(target: NewChatTarget) -> impl Scene {
    let name = match target {
        NewChatTarget::Project(project) => format!("CreateProjectSession-{project}"),
        NewChatTarget::Standalone => "CreateStandaloneSession".into(),
    };
    bsn! {
        @crate::archives::reveal_button(SidebarIcon::Add)
        Name(name) template(move |_| Ok(ChatAction::NewChat(target)))
        on(activate) on(crate::archives::block_row_click)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ChatTarget {
    Session(SessionId),
    Draft(NewChatTarget),
}

#[derive(Resource, Default)]
pub(super) struct SessionUiStates(HashMap<ChatTarget, ChatState>);

impl SessionUiStates {
    pub(super) fn session(&self, id: SessionId) -> Option<&ChatState> {
        self.0.get(&ChatTarget::Session(id))
    }
}

impl ChatState {
    fn target(&self) -> Option<ChatTarget> {
        self.session
            .as_ref()
            .map(|session| ChatTarget::Session(session.id))
            .or_else(|| self.new_chat.map(ChatTarget::Draft))
    }

    fn owns_operation(&self, id: u64) -> bool {
        self.operation == Some(id)
            || self.queue_controls.contains(&id)
            || self.pending_inputs.iter().any(|input| input.id == id)
    }

    fn clear_saved_draft(&mut self, received: PendingDraft) -> bool {
        if self.draft_revision != received.revision {
            return false;
        }
        self.composer = default();
        true
    }

    pub(crate) fn edit_draft(&mut self) -> Result {
        self.draft_revision = self
            .draft_revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("草稿修订已耗尽"))?;
        Ok(())
    }
}

fn switch_view(world: &mut World, view: SessionView) -> Result {
    let next = project(view)?;
    let previous = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|session| session.id);
    if previous != Some(next.id) {
        switch_draft(world, ChatTarget::Session(next.id))?;
        crate::reading::switch(world, previous, Some(next.id));
    }
    let mut state = world.resource_mut::<ChatState>();
    state.new_chat = None;
    state.session = Some(next);
    Ok(())
}

fn switch_draft(world: &mut World, target: ChatTarget) -> Result {
    agentelier_agent_host::flush_scene_state(world);
    composer::read_fields(world)?;
    let mut previous = std::mem::take(&mut *world.resource_mut::<ChatState>());
    if let Some(clear) = world.resource_mut::<InputIntents>().clear_accepted.take() {
        previous.clear_saved_draft(clear);
    }
    let restored = {
        let mut states = world.resource_mut::<SessionUiStates>();
        if let Some(previous_target) = previous.target() {
            states.0.insert(previous_target, previous);
        }
        states.0.remove(&target).unwrap_or_default()
    };
    *world.resource_mut::<ChatState>() = restored;
    composer::rebuild(world, None)
}

fn forget_session(world: &mut World, session: SessionId) -> Result {
    world
        .resource_mut::<SessionUiStates>()
        .0
        .remove(&ChatTarget::Session(session));
    let current = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .is_some_and(|view| view.id == session);
    crate::reading::forget(world, session, current);
    if current {
        *world.resource_mut::<ChatState>() = default();
        composer::rebuild(world, None)?;
    }
    Ok(())
}

pub(super) fn stop_button() -> impl Scene {
    bsn! {
        @AgentelierButton { @style: AgentelierButtonStyle::Danger, @children: bsn_list! { Text("停止") WidgetryText } }
        Name("StopChatGeneration") template(|_| Ok(ChatAction::Stop))
        Node { min_width: px(128), height: px(BUTTON_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0 }
        on(activate)
    }
}

fn activate(event: On<Activate>, actions: Query<&ChatAction>, mut commands: Commands) -> Result {
    match *actions.get(event.entity)? {
        ChatAction::NewChat(target) => {
            commands.queue(move |world: &mut World| open_draft(world, target));
        }
        ChatAction::Stop => commands.queue(stop),
    }
    Ok(())
}

fn open_draft(world: &mut World, target: NewChatTarget) -> Result {
    let state = world.resource::<ChatState>();
    if world.resource::<ArchiveState>().opening.is_some() {
        return Ok(());
    }
    if let NewChatTarget::Project(id) = target
        && crate::projects::project(crate::projects::library(world), id).is_none()
    {
        return Ok(());
    }
    if state.new_chat != Some(target) {
        let previous = state.session.as_ref().map(|session| session.id);
        switch_draft(world, ChatTarget::Draft(target))?;
        crate::reading::switch(world, previous, None);
        let mut state = world.resource_mut::<ChatState>();
        state.session = None;
        state.new_chat = Some(target);
        state.error = None;
        let mut archives = world.resource_mut::<crate::ArchiveState>();
        archives.selected = None;
        archives.error = None;
        crate::archives::sync_selection(world)?;
        reconcile(world)?;
    }
    composer::focus_end(world)
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

pub(crate) fn append_mcp_draft(world: &mut World, material: &str) -> Result {
    let state = world.resource::<ChatState>();
    if state.session.is_none() && state.new_chat.is_none() {
        return Err(io::Error::other("请先打开聊天会话").into());
    }
    composer::append(world, material)
}

fn send(world: &mut World) -> Result {
    if crate::composer_media::importing(world, world.resource::<ChatState>().composer.owner()) {
        world.resource_mut::<ChatState>().input_feedback = Some("正在保存所选附件…".into());
        return Ok(());
    }
    if world
        .resource::<crate::audio_attachment::AudioDraftState>()
        .busy()
    {
        return Ok(());
    }
    if world.resource::<ChatState>().operation.is_some() {
        return steer(world);
    }
    let state = world.resource::<ChatState>();
    if !state.can_send(
        world.resource::<ModelState>(),
        world.resource::<AccountState>(),
    ) {
        return Ok(());
    }
    let mut input = state.composer.submission();
    let submitted = PendingDraft {
        id: 0,
        revision: state.draft_revision,
    };
    let starting_chat = state.new_chat.is_some();
    let request = if let Some(target) = state.new_chat {
        let SessionModelDefaults { slug, effort, fast } =
            super::composer_footer::draft_model(world)?;
        let instructions = Arc::new(crate::prompts::snapshot(world)?);
        let location = match target {
            NewChatTarget::Project(project) => SessionLocation::InProject {
                project,
                directory_override: None,
            },
            NewChatTarget::Standalone => match std::env::current_dir() {
                Ok(directory) => SessionLocation::Standalone(directory),
                Err(failure) => {
                    world.resource_mut::<ChatState>().error =
                        Some(format!("无法读取启动工作目录：{failure}"));
                    return Ok(());
                }
            },
        };
        ConversationRequest::CreateAndSend {
            create: SessionCreate {
                slug,
                effort,
                fast,
                instructions,
                location,
            },
            input,
        }
    } else {
        let session = state
            .session
            .as_ref()
            .ok_or_else(|| io::Error::other("发送消息缺少聊天会话"))?;
        add_draft_materials(world, session, &mut input);
        ConversationRequest::Send {
            session: session.id,
            input,
        }
    };
    let result = world.resource::<AgentHost>().request_conversation(request);
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.pending_inputs.push(PendingDraft { id, ..submitted });
            state.input_feedback = None;
            state.error = None;
            info!(operation_id = id, starting_chat, "开始提交聊天消息");
        }
        Err(failure) => {
            error!(error = %failure, "无法提交聊天消息");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn add_draft_materials(world: &World, session: &ChatSession, input: &mut ComposerSubmission) {
    input.transcript = world
        .resource::<crate::transcription::TranscriptionState>()
        .selection(session);
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
        revision: state.draft_revision,
    };
    let mut input = state.composer.submission();
    let text = state.composer.text();
    add_draft_materials(world, session, &mut input);
    let queue = input.requires_queue();
    let host = world.resource::<AgentHost>();
    let result = if queue {
        host.request_session(SessionAction::Enqueue {
            session: session.id,
            input,
        })
    } else {
        host.steer_conversation(session.id, turn.id, turn.operation_id, text)
    };
    let mut state = world.resource_mut::<ChatState>();
    match result {
        Ok(id) => {
            state.pending_inputs.push(PendingDraft { id, ..submitted });
            if queue {
                state.queue_controls.push(id);
            }
            state.input_feedback = Some(
                if queue {
                    "正在保存排队消息…"
                } else {
                    "正在提交补充输入…"
                }
                .into(),
            );
        }
        Err(failure) => {
            state.input_feedback = Some(format!("补充输入未提交：{failure}；草稿保留"));
        }
    }
    Ok(())
}

fn capture_input(
    world: &mut World,
    mut events: Local<MessageCursor<KeyboardInput>>,
    mut ime: Local<MessageCursor<Ime>>,
    mut image_paste: Local<bool>,
) -> Result {
    let ime_active = ime.read(world.resource::<Messages<Ime>>()).any(|event| {
        matches!(event, Ime::Commit { .. })
            || matches!(event, Ime::Preedit { value, .. } if !value.is_empty())
    });
    let focus = world.resource::<InputFocus>().get();
    let mut fields = world.query_filtered::<
        (Entity, &WidgetryTextFieldHeadless),
        Or<(With<TextSegment>, With<crate::memory_chat::Composer>)>,
    >();
    let root = fields
        .iter(world)
        .find(|(_, field)| Some(field.editor) == focus)
        .map(|(entity, _)| entity);
    let window = root
        .and_then(|entity| world.get::<ComputedUiTargetCamera>(entity)?.get())
        .and_then(|camera| match world.get::<RenderTarget>(camera)? {
            RenderTarget::Window(WindowRef::Entity(window)) => Some(*window),
            _ => None,
        });
    let composing = if let Some(root) = root {
        WidgetryTextFieldHeadless::editor(world, root)
            .map_err(|failure| failure.into_bevy_error())?
            .is_composing()
    } else {
        false
    };
    let pressed = world.resource::<ButtonInput<KeyCode>>();
    let shift = pressed.pressed(KeyCode::ShiftLeft) || pressed.pressed(KeyCode::ShiftRight);
    let control = pressed.pressed(KeyCode::ControlLeft) || pressed.pressed(KeyCode::ControlRight);
    let alt_or_super = [
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]
    .iter()
    .any(|key| pressed.pressed(*key));
    let boundary_candidate = root.filter(|entity| world.get::<TextSegment>(*entity).is_some());
    if control
        && !alt_or_super
        && !shift
        && !ime_active
        && !composing
        && boundary_candidate.is_some()
    {
        return world.resource_scope(|world, mut messages: Mut<Messages<KeyboardInput>>| {
            for event in events.read_mut(&mut messages) {
                if window != Some(event.window) || event.state != ButtonState::Pressed {
                    continue;
                }
                let paste = match &event.logical_key {
                    Key::Character(key) if key.is_ascii() => key.eq_ignore_ascii_case("v"),
                    Key::Character(_) => event.key_code == KeyCode::KeyV,
                    _ => false,
                };
                if !paste {
                    continue;
                }
                if !event.repeat {
                    *image_paste = crate::composer_media::paste_clipboard_image(world)?;
                }
                if *image_paste {
                    event.logical_key = Key::Unidentified(NativeKey::Unidentified);
                    event.text = None;
                }
            }
            Ok(())
        });
    }
    *image_paste = false;
    let mut boundaries = [
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::Backspace,
        KeyCode::Delete,
    ]
    .map(|key| {
        (
            key,
            boundary_candidate.and_then(|root| composer::boundary(world, root, key)),
        )
    });
    let mut boundary = None;
    let mut completion = None;
    let completing = root.is_some_and(|root| crate::composer_completion::active(world, root));
    let mut send = false;
    for event in events.read_mut(&mut world.resource_mut::<Messages<KeyboardInput>>()) {
        if window != Some(event.window) || ime_active || composing || control || alt_or_super {
            continue;
        }
        if completing
            && !shift
            && matches!(
                event.key_code,
                KeyCode::ArrowUp
                    | KeyCode::ArrowDown
                    | KeyCode::Escape
                    | KeyCode::Enter
                    | KeyCode::NumpadEnter
            )
        {
            event.logical_key = Key::Unidentified(NativeKey::Unidentified);
            event.text = None;
            if event.state == ButtonState::Pressed {
                completion = Some(event.key_code);
            }
            continue;
        }
        if !shift
            && event.state == ButtonState::Pressed
            && !event.repeat
            && let Some(action) = boundaries
                .iter_mut()
                .find(|(key, _)| *key == event.key_code)
                .and_then(|(_, action)| action.take())
        {
            event.logical_key = Key::Unidentified(NativeKey::Unidentified);
            event.text = None;
            boundary = Some(action);
            continue;
        }
        if !matches!(event.key_code, KeyCode::Enter | KeyCode::NumpadEnter) {
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
    let mut intents = world.resource_mut::<InputIntents>();
    if send {
        intents.send = root;
    }
    if completion.is_some() {
        intents.completion = completion;
    }
    if boundary.is_some() {
        intents.boundary = boundary;
    }
    Ok(())
}

fn process_input(world: &mut World) -> Result {
    let mut intents = world.resource_mut::<InputIntents>();
    let clear = intents.clear_accepted.take();
    let requested = intents.send.take();
    let boundary = intents.boundary.take();
    let completion = intents.completion.take();
    composer::read_fields(world)?;
    let chat_requested = requested.is_some_and(|root| world.get::<TextSegment>(root).is_some());
    if let Some(clear) = clear
        && world.resource_mut::<ChatState>().clear_saved_draft(clear)
    {
        let restore_focus = composer::anchor(world)?.is_some();
        composer::rebuild(world, None)?;
        if restore_focus {
            composer::focus_end(world)?;
        }
    }
    composer::remember_anchor(world)?;
    composer::synchronize_audio(world)?;
    if let Some(boundary) = boundary {
        composer::apply_boundary(world, boundary)?;
    }
    if let Some(completion) = completion {
        crate::composer_completion::key(world, completion)?;
    }
    crate::composer_completion::update(world)?;
    preconnect_draft(world);
    if chat_requested {
        if requested.is_some_and(|root| crate::composer_completion::active(world, root)) {
            crate::composer_completion::key(world, KeyCode::Enter)?;
        } else {
            send(world)?;
        }
    } else if requested
        .is_some_and(|root| world.get::<crate::memory_chat::Composer>(root).is_some())
    {
        crate::memory_chat::submit(world)?;
    }
    Ok(())
}

fn preconnect_draft(world: &mut World) {
    use agentelier_agent_host::ChatWarmupTarget;
    let state = world.resource::<ChatState>();
    let target = if !state.can_send(
        world.resource::<ModelState>(),
        world.resource::<AccountState>(),
    ) {
        None
    } else if state.new_chat.is_some() {
        super::composer_footer::draft_model(world)
            .ok()
            .map(|model| ChatWarmupTarget::Draft(model.slug))
    } else {
        state
            .session
            .as_ref()
            .map(|session| ChatWarmupTarget::Session(session.id))
    };
    if world.resource::<InputIntents>().warmup == target {
        return;
    }
    world.resource_mut::<InputIntents>().warmup = target.clone();
    if let Some(target) = target
        && let Err(failure) = world.resource::<AgentHost>().preconnect_chat(target)
    {
        tracing::debug!(%failure, "聊天连接预热未提交");
    }
}

fn history_item(item: agentelier_conversation_bridge::HistoryItem) -> ChatHistoryItem {
    match item {
        agentelier_conversation_bridge::HistoryItem::Compaction => ChatHistoryItem::Compaction,
        agentelier_conversation_bridge::HistoryItem::Project { publication } => {
            ChatHistoryItem::Project { publication }
        }
        agentelier_conversation_bridge::HistoryItem::User => ChatHistoryItem::User,
        agentelier_conversation_bridge::HistoryItem::AgentResult => ChatHistoryItem::AgentResult,
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
        result_continuation: turn.result_continuation,
        compaction_continuation: turn.compaction_continuation,
        requests: turn
            .requests
            .into_iter()
            .map(|fact| ChatRequest {
                fast: fact.fast,
                behavior: fact.behavior.map(|material| ChatRequestBehavior {
                    hash: material.hash,
                    text: material.text,
                    input_index: material.input_index,
                }),
                transcript: fact
                    .transcript
                    .map(|(index, source)| (index, source.into())),
                response_accepted: fact.response_accepted,
                additional_tools_in_input: fact.additional_tools_in_input,
                memory: fact.memory.map(|memory| ChatRequestMemory {
                    publication_id: memory.publication_id,
                    hash: memory.hash,
                    text: memory.text,
                    role: memory.role,
                    input_index: memory.input_index,
                }),
                skills: fact.skills,
                sending_started: fact.sending_started,
                response_status: fact.response_status,
                request: fact.request,
                window: fact.window,
                publication: fact.publication,
                items: fact.items.into_iter().map(history_item).collect(),
                base_hashes: fact.base_hashes,
                payload_hash: fact.payload_hash,
                payload_bytes: fact.payload_bytes,
            })
            .collect(),
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
                generated_image: crate::generated_image::Artifact::project(&tool),
                published_audio: tool.published_audio.map(Into::into),
                published_scene: tool.published_scene,
                collaboration: tool.collaboration,
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
        input: turn.submission.text().to_owned(),
        input_elements: turn.submission.elements().to_vec(),
        audio: turn
            .submission
            .audio()
            .cloned()
            .map(agentelier_conversation_bridge::AudioAttachmentView::from)
            .map(Into::into)
            .collect(),
        transcript: turn
            .submission
            .transcript
            .map(agentelier_conversation_bridge::TranscriptReferenceView::from)
            .map(Into::into),
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
pub(super) fn project(session: SessionView) -> Result<ChatSession> {
    let limits = &session.context.limits;
    let basis = if session.context.server_anchor {
        if session.context.estimated_tokens == 0 {
            "最近服务端用量"
        } else {
            "最近服务端用量 + 新增项估算"
        }
    } else {
        "内容粗略估算"
    };
    let context = format!(
        "上下文 {} tokens · 有效上限 {} · 自动阈值 {} · {}",
        session.context.tokens,
        limits
            .effective
            .map_or_else(|| "未知".into(), |value| value.to_string()),
        limits
            .automatic
            .map_or_else(|| "未知".into(), |value| value.to_string()),
        basis
    );
    let usage = session.context.latest_usage.map_or_else(
        || "尚无服务端用量".into(),
        |(input, output, total)| format!("最近请求用量：输入 {input}，输出 {output}，总计 {total}"),
    );
    let context_details = format!(
        "工作窗口 {}；已估算新增 {} tokens；服务端覆盖 {} 项\n{usage}",
        session.context.window,
        session.context.estimated_tokens,
        session.context.covered_items.unwrap_or(0)
    );
    Ok(ChatSession {
        project_publications: session
            .project_publications
            .iter()
            .map(|publication| crate::request_inspection::ChatPublication {
                id: publication.id,
                window: publication.window,
                kind: publication.kind.clone(),
                snapshot: crate::project_context::project_snapshot(publication.snapshot.clone()),
                rendered: publication.rendered.clone(),
            })
            .collect(),
        project_context: crate::project_context::project(*session.project_context),
        context,
        context_details,
        context_limits: session.settings.context_limits.clone(),
        codex_identity: session.codex_identity,
        notice: session.notice,
        execution_blocked: session.execution_blocked,
        mcp_capabilities: session.mcp_capabilities,
        tool_catalog: session.tool_catalog,
        queue: session
            .queue
            .into_iter()
            .map(|input| crate::queue::ChatQueuedInput {
                operation_id: input.operation_id,
                input_id: input.input_id,
                text: input.submission.text().to_owned(),
                transcript: input
                    .submission
                    .transcript
                    .map(agentelier_conversation_bridge::TranscriptReferenceView::from)
                    .map(Into::into),
            })
            .collect(),
        queue_paused: session.queue_paused,
        id: session.id,
        slug: session.settings.slug,
        display_name: session.settings.display_name,
        effort: session.settings.effort,
        fast: session.settings.fast,
        requires_websocket_model: session.requires_websocket_model,
        image_input: session.settings.image_input,
        working_path: session.settings.working_directory.clone(),
        working_directory: bevy_widgetry_path_display::display_path(
            &session.settings.working_directory,
        ),
        instruction_snapshot: serde_json::to_string_pretty(session.settings.instructions.as_ref())?,
        turn: session.turn.map(|turn| project_turn(*turn)).transpose()?,
        audio_draft: session.audio_draft.map(Into::into),
        transcriptions: session.transcriptions.into_iter().map(Into::into).collect(),
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
        if let ConversationEvent::AudioDraft { session, result } = event.kind {
            crate::audio_attachment::receipt(world, event.operation_id, session, result)?;
            continue;
        }
        if let ConversationEvent::AudioTranscription { session, result } = event.kind {
            crate::transcription::receipt(world, event.operation_id, session, result);
            continue;
        }
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

        if let ConversationEvent::MetadataSaved { session, result } = event.kind {
            crate::session_actions::complete(world, event.operation_id, session, result)?;
            continue;
        }
        if let ConversationEvent::Archives(entries) = event.kind {
            crate::archives::catalog(world, entries)?;
            changed = true;
            continue;
        }
        if let ConversationEvent::BackgroundTasks(tasks) = event.kind {
            world.resource_mut::<crate::background_tasks::Tasks>().items = tasks;
            continue;
        }
        if let ConversationEvent::Projects(projects) = event.kind {
            crate::projects::catalog(world, projects)?;
            continue;
        }
        if let ConversationEvent::ProjectSaved(result) = event.kind {
            crate::projects::saved(world, event.operation_id, result);
            continue;
        }
        if let ConversationEvent::SessionModelSaved { session, result } = event.kind {
            changed |= with_target(world, ChatTarget::Session(session), |state| {
                let latest = state
                    .pending_model
                    .as_ref()
                    .is_some_and(|pending| pending.id == event.operation_id);
                if latest {
                    state.pending_model = None;
                }
                match result {
                    Ok(view) => state.session = Some(project(view)?),
                    Err(reason) if latest => state.input_feedback = Some(reason),
                    Err(_) => {}
                }
                Ok(None)
            })?;
            continue;
        }
        if let ConversationEvent::Opened(view) = event.kind {
            let id = view.id;
            let opening = world.resource::<ArchiveState>().opening;
            let target = ChatTarget::Session(id);
            let current = world.resource::<ChatState>().target();
            if event.operation_id == 0 {
                world.resource_mut::<ArchiveState>().finish_startup();
                if current.is_some() {
                    changed |= with_target(world, target, |state| {
                        if state.operation.is_none() {
                            state.session = Some(project(view)?);
                        }
                        Ok(None)
                    })?;
                    continue;
                }
            } else if opening != Some((event.operation_id, id)) {
                continue;
            }
            switch_view(world, view)?;
            let mut archives = world.resource_mut::<ArchiveState>();
            archives.selected = Some(id);
            archives.opening = None;
            archives.error = None;
            crate::archives::reveal_selection(world)?;
            changed = true;
            continue;
        }
        let opening = world.resource::<ArchiveState>().opening;
        let archive_error = match &event.kind {
            ConversationEvent::ArchiveFailed(reason)
                if event.operation_id == 0
                    || opening.is_some_and(|(id, _)| id == event.operation_id) =>
            {
                Some(reason)
            }
            ConversationEvent::Rejected(reason)
                if opening.is_some_and(|(id, _)| id == event.operation_id) =>
            {
                Some(reason)
            }
            _ => None,
        };
        if let Some(reason) = archive_error {
            let mut archives = world.resource_mut::<ArchiveState>();
            if event.operation_id == 0 {
                archives.finish_startup();
            } else {
                archives.opening = None;
            }
            archives.error = Some(reason.clone());
            crate::archives::sync_selection(world)?;
            continue;
        }
        let operation_target = || {
            let current = world.resource::<ChatState>();
            if current.owns_operation(event.operation_id) {
                current.target()
            } else {
                world
                    .resource::<SessionUiStates>()
                    .0
                    .iter()
                    .find_map(|(&target, state)| {
                        state.owns_operation(event.operation_id).then_some(target)
                    })
            }
        };
        let target = match &event.kind {
            ConversationEvent::IdleUpdated(view) | ConversationEvent::Started(view) => {
                Some(ChatTarget::Session(view.id))
            }
            ConversationEvent::Tasks(update) => Some(ChatTarget::Session(update.session)),
            ConversationEvent::InputReceipt(receipt) => Some(ChatTarget::Session(receipt.session)),
            ConversationEvent::Queue(reply) => reply
                .view
                .as_ref()
                .map(|view| ChatTarget::Session(view.id))
                .or_else(operation_target),
            _ => operation_target(),
        };
        let Some(target) = target else {
            continue;
        };
        let terminal = event.kind.is_terminal();
        let created = match &event.kind {
            ConversationEvent::Created(view) | ConversationEvent::CreatedForSend(view) => {
                Some(view.id)
            }
            _ => None,
        };
        match &event.kind {
            ConversationEvent::Created(view) | ConversationEvent::CreatedForSend(view) => {
                info!(
                    operation_id = event.operation_id,
                    session_id = %view.id,
                    "聊天会话已创建"
                )
            }
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
            ConversationEvent::Tasks(_)
            | ConversationEvent::GlobalTasks(_)
            | ConversationEvent::BackgroundTasks(_)
            | ConversationEvent::TaskNotification(_)
            | ConversationEvent::AudioDraft { .. }
            | ConversationEvent::AudioTranscription { .. }
            | ConversationEvent::IdleUpdated(_)
            | ConversationEvent::SceneStateSaved(_)
            | ConversationEvent::Processes(_)
            | ConversationEvent::TaskAction(_)
            | ConversationEvent::Updated(_)
            | ConversationEvent::Archives(_)
            | ConversationEvent::Projects(_)
            | ConversationEvent::ProjectSaved(_)
            | ConversationEvent::SessionModelSaved { .. }
            | ConversationEvent::Opened(_)
            | ConversationEvent::WorkingDirectorySaved(_)
            | ConversationEvent::ArchiveFailed(_)
            | ConversationEvent::InputReceipt(_)
            | ConversationEvent::Queue(_)
            | ConversationEvent::ArchiveCleanup(_)
            | ConversationEvent::Deleted { .. }
            | ConversationEvent::MetadataSaved { .. }
            | ConversationEvent::Started(_) => {}
        }
        let visible = with_target(world, target, |state| {
            let id = event.operation_id;
            let clear = match event.kind {
                ConversationEvent::IdleUpdated(view) => {
                    if state.operation.is_none() {
                        state.session = Some(project(view)?);
                    }
                    None
                }
                ConversationEvent::Tasks(update) => {
                    if state
                        .task_revision
                        .is_none_or(|revision| update.revision >= revision)
                        && let Some(session) = &mut state.session
                    {
                        state.task_revision = Some(update.revision);
                        session.queue_paused = update.queue_paused;
                    }
                    None
                }
                ConversationEvent::Queue(reply) => {
                    state.queue_controls.retain(|operation| *operation != id);
                    crate::queue::receive(state, reply)
                }
                ConversationEvent::Started(view) => {
                    state.session = Some(project(view)?);
                    state.operation = Some(id);
                    state.cancelling = false;
                    state.error = None;
                    state.input_feedback = None;
                    info!(operation_id = id, "排队消息已开始执行");
                    None
                }
                ConversationEvent::InputReceipt(receipt) => {
                    crate::active_input::receipt(state, receipt)
                }
                ConversationEvent::Rejected(reason) => {
                    state.pending_inputs.retain(|input| input.id != id);
                    if state.operation == Some(id) {
                        state.error = Some(reason);
                    } else {
                        state.input_feedback = Some(format!("发送未接纳：{reason}；草稿保留"));
                    }
                    None
                }
                ConversationEvent::Created(view) | ConversationEvent::CreatedForSend(view) => {
                    state.session = Some(project(view)?);
                    state.new_chat = None;
                    state.draft_model = None;
                    state.task_revision = None;
                    None
                }
                ConversationEvent::Accepted(view) => {
                    state.session = Some(project(view)?);
                    state
                        .pending_inputs
                        .iter()
                        .position(|input| input.id == id)
                        .map(|index| state.pending_inputs.remove(index))
                }
                ConversationEvent::Updated(view)
                | ConversationEvent::Finished(view)
                | ConversationEvent::WorkingDirectorySaved(view) => {
                    state.session = Some(project(view)?);
                    None
                }
                _ => None,
            };
            if terminal && state.operation == Some(id) {
                state.cancelling = false;
                state.operation = None;
            }
            Ok(clear)
        })?;
        if visible && let Some(session) = created {
            world.resource_mut::<ArchiveState>().selected = Some(session);
        }
        if terminal || created.is_some() {
            refresh_archives(world);
        }
        changed |= visible;
    }
    if let Some(reason) = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|failure| failure.reason.clone())
    {
        world
            .resource_mut::<crate::audio_attachment::AudioDraftState>()
            .host_failed(&reason);
        world
            .resource_mut::<crate::transcription::TranscriptionState>()
            .host_failed(&reason);
        let mut archives = world.resource_mut::<ArchiveState>();
        archives.finish_startup();
        archives.opening = None;
        archives.error = Some(reason.clone());
        for state in world.resource_mut::<SessionUiStates>().0.values_mut() {
            state.operation = None;
            state.pending_inputs.clear();
            state.pending_model = None;
            state.queue_controls.clear();
            state.cancelling = false;
            state.error = Some(reason.clone());
        }
        crate::session_actions::failed(world, &reason);
        crate::projects::failed(world, &reason);
        let mut state = world.resource_mut::<ChatState>();
        state.operation = None;
        state.pending_inputs.clear();
        state.pending_model = None;
        state.queue_controls.clear();
        state.cancelling = false;
        state.error = Some(reason);
        changed = true;
    }
    if changed {
        reconcile(world)?;
    }
    Ok(())
}

fn with_target(
    world: &mut World,
    target: ChatTarget,
    update: impl FnOnce(&mut ChatState) -> Result<Option<PendingDraft>>,
) -> Result<bool> {
    if world.resource::<ChatState>().target() == Some(target) {
        let clear = update(&mut world.resource_mut::<ChatState>())?;
        if let Some(clear) = clear {
            world.resource_mut::<InputIntents>().clear_accepted = Some(clear);
        }
        return Ok(true);
    }
    let mut states = world.resource_mut::<SessionUiStates>();
    if let Some(mut state) = states.0.remove(&target) {
        let result = update(&mut state);
        if let Ok(Some(clear)) = &result {
            state.clear_saved_draft(clear.clone());
        }
        states.0.insert(state.target().unwrap_or(target), state);
        result?;
    }
    Ok(false)
}

pub(crate) fn apply_legacy_audio(
    world: &mut World,
    session: SessionId,
    audio: Option<agentelier_conversation_bridge::AudioAttachmentView>,
) -> Result {
    with_target(world, ChatTarget::Session(session), |state| {
        if let Some(session) = &mut state.session {
            session.audio_draft = audio.map(Into::into);
        }
        Ok(None)
    })?;
    Ok(())
}

pub(crate) fn apply_draft_assets(
    world: &mut World,
    placement: crate::composer_media::Placement,
    assets: Vec<ComposerContent>,
    errors: Vec<String>,
) -> Result {
    composer::read_fields(world)?;
    let apply = |state: &mut ChatState| -> Result<Option<composer::Anchor>> {
        let advice = assets
            .iter()
            .any(|asset| matches!(asset, ComposerContent::Audio { .. }));
        let mut errors = errors;
        let mut focus = None;
        if !assets.is_empty() {
            if state.draft_revision == placement.revision {
                focus = Some(state.composer.insert_assets(placement.anchor, assets)?);
                state.edit_draft()?;
            } else {
                errors.push("附件插入位置已变化，请重新选择插入点".into());
            }
        }
        state.input_feedback = if errors.is_empty() {
            advice.then(|| "音频随消息保存，发送后可转写并引用文稿。".into())
        } else {
            Some(errors.join("\n"))
        };
        Ok(focus)
    };
    if world.resource::<ChatState>().composer.owner() == placement.owner {
        let focus = apply(&mut world.resource_mut::<ChatState>())?;
        if focus.is_some() {
            composer::rebuild(world, focus)?;
        }
    } else if let Some(state) = world
        .resource_mut::<SessionUiStates>()
        .0
        .values_mut()
        .find(|state| state.composer.owner() == placement.owner)
    {
        apply(state)?;
    }
    Ok(())
}

fn mount_messages(_event: On<Add<MessageList>>, mut commands: Commands) {
    commands.queue(reconcile);
}

fn refresh_archives(world: &mut World) {
    if let Err(failure) = world.resource::<AgentHost>().refresh_archives() {
        world.resource_mut::<crate::ArchiveState>().error = Some(failure.to_string());
    }
}

enum RowContent {
    Plain(String),
    User(String),
    Reply(ReplyContent),
    MemoryMessage { label: String, source: String },
    Audio(crate::audio_attachment::ChatAudio),
    InputImage(agentelier_conversation_bridge::InputImage),
}

struct ReplyContent {
    activity: crate::reply_activity::Presentation,
    items: Vec<(ReplyItemKey, ReplyItemContent)>,
}

enum ReplyItemContent {
    Text(String),
    Image(crate::generated_image::Artifact),
    Audio(crate::audio_attachment::ChatAudio),
    NativeScene(agentelier_conversation_bridge::PublishedScene),
}

pub(super) enum TranscriptPresentation {
    Chat { operation: Option<u64> },
    Memory,
}

fn public_reply(turn: &ChatTurn, message: &ChatMessage) -> bool {
    match message.phase.as_deref() {
        Some("commentary" | "final_answer") => true,
        Some(_) => false,
        None => {
            matches!(turn.status, ChatTurnStatus::Completed)
                && message.request_index == turn.request_index
                && !turn
                    .tools
                    .iter()
                    .any(|tool| tool.request_index == message.request_index)
        }
    }
}

fn reply_content(turn: &ChatTurn, operation: Option<u64>) -> Option<ReplyContent> {
    let mut items: Vec<_> = turn
        .messages
        .iter()
        .filter(|message| public_reply(turn, message) && !message.text.trim().is_empty())
        .map(|message| {
            (
                ReplyItemKey {
                    request_index: message.request_index,
                    output_index: message.output_index,
                    id: ReplyItemId::Text(message.id.clone()),
                },
                ReplyItemContent::Text(message.text.clone()),
            )
        })
        .collect();
    items.extend(turn.tools.iter().filter_map(|tool| {
        let audio = tool.published_audio.clone()?;
        Some((
            ReplyItemKey {
                request_index: tool.request_index,
                output_index: tool.output_index,
                id: ReplyItemId::Audio {
                    call_id: tool.call_id.clone(),
                    attachment: audio.id,
                },
            },
            ReplyItemContent::Audio(audio),
        ))
    }));
    items.extend(turn.tools.iter().filter_map(|tool| {
        let artifact = tool.published_scene.clone()?;
        Some((
            ReplyItemKey {
                request_index: tool.request_index,
                output_index: tool.output_index,
                id: ReplyItemId::NativeScene(artifact.id),
            },
            ReplyItemContent::NativeScene(artifact),
        ))
    }));
    let public = !items.is_empty()
        || turn.tools.iter().any(|tool| {
            tool.generated_image.as_ref().is_some_and(|artifact| {
                !matches!(artifact, crate::generated_image::Artifact::Generating)
            })
        });
    if public {
        items.extend(turn.tools.iter().filter_map(|tool| {
            let artifact = tool.generated_image.clone()?;
            Some((
                ReplyItemKey {
                    request_index: tool.request_index,
                    output_index: tool.output_index,
                    id: ReplyItemId::Image(tool.call_id.clone()),
                },
                ReplyItemContent::Image(artifact),
            ))
        }));
    }
    let running =
        operation == Some(turn.operation_id) && matches!(turn.status, ChatTurnStatus::Running);
    if items.is_empty() && turn.tools.is_empty() && !running {
        return None;
    }
    items.sort_by_key(|(key, _)| (key.request_index, key.output_index));
    Some(ReplyContent {
        activity: crate::reply_activity::Presentation::new(turn, running),
        items,
    })
}

fn reconcile(world: &mut World) -> Result {
    crate::queue::reconcile(world)?;
    let state = world.resource::<ChatState>().clone();
    let mut lists = world.query_filtered::<Entity, With<MessageList>>();
    let lists: Vec<_> = lists.iter(world).collect();
    for list in lists {
        reconcile_list(
            world,
            list,
            state.session.as_ref(),
            TranscriptPresentation::Chat {
                operation: state.operation,
            },
            Color::NONE,
        )?;
    }
    Ok(())
}

pub(super) fn reconcile_list(
    world: &mut World,
    list: Entity,
    session: Option<&ChatSession>,
    presentation: TranscriptPresentation,
    message_surface: Color,
) -> Result {
    let mut rows = world.query::<(Entity, &ChatRow, &ChildOf)>();
    let mut existing: HashMap<_, _> = rows
        .iter(world)
        .filter(|(_, _, parent)| parent.parent() == list)
        .map(|(entity, row, _)| (row.clone(), entity))
        .collect();
    let mut desired: Vec<(ChatRow, RowContent)> = Vec::new();
    let memory = matches!(presentation, TranscriptPresentation::Memory);
    if let Some(session) = session {
        let mut inputs = HashSet::new();
        for turn in session.previous_turns.iter().chain(session.turn.iter()) {
            let user_key = ChatRow {
                session: session.id,
                turn: turn.input_id,
                request_index: 0,
                output_index: None,
                item_id: None,
            };
            if !turn.result_continuation && inputs.insert(turn.input_id) {
                if turn.input_elements.iter().any(|item| {
                    matches!(
                        item.content,
                        ComposerContent::Audio { .. } | ComposerContent::Image { .. }
                    )
                }) {
                    for element in &turn.input_elements {
                        let key = ChatRow {
                            item_id: Some(element.id.to_string()),
                            ..user_key.clone()
                        };
                        let content = match &element.content {
                            ComposerContent::Text { text } if !text.is_empty() => {
                                RowContent::User(text.clone())
                            }
                            ComposerContent::Text { .. } => continue,
                            ComposerContent::Image { image } => {
                                RowContent::InputImage(image.clone())
                            }
                            ComposerContent::Audio { audio } => RowContent::Audio(
                                agentelier_conversation_bridge::AudioAttachmentView::from(
                                    audio.clone(),
                                )
                                .into(),
                            ),
                            ComposerContent::File { reference } => {
                                RowContent::User(reference.display())
                            }
                            ComposerContent::Skill { name, .. } => {
                                RowContent::User(format!("${name}"))
                            }
                            ComposerContent::Mcp { label, .. } => {
                                RowContent::User(format!("${label}"))
                            }
                        };
                        desired.push((key, content));
                    }
                } else {
                    desired.push((user_key.clone(), RowContent::User(turn.input.clone())));
                }
                if let Some(source) = &turn.transcript {
                    let key = ChatRow {
                        item_id: Some(format!("transcript-{}", source.transcript)),
                        ..user_key
                    };
                    desired.push((
                        key,
                        RowContent::Plain(format!("引用文稿 · {}", source.label())),
                    ));
                }
            }
            if memory
                && turn.messages.is_empty()
                && turn.tools.iter().all(|tool| tool.collaboration)
            {
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
            if memory {
                output.extend(turn.messages.iter().map(|message| {
                    (
                        ChatRow {
                            session: session.id,
                            turn: turn.id,
                            request_index: message.request_index,
                            output_index: Some(message.output_index),
                            item_id: Some(message.id.clone()),
                        },
                        RowContent::MemoryMessage {
                            label: memory_message_label(turn, message),
                            source: message.text.clone(),
                        },
                    )
                }));
            } else if let TranscriptPresentation::Chat { operation } = presentation
                && let Some(content) = reply_content(turn, operation)
            {
                // A Turn owns one reply position before its supplemental user rows.
                desired.push((
                    ChatRow {
                        session: session.id,
                        turn: turn.id,
                        request_index: 0,
                        output_index: None,
                        item_id: Some("reply".into()),
                    },
                    RowContent::Reply(content),
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
                    RowContent::User(format!("你 · 补充 {} · {label}\n{}", input.id, input.text)),
                ));
            }
            output.sort_by_key(|(key, _)| (key.request_index, key.output_index));
            desired.extend(output);
        }
    }
    let mut ordered = Vec::with_capacity(desired.len());
    for (key, content) in desired {
        if let Some(entity) = existing.remove(&key) {
            ordered.push(entity);
            match content {
                RowContent::Audio(_) | RowContent::InputImage(_) => {}
                RowContent::Plain(text) => {
                    world
                        .get_mut::<Text>(entity)
                        .ok_or_else(|| io::Error::other("聊天消息缺少正文"))?
                        .set_if_neq(Text(text));
                }
                RowContent::User(text) => {
                    let body = world
                        .get::<Children>(entity)
                        .and_then(|children| {
                            children
                                .iter()
                                .find(|child| world.get::<Text>(*child).is_some())
                        })
                        .ok_or_else(|| io::Error::other("用户消息缺少正文"))?;
                    world
                        .get_mut::<Text>(body)
                        .ok_or_else(|| io::Error::other("用户消息正文不可用"))?
                        .set_if_neq(Text(text));
                }
                RowContent::Reply(content) => reconcile_reply(world, entity, key.session, content)?,
                RowContent::MemoryMessage { label, source } => {
                    let children = world
                        .get::<Children>(entity)
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
                    RowContent::InputImage(image) => {
                        let owner = agentelier_agent_host::ComposerAssetOwner::Session(key.session);
                        let label = format!("你 · {}", image.name);
                        world.commands().spawn_scene_with_error_handler(bsn! {
                            Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                            Node { width: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::End, row_gap: px(CONTENT_GAP) }
                            Children [Text(label) WidgetryText-- @crate::composer_media::image(owner, image)]
                        }).id()
                    }
                    RowContent::Audio(audio) => world.commands().spawn_scene_with_error_handler(bsn! {
                        @crate::audio_attachment::card(key.session, audio)
                        Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                    }).id(),
                    RowContent::Reply(content) => {
                        let session = key.session;
                        let mut commands = world.commands();
                        let entity = commands.spawn_scene_with_error_handler(bsn! {
                            Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                            BackgroundColor(message_surface)
                            Node {width:percent(100),min_width:px(0),flex_shrink:0.0,flex_direction:FlexDirection::Column,row_gap:px(CONTENT_GAP),padding:UiRect::all(px(CONTENT_PADDING))}
                            Children [
                                @AgentelierAvatar { @source: AvatarSource::Assistant } Name("ChatAssistantAvatar") Node { align_self: AlignSelf::Start }--
                                @crate::spinner::ring(REPLY_SPINNER_SIZE) ReplyWaiting Name("ChatReplyWaiting")
                                Node { margin: UiRect::vertical(px(4)) }--
                                @crate::reply_activity::panel()--
                                ReplyBody Name("ChatReplyBody")
                                Node {width:percent(100),min_width:px(0),flex_direction:FlexDirection::Column,row_gap:px(CONTENT_GAP)}
                            ]
                        }).id();
                        commands.queue(move |world: &mut World| reconcile_reply(world, entity, session, content));
                        entity
                    }
                    RowContent::MemoryMessage {label,source} => world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                        BackgroundColor(message_surface)
                        Node {width:percent(100),min_width:px(0),flex_shrink:0.0,flex_direction:FlexDirection::Column,row_gap:px(CONTENT_GAP),padding:UiRect::all(px(CONTENT_PADDING))}
                        Children [
                            @AgentelierAvatar { @source: AvatarSource::Assistant } Name("ChatAssistantAvatar") Node { align_self: AlignSelf::Start }--
                            Text(label) WidgetryText MessageLabel--
                            @MarkdownView { @source: source, @source_controls: true }
                        ]
                    }).id(),
                    RowContent::User(text) => world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) template(move |_| Ok(key.clone()))
                        BackgroundColor(message_surface)
                        Node {width:percent(100),min_width:px(0),flex_shrink:0.0,flex_direction:FlexDirection::Column,row_gap:px(CONTENT_GAP),padding:UiRect::all(px(CONTENT_PADDING))}
                        Children [
                            @AgentelierAvatar Name("ChatUserAvatar") Node { align_self: AlignSelf::End }--
                            Text(text) WidgetryText Name("ChatUserMessageBody")
                            TextLayout { justify: Justify::Right, linebreak: LineBreak::AnyCharacter }
                            Node { width: percent(100), min_width: px(0) }
                        ]
                    }).id(),
                    RowContent::Plain(text) => world.commands().spawn_scene_with_error_handler(bsn! {
                        Name(name) ChildOf(list) Text(text) WidgetryText template(move |_| Ok(key.clone()))
                        BackgroundColor(message_surface)
                        TextLayout::linebreak(LineBreak::AnyCharacter)
                        Node {width:percent(100),min_width:px(0),flex_shrink:0.0,padding:UiRect::all(px(CONTENT_PADDING))}
                    }).id(),
                };
            ordered.push(entity);
        }
    }
    for entity in existing.into_values() {
        world.entity_mut(entity).despawn();
    }
    order_children(world, list, &ordered);
    Ok(())
}

fn reconcile_reply(
    world: &mut World,
    root: Entity,
    session: SessionId,
    content: ReplyContent,
) -> Result {
    let children = world
        .get::<Children>(root)
        .ok_or_else(|| io::Error::other("助手回复缺少子实体"))?;
    let waiting = children
        .iter()
        .find(|child| world.get::<ReplyWaiting>(*child).is_some())
        .ok_or_else(|| io::Error::other("助手回复缺少等待控件"))?;
    let body = children
        .iter()
        .find(|child| world.get::<ReplyBody>(*child).is_some())
        .ok_or_else(|| io::Error::other("助手回复缺少内容区域"))?;
    let ReplyContent { activity, items } = content;
    let is_waiting = activity.running && !activity.has_tools && items.is_empty();
    for (entity, visible) in [(waiting, is_waiting), (body, !items.is_empty())] {
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        world
            .get_mut::<Node>(entity)
            .ok_or_else(|| io::Error::other("助手回复布局不可用"))?
            .map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
    }
    crate::reply_activity::reconcile(world, root, activity)?;
    let mut existing: HashMap<_, _> = world
        .get::<Children>(body)
        .into_iter()
        .flat_map(|children| children.iter())
        .filter_map(|child| {
            world
                .get::<ReplyItemKey>(child)
                .map(|key| (key.clone(), child))
        })
        .collect();
    let mut ordered = Vec::with_capacity(items.len());
    for (key, item) in items {
        let entity = if let Some(entity) = existing.remove(&key) {
            match item {
                ReplyItemContent::Text(source) => {
                    world
                        .get_mut::<MarkdownSource>(entity)
                        .ok_or_else(|| io::Error::other("助手回复正文不可用"))?
                        .set_if_neq(MarkdownSource(source));
                }
                ReplyItemContent::Image(artifact) => {
                    crate::generated_image::reconcile(world, entity, artifact)?
                }
                ReplyItemContent::Audio(_) => {}
                ReplyItemContent::NativeScene(artifact) => {
                    if crate::native_scene::needs_revision(world, entity, artifact.revision) {
                        world.commands().queue(move |world: &mut World| {
                            crate::native_scene::reconcile(world, entity, session, artifact)
                        });
                    }
                }
            }
            entity
        } else {
            match item {
                ReplyItemContent::Text(source) => world
                    .commands()
                    .spawn_scene_with_error_handler(bsn! {
                        @MarkdownView { @source: source, @source_controls: false }
                        ChildOf(body) Name("ChatReplyText") template(move |_| Ok(key.clone()))
                    })
                    .id(),
                ReplyItemContent::Image(artifact) => {
                    let ReplyItemId::Image(call_id) = &key.id else {
                        return Err(io::Error::other("助手回复图片缺少调用标识").into());
                    };
                    let scene = crate::generated_image::card(session, call_id.clone());
                    let mut commands = world.commands();
                    let entity = commands
                        .spawn_scene_with_error_handler(bsn! {
                            @scene ChildOf(body) template(move |_| Ok(key.clone()))
                        })
                        .id();
                    commands.queue(move |world: &mut World| {
                        crate::generated_image::reconcile(world, entity, artifact)
                    });
                    entity
                }
                ReplyItemContent::Audio(audio) => world
                    .commands()
                    .spawn_scene_with_error_handler(bsn! {
                        @crate::audio_attachment::published_card(session, audio)
                        ChildOf(body) template(move |_| Ok(key.clone()))
                    })
                    .id(),
                ReplyItemContent::NativeScene(artifact) => {
                    let scene = crate::native_scene::card(artifact.id);
                    let mut commands = world.commands();
                    let entity = commands
                        .spawn_scene_with_error_handler(bsn! {
                            @scene ChildOf(body) template(move |_| Ok(key.clone()))
                        })
                        .id();
                    commands.queue(move |world: &mut World| {
                        crate::native_scene::reconcile(world, entity, session, artifact)
                    });
                    entity
                }
            }
        };
        ordered.push(entity);
    }
    for entity in existing.into_values() {
        world.entity_mut(entity).despawn();
    }
    order_children(world, body, &ordered);
    Ok(())
}

fn order_children(world: &mut World, parent: Entity, ordered: &[Entity]) {
    let current = world
        .get::<Children>(parent)
        .into_iter()
        .flat_map(|children| children.iter());
    if !current.eq(ordered.iter().copied()) {
        world.commands().entity(parent).replace_children(ordered);
    }
}

fn memory_message_label(turn: &ChatTurn, message: &ChatMessage) -> String {
    let accepted = turn
        .requests
        .iter()
        .find(|fact| fact.request == message.request_index)
        .and_then(|fact| fact.response_accepted)
        .unwrap_or(message.request_index <= turn.committed_steps);
    let retained = if accepted {
        "已保留"
    } else if matches!(turn.status, ChatTurnStatus::Running) {
        "接收中"
    } else {
        "未纳入后续上下文"
    };
    format!(
        "助手 · {} · 请求 {} · {retained}",
        message.phase.as_deref().unwrap_or("回答"),
        message.request_index
    )
}

fn composer_feedback(state: &ChatState, model: &ModelState, account: &AccountState) -> String {
    if let Some(error) = &state.error {
        return error.clone();
    }
    if let Some(feedback) = &state.input_feedback {
        return feedback.clone();
    }
    if state.new_chat.is_some() {
        if let Some(error) = &model.catalog_error {
            return format!("无法读取模型目录：{error}");
        }
        if state.operation.is_some() {
            return "正在创建会话…".into();
        }
    }
    let Some(session) = &state.session else {
        return String::new();
    };
    if session.binding().is_some()
        && account.model_identity().is_some()
        && !session.matches_account(account)
    {
        return "此会话绑定了其他 Codex 账号或工作区。历史可查看，请新建会话继续聊天。".into();
    }
    if let Some(turn) = &session.turn {
        match &turn.status {
            ChatTurnStatus::Failed(reason) => return format!("本轮失败：{reason}"),
            ChatTurnStatus::Limited => {
                return format!("任务触限，已保留 {} 个完整步骤", turn.committed_steps);
            }
            _ => {}
        }
    }
    if let Some(notice) = &session.notice {
        return notice.clone();
    }
    if session
        .project_context
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.sources.iter().any(|source| source.truncated))
    {
        return "项目规则仅载入预览".into();
    }
    String::new()
}
fn update(
    state: Res<ChatState>,
    model: Res<ModelState>,
    account: Res<AccountState>,
    archives: Res<ArchiveState>,
    new_buttons: Query<(), Added<ChatAction>>,
    mut buttons: Query<(Entity, &ChatAction, Has<InteractionDisabled>, &mut Node)>,
    mut feedback: Query<(&mut Text, &mut Node), (With<ComposerFeedback>, Without<ChatAction>)>,
    mut commands: Commands,
) {
    if !state.is_changed()
        && !model.is_changed()
        && !account.is_changed()
        && !archives.is_changed()
        && new_buttons.is_empty()
    {
        return;
    }
    let running = state
        .session
        .as_ref()
        .and_then(|session| session.turn.as_ref())
        .is_some_and(|turn| {
            state.operation == Some(turn.operation_id)
                && matches!(turn.status, ChatTurnStatus::Running)
        });
    for (entity, action, disabled, mut node) in &mut buttons {
        if matches!(action, ChatAction::Stop) {
            let display = if running {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
            AgentelierButtonLoading::set(&mut commands, entity, state.cancelling);
        }
        let enabled = match action {
            ChatAction::NewChat(_) => archives.opening.is_none(),
            ChatAction::Stop => !state.cancelling && running,
        };
        if enabled && disabled {
            commands.entity(entity).try_remove::<InteractionDisabled>();
        } else if !enabled && !disabled {
            commands.entity(entity).try_insert(InteractionDisabled);
        }
    }
    let message = composer_feedback(&state, &model, &account);
    let display = if message.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    for (mut text, node) in &mut feedback {
        text.set_if_neq(Text(message.clone()));
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
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
    text.push_str(&format!(
        "{}\n{}\n",
        session.context, session.context_details
    ));
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
        for request in &turn.requests {
            text.push_str(&format!(
                "\n已准备请求 {} · 窗口 {} · 项目发布 {:?} · {} 字节 · SHA-256 {}\n",
                request.request,
                request.window,
                request.publication,
                request.payload_bytes,
                request.payload_hash
            ));
        }
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
