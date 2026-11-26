use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{AudioAttachmentView, SessionId};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::text::WidgetryText;

#[derive(Clone, PartialEq, Reflect)]
pub struct ChatAudio {
    pub id: SessionId,
    pub name: String,
    pub mime: String,
    pub digest: String,
    pub encoded_bytes: usize,
    pub duration_ms: u64,
}

impl From<AudioAttachmentView> for ChatAudio {
    fn from(attachment: AudioAttachmentView) -> Self {
        Self {
            id: attachment.id,
            name: attachment.name,
            mime: attachment.mime.into(),
            digest: attachment.digest,
            encoded_bytes: attachment.encoded_bytes,
            duration_ms: attachment.duration_ms,
        }
    }
}

impl ChatAudio {
    fn label(&self) -> String {
        let seconds = self.duration_ms as f64 / 1000.0;
        let kib = self.encoded_bytes as f64 / 1024.0;
        let format = match self.mime.as_str() {
            "audio/wav" => "WAV",
            "audio/flac" => "FLAC",
            _ => "MP3",
        };
        format!("{} · {format} · {seconds:.2} 秒 · {kib:.1} KiB", self.name)
    }
}

pub(super) fn published_card(audio: ChatAudio) -> impl Scene {
    let label = audio.label();
    bsn! {
        Name("ChatPublishedAudio") Text(label) WidgetryText
        TextLayout::linebreak(LineBreak::AnyCharacter)
        Node {width: percent(100), min_width: px(0)}
    }
}

#[derive(Resource, Default)]
pub(crate) struct AudioDraftState {
    pending: Option<(u64, SessionId)>,
    feedback: Option<(SessionId, String)>,
}

impl AudioDraftState {
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn host_failed(&mut self, reason: &str) {
        if let Some((_, session)) = self.pending.take() {
            self.feedback = Some((session, format!("音频保存未确认：{reason}")));
        }
    }
}

#[derive(Component, Clone, Default)]
struct AudioDraftPanel;
#[derive(Component, Clone, Default)]
struct RemoveAudio;
#[derive(Component, Clone, Default)]
struct AudioDraftText;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AudioDraftState>()
        .add_systems(Update, synchronize);
}

pub(crate) fn draft_panel() -> impl Scene {
    bsn! {
        AudioDraftPanel
        Node { display: Display::None, width: percent(100), min_width: px(0), column_gap: px(8), row_gap: px(4), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0, align_items: AlignItems::Center }
        Children [
            @AgentelierButton { @children: {Some(Box::new(bsn_list!{Text("移除附件") WidgetryText}) as Box<dyn SceneList>)} }
            RemoveAudio Name("RemoveAudioAttachment") Node { display: Display::None } on(remove)--
            Text::default() WidgetryText AudioDraftText Name("AudioDraftStatus")
        ]
    }
}

pub(crate) fn card(audio: ChatAudio) -> impl Scene {
    let metadata = audio.label();
    let id = audio.id;
    bsn! {
        Node { flex_direction: FlexDirection::Column, padding: UiRect::all(px(8)), row_gap: px(4), align_self: AlignSelf::Start, max_width: percent(100), flex_shrink: 0.0 }
        Children [Text("音频附件") WidgetryText-- Text(metadata) WidgetryText--
            @crate::transcription::panel(crate::transcription::Target::Saved(id))]
    }
}

fn remove(
    _: On<Activate>,
    chat: Res<ChatState>,
    host: Res<AgentHost>,
    transcription: Res<crate::transcription::TranscriptionState>,
    mut state: ResMut<AudioDraftState>,
) {
    if let Some(session) = &chat.session
        && session.audio_draft.is_some()
        && !state.busy()
        && !transcription.busy()
    {
        state.feedback = None;
        match host.change_audio_draft(session.id, None) {
            Ok(id) => state.pending = Some((id, session.id)),
            Err(failure) => {
                state.feedback = Some((session.id, format!("无法移除音频附件：{failure}")));
            }
        }
    }
}

pub(crate) fn receipt(
    world: &mut World,
    operation: u64,
    session: SessionId,
    result: Result<Option<AudioAttachmentView>, String>,
) {
    let mut state = world.resource_mut::<AudioDraftState>();
    if state.pending != Some((operation, session)) {
        return;
    }
    state.pending = None;
    match result {
        Ok(draft) => {
            if let Some(chat) = world
                .resource_mut::<ChatState>()
                .session
                .as_mut()
                .filter(|view| view.id == session)
            {
                chat.audio_draft = draft.map(Into::into);
            }
        }
        Err(failure) => state.feedback = Some((session, format!("无法保存音频附件：{failure}"))),
    }
}

fn synchronize(
    media: Res<AudioDraftState>,
    chat: Res<ChatState>,
    transcription: Res<crate::transcription::TranscriptionState>,
    mut panels: Query<&mut Node, (With<AudioDraftPanel>, Without<RemoveAudio>)>,
    mut texts: Query<&mut Text, With<AudioDraftText>>,
    mut buttons: Query<(Entity, Has<InteractionDisabled>, &mut Node), With<RemoveAudio>>,
    mut commands: Commands,
) {
    let session = chat.session.as_ref();
    let draft = session.and_then(|session| session.audio_draft.as_ref());
    let metadata = draft
        .map(|audio| format!("随下一条直接发送的消息附上 {}", audio.label()))
        .unwrap_or_default();
    let feedback = if media.pending.is_some() {
        "正在移除音频附件…"
    } else {
        media
            .feedback
            .as_ref()
            .filter(|(session, _)| Some(*session) == chat.session.as_ref().map(|chat| chat.id))
            .map_or("", |(_, message)| message.as_str())
    };
    let text = [metadata.as_str(), feedback]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let panel_display = if text.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    for node in &mut panels {
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(panel_display);
    }
    for mut output in &mut texts {
        output.set_if_neq(Text::new(&text));
    }
    let display = if draft.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    for (entity, disabled, node) in &mut buttons {
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
        let next = media.busy() || transcription.busy();
        if next && !disabled {
            commands.entity(entity).insert(InteractionDisabled);
        }
        if !next && disabled {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
}
