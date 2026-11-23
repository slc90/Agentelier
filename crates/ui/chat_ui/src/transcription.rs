use crate::conversation::{ChatSession, ChatState};
use agentelier_agent_host::AgentHost;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{
    SessionId, TranscriptReferenceView, TranscriptionOutcomeView, TranscriptionView,
};
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};
use std::collections::HashMap;
use std::io;

const TRANSCRIPT_MIN_ROWS: usize = 3;
const TRANSCRIPT_MAX_ROWS: usize = 7;

#[derive(Clone, Reflect)]
pub struct ChatTranscription {
    pub id: SessionId,
    pub audio: SessionId,
    pub digest: String,
    pub engine: String,
    pub outcome: ChatOutcome,
}

#[derive(Clone, Reflect)]
pub enum ChatOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
    Interrupted,
}

#[derive(Clone, Reflect, PartialEq, Eq)]
pub struct ChatTranscriptReference {
    pub session: SessionId,
    pub audio: SessionId,
    pub transcript: SessionId,
    pub filename: String,
    pub engine: String,
    pub audio_sha256: String,
    pub text_sha256: String,
}

impl From<TranscriptReferenceView> for ChatTranscriptReference {
    fn from(value: TranscriptReferenceView) -> Self {
        Self {
            session: value.session,
            audio: value.audio,
            transcript: value.transcript,
            filename: value.filename,
            engine: value.engine,
            audio_sha256: value.audio_sha256,
            text_sha256: value.text_sha256,
        }
    }
}

impl ChatTranscriptReference {
    pub(crate) fn label(&self) -> String {
        source_label(&self.filename, &self.engine, self.transcript)
    }
}

impl From<TranscriptionView> for ChatTranscription {
    fn from(value: TranscriptionView) -> Self {
        Self {
            id: value.id,
            audio: value.audio,
            digest: value.digest,
            engine: value.engine.into(),
            outcome: match value.outcome {
                TranscriptionOutcomeView::Completed(text) => ChatOutcome::Completed(text),
                TranscriptionOutcomeView::Failed(error) => ChatOutcome::Failed(error),
                TranscriptionOutcomeView::Cancelled => ChatOutcome::Cancelled,
                TranscriptionOutcomeView::Interrupted => ChatOutcome::Interrupted,
            },
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct TranscriptionState {
    pending: Option<Pending>,
    feedback: Option<(SessionId, SessionId, String)>,
    selected: HashMap<SessionId, Option<SessionId>>,
}

#[derive(Clone, Copy)]
struct Pending {
    id: u64,
    session: SessionId,
    audio: SessionId,
    cancelling: bool,
}

impl TranscriptionState {
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn selection(&self, session: &ChatSession) -> Option<SessionId> {
        let id = self.selected.get(&session.id).copied().unwrap_or_else(|| {
            session
                .turn
                .iter()
                .chain(session.previous_turns.iter().rev())
                .find(|turn| !turn.result_continuation)
                .and_then(|turn| turn.transcript.as_ref().map(|source| source.transcript))
        })?;
        let turns = || session.turn.iter().chain(&session.previous_turns);
        let available = turns()
            .filter_map(|turn| turn.transcript.as_ref())
            .any(|source| source.transcript == id)
            || session
                .transcriptions
                .iter()
                .find(|entry| entry.id == id)
                .is_some_and(|entry| {
                    session
                        .audio_draft
                        .iter()
                        .chain(turns().filter_map(|turn| turn.audio.as_ref()))
                        .any(|audio| audio.id == entry.audio)
                });
        available.then_some(id)
    }

    pub(crate) fn host_failed(&mut self, failure: &str) {
        if let Some(pending) = self.pending.take() {
            self.feedback = Some((
                pending.session,
                pending.audio,
                format!("转写结果未确认：{failure}"),
            ));
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Target {
    Draft,
    Saved(SessionId),
}

impl Target {
    fn audio(self, session: &ChatSession) -> Option<SessionId> {
        match self {
            Self::Draft => session.audio_draft.as_ref().map(|audio| audio.id),
            Self::Saved(id) => Some(id),
        }
    }
}

#[derive(Component, Clone)]
struct Panel {
    target: Target,
    expanded: bool,
    displayed: Option<SessionId>,
}

#[derive(Component, Clone, Default)]
struct Summary;
#[derive(Component, Clone, Default)]
struct TranscriptText;
#[derive(Component, Clone, Default)]
struct SelectionPanel;
#[derive(Component, Clone, Default)]
struct SelectionLabel;

#[derive(Component, Clone, Copy)]
enum Action {
    Start,
    Cancel,
    View,
    Copy,
    Ask,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<TranscriptionState>()
        .add_systems(Update, synchronize);
}

pub(crate) fn panel(target: Target) -> impl Scene {
    bsn! {
        template(move |_| Ok(Panel { target, expanded: false, displayed: None }))
        Node { display: Display::None, width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(8), row_gap: px(4), flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center }
            Children [
                @button("转写音频", "TranscribeAudio", Action::Start)--
                @button("取消转写", "CancelTranscription", Action::Cancel)--
                @button("查看文稿", "ViewTranscript", Action::View)--
                @button("复制文稿", "CopyTranscript", Action::Copy)--
                @button("用文稿提问", "AskAboutTranscript", Action::Ask)--
                Text::default() WidgetryText Summary Name("TranscriptionStatus")
            ]--
            @WidgetryReadOnlyTextField {
                @multiline: true,
                @auto_size: {Some(AutoSize {min_rows:TRANSCRIPT_MIN_ROWS, max_rows:TRANSCRIPT_MAX_ROWS})},
            }
            TranscriptText Name("TranscriptText") Node { display: Display::None, width: percent(100), min_width: px(0) }
        ]
    }
}

pub(crate) fn selection_panel() -> impl Scene {
    bsn! {
        SelectionPanel Name("TranscriptSelection")
        Node { display: Display::None, min_width: px(0), column_gap: px(8), flex_shrink: 0.0, align_items: AlignItems::Center }
        Children [
            Text::default() WidgetryText SelectionLabel Name("SelectedTranscriptLabel")--
            @AgentelierButton { @children: {Some(Box::new(bsn_list!{Text("清除文稿引用") WidgetryText}) as Box<dyn SceneList>)} }
            Name("ClearTranscriptSelection") on(clear)
        ]
    }
}

fn clear(_: On<Activate>, chat: Res<ChatState>, mut state: ResMut<TranscriptionState>) {
    if let Some(session) = &chat.session {
        state.selected.insert(session.id, None);
    }
}

fn button(label: &'static str, name: &'static str, action: Action) -> impl Scene {
    bsn! {
        @AgentelierButton { @children: {Some(Box::new(bsn_list! {Text(label) WidgetryText}) as Box<dyn SceneList>)} }
        template(move |_| Ok(action)) Name(name) Node { display: Display::None } on(activate)
    }
}

fn latest(session: &ChatSession, audio: SessionId) -> Option<(&ChatTranscription, &str)> {
    session
        .transcriptions
        .iter()
        .rev()
        .find_map(|entry| match &entry.outcome {
            ChatOutcome::Completed(text) if entry.audio == audio => Some((entry, text.as_str())),
            _ => None,
        })
}

fn activate(event: On<Activate>, mut commands: Commands) {
    let button = event.entity;
    commands.queue(move |world: &mut World| invoke(world, button));
}

fn invoke(world: &mut World, button: Entity) -> Result {
    let action = *world
        .get::<Action>(button)
        .ok_or_else(|| io::Error::other("转写按钮缺少动作"))?;
    let row = world
        .get::<ChildOf>(button)
        .ok_or_else(|| io::Error::other("转写按钮缺少容器"))?
        .parent();
    let panel = world
        .get::<ChildOf>(row)
        .ok_or_else(|| io::Error::other("转写按钮缺少面板"))?
        .parent();
    let target = world
        .get::<Panel>(panel)
        .ok_or_else(|| io::Error::other("转写面板缺少状态"))?
        .target;
    let state = world.resource::<ChatState>();
    let Some(session) = state.session.as_ref() else {
        return Ok(());
    };
    let Some(audio) = target.audio(session) else {
        return Ok(());
    };
    let session_id = session.id;
    let pending = world.resource::<TranscriptionState>().pending;
    match action {
        Action::Start => {
            if pending.is_some()
                || world
                    .resource::<crate::audio_attachment::AudioDraftState>()
                    .busy()
            {
                return Ok(());
            }
            let result = world
                .resource::<AgentHost>()
                .transcribe_audio(session_id, audio);
            let mut state = world.resource_mut::<TranscriptionState>();
            state.feedback = None;
            match result {
                Ok(id) => {
                    state.pending = Some(Pending {
                        id,
                        session: session_id,
                        audio,
                        cancelling: false,
                    })
                }
                Err(failure) => {
                    state.feedback = Some((session_id, audio, format!("无法开始转写：{failure}")))
                }
            }
        }
        Action::Cancel => {
            if let Some(pending) = pending.filter(|item| {
                item.session == session_id && item.audio == audio && !item.cancelling
            }) {
                match world
                    .resource::<AgentHost>()
                    .cancel_transcription(session_id, pending.id)
                {
                    Ok(true) => {
                        world.resource_mut::<TranscriptionState>().pending = Some(Pending {
                            cancelling: true,
                            ..pending
                        })
                    }
                    Ok(false) => {}
                    Err(failure) => {
                        world.resource_mut::<TranscriptionState>().feedback =
                            Some((session_id, audio, format!("无法取消转写：{failure}")))
                    }
                }
            }
        }
        Action::View => {
            if let Some(mut panel) = world.get_mut::<Panel>(panel) {
                panel.expanded = !panel.expanded;
            }
        }
        Action::Copy => {
            let Some((_, text)) = latest(session, audio) else {
                return Ok(());
            };
            let text = text.to_owned();
            let result = world
                .resource_mut::<bevy::clipboard::Clipboard>()
                .set_text(&text);
            let message = match result {
                Ok(()) => "已复制文稿".into(),
                Err(failure) => format!("复制文稿失败：{failure}"),
            };
            world.resource_mut::<TranscriptionState>().feedback =
                Some((session_id, audio, message));
        }
        Action::Ask => {
            let Some((version, _)) = latest(session, audio) else {
                return Ok(());
            };
            let id = version.id;
            world
                .resource_mut::<TranscriptionState>()
                .selected
                .insert(session_id, Some(id));
        }
    }
    Ok(())
}

pub(crate) fn receipt(
    world: &mut World,
    operation: u64,
    session: SessionId,
    result: Result<TranscriptionView, String>,
) {
    let mut state = world.resource_mut::<TranscriptionState>();
    let Some(pending) = state
        .pending
        .filter(|item| item.id == operation && item.session == session)
    else {
        return;
    };
    state.pending = None;
    match result {
        Ok(result) => {
            if let Some(chat) = world
                .resource_mut::<ChatState>()
                .session
                .as_mut()
                .filter(|view| view.id == session)
            {
                let entry = ChatTranscription::from(result);
                if let Some(existing) = chat
                    .transcriptions
                    .iter_mut()
                    .find(|item| item.id == entry.id)
                {
                    *existing = entry;
                } else {
                    chat.transcriptions.push(entry);
                }
            }
        }
        Err(failure) => {
            state.feedback = Some((session, pending.audio, format!("转写未确认：{failure}")))
        }
    }
}

fn synchronize(world: &mut World) -> Result {
    synchronize_selection(world);
    let mut panels = world.query::<(Entity, &Panel)>();
    let panels: Vec<_> = panels
        .iter(world)
        .map(|(entity, panel)| (entity, panel.clone()))
        .collect();
    let draft_busy = world
        .resource::<crate::audio_attachment::AudioDraftState>()
        .busy();
    for (entity, panel) in panels {
        let chat = world.resource::<ChatState>();
        let session = chat.session.as_ref();
        let target =
            session.and_then(|session| panel.target.audio(session).map(|audio| (session, audio)));
        let Some((session, audio)) = target else {
            set_display(world, entity, false);
            continue;
        };
        let state = world.resource::<TranscriptionState>();
        let pending = state
            .pending
            .filter(|item| item.session == session.id && item.audio == audio);
        let newest = session
            .transcriptions
            .iter()
            .rev()
            .find(|item| item.audio == audio);
        let finished = latest(session, audio);
        let displayed = finished.map(|(item, _)| item.id);
        let content = (displayed != panel.displayed)
            .then(|| finished.map_or("", |(_, text)| text).to_owned());
        let feedback = state
            .feedback
            .as_ref()
            .filter(|(session_id, id, _)| *session_id == session.id && *id == audio);
        let summary = if let Some((_, _, message)) = feedback {
            message.to_owned()
        } else if let Some(pending) = pending {
            if pending.cancelling {
                "正在取消转写…".into()
            } else {
                "正在转写…".into()
            }
        } else {
            newest.map_or_else(String::new, |entry| match &entry.outcome {
                ChatOutcome::Completed(_) => format!("文稿已保存 · {}", entry.engine),
                ChatOutcome::Failed(error) => format!("转写失败：{error}"),
                ChatOutcome::Cancelled => "已取消转写".into(),
                ChatOutcome::Interrupted => "转写中断，未确认完整文稿".into(),
            })
        };
        let idle = !state.busy() && !draft_busy;
        let can_cancel = pending.is_some_and(|item| !item.cancelling);
        set_display(world, entity, true);
        let children = world
            .get::<Children>(entity)
            .map(|items| items.to_vec())
            .unwrap_or_default();
        for child in children {
            if world.get::<TranscriptText>(child).is_some() {
                set_display(world, child, panel.expanded && displayed.is_some());
                if let Some(content) = &content
                    && world.get::<WidgetryTextFieldHeadless>(child).is_some()
                {
                    WidgetryTextFieldHeadless::set_text_in_world(world, child, content)
                        .map_err(|failure| failure.into_bevy_error())?;
                }
            } else {
                let controls = world
                    .get::<Children>(child)
                    .map(|items| items.to_vec())
                    .unwrap_or_default();
                for control in controls {
                    if world.get::<Summary>(control).is_some()
                        && let Some(mut output) = world.get_mut::<Text>(control)
                    {
                        output.set_if_neq(Text::new(&summary));
                    }
                    if let Some(action) = world.get::<Action>(control) {
                        let (visible, enabled) = match action {
                            Action::Start => (pending.is_none(), idle),
                            Action::Cancel => (pending.is_some(), can_cancel),
                            Action::View | Action::Copy | Action::Ask => {
                                (displayed.is_some(), true)
                            }
                        };
                        set_display(world, control, visible);
                        if enabled && world.get::<InteractionDisabled>(control).is_some() {
                            world.entity_mut(control).remove::<InteractionDisabled>();
                        } else if !enabled && world.get::<InteractionDisabled>(control).is_none() {
                            world.entity_mut(control).insert(InteractionDisabled);
                        }
                    }
                }
            }
        }
        if let Some(mut panel) = world.get_mut::<Panel>(entity) {
            panel.displayed = displayed;
        }
    }
    Ok(())
}

fn synchronize_selection(world: &mut World) {
    let chat = world.resource::<ChatState>();
    let selection = chat.session.as_ref().and_then(|session| {
        let id = world.resource::<TranscriptionState>().selection(session)?;
        let turns = || {
            session
                .turn
                .iter()
                .chain(session.previous_turns.iter().rev())
        };
        if let Some(reference) = turns()
            .filter_map(|turn| turn.transcript.as_ref())
            .find(|reference| reference.transcript == id)
        {
            return Some(reference.label());
        }
        let document = session.transcriptions.iter().find(|entry| entry.id == id)?;
        let source = session
            .audio_draft
            .iter()
            .chain(turns().filter_map(|turn| turn.audio.as_ref()))
            .find(|audio| audio.id == document.audio)?;
        Some(source_label(&source.name, &document.engine, id))
    });
    let mut panels = world.query_filtered::<Entity, With<SelectionPanel>>();
    let panels: Vec<_> = panels.iter(world).collect();
    let visible = selection.is_some();
    for entity in panels {
        set_display(world, entity, visible);
    }
    let label = selection.map_or_else(String::new, |source| format!("下一条消息引用 · {source}"));
    let mut outputs = world.query_filtered::<&mut Text, With<SelectionLabel>>();
    for mut output in outputs.iter_mut(world) {
        output.set_if_neq(Text::new(&label));
    }
}

fn source_label(filename: &str, engine: &str, id: SessionId) -> String {
    let version = id.to_string();
    format!("{filename} · {engine} · 文稿 {} · 仅文字", &version[..8])
}

fn set_display(world: &mut World, entity: Entity, visible: bool) {
    let display = if visible {
        Display::Flex
    } else {
        Display::None
    };
    if let Some(mut node) = world.get_mut::<Node>(entity)
        && node.display != display
    {
        node.display = display;
    }
}
