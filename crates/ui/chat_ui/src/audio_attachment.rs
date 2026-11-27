use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{AudioAttachmentView, SessionId};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui_widgets::Activate;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent, WidgetryFileDialogWindow,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use std::path::PathBuf;

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
    pub(crate) fn attachment(&self) -> agentelier_conversation_bridge::AudioAttachment {
        use agentelier_conversation_bridge::{AudioAttachment, AudioFormat};
        AudioAttachment {
            id: self.id,
            name: self.name.clone(),
            format: match self.mime.as_str() {
                "audio/wav" => AudioFormat::Wav,
                "audio/flac" => AudioFormat::Flac,
                _ => AudioFormat::Mp3,
            },
            digest: self.digest.clone(),
            encoded_bytes: self.encoded_bytes,
            duration_ms: self.duration_ms,
        }
    }

    fn label(&self) -> String {
        let kib = self.encoded_bytes as f64 / 1024.0;
        let format = match self.mime.as_str() {
            "audio/wav" => "WAV",
            "audio/flac" => "FLAC",
            _ => "MP3",
        };
        format!("{} · {format} · {kib:.1} KiB", self.name)
    }
}

pub(super) fn published_card(session: SessionId, audio: ChatAudio) -> impl Scene {
    let label = audio.label();
    let controls = super::audio_playback::controls(session, &audio);
    bsn! {
        Name("ChatPublishedAudio")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [Text(label) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)-- @controls]
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
struct AddAudio;

#[derive(Component, Clone, Default)]
struct AudioDraftText;

#[derive(Component, Clone, Copy)]
struct AudioPicker(SessionId);

pub(crate) fn install(app: &mut App) {
    super::audio_playback::install(app);
    app.init_resource::<AudioDraftState>()
        .add_systems(Update, synchronize);
}

pub(crate) fn draft_panel() -> impl Scene {
    bsn! {
        Node { width: percent(100), min_width: px(0), column_gap: px(8), flex_shrink: 0.0, align_items: AlignItems::Center }
        Children [
            @AgentelierButton { @children: bsn_list! {Text("添加音频") WidgetryText} }
            AddAudio Name("AddComposerAudio") Node { display: Display::None } on(pick_audio)--
            Text::default() WidgetryText AudioDraftText Name("AudioDraftStatus") Node { display: Display::None }
        ]
    }
}

fn pick_audio(
    _: On<Activate>,
    chat: Res<ChatState>,
    state: Res<AudioDraftState>,
    mut commands: Commands,
) {
    let Some(session) = chat
        .session
        .as_ref()
        .map(|session| session.id)
        .filter(|_| !state.busy())
    else {
        return;
    };
    let window = WidgetryFileDialogWindow {
        native: Window {
            title: "选择音频文件".into(),
            resolution: (840, 580).into(),
            ..default()
        },
        ..default()
    };
    commands.spawn_scene_with_error_handler(bsn! {
        @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickFile, @window: {Some(window)} }
        Name("ComposerAudioPicker") template(move |_| Ok(AudioPicker(session))) on(audio_picked)
    });
}

fn audio_picked(
    event: On<WidgetryFileDialogResultEvent>,
    pickers: Query<&AudioPicker>,
    mut commands: Commands,
) {
    let Ok(&AudioPicker(session)) = pickers.get(event.entity) else {
        return;
    };
    if let WidgetryFileDialogResult::File(path) = &event.result {
        let path = path.clone();
        commands.queue(move |world: &mut World| -> Result {
            if world
                .resource::<ChatState>()
                .session
                .as_ref()
                .is_some_and(|view| view.id == session)
            {
                request_change(world, Some(path))?;
            }
            Ok(())
        });
    }
}

pub(crate) fn card(session: SessionId, audio: ChatAudio) -> impl Scene {
    let metadata = audio.label();
    let id = audio.id;
    let controls = super::audio_playback::controls(session, &audio);
    bsn! {
        Node { flex_direction: FlexDirection::Column, padding: UiRect::all(px(8)), row_gap: px(4), align_self: AlignSelf::Start, max_width: percent(100), flex_shrink: 0.0 }
        Children [Text("音频附件") WidgetryText-- Text(metadata) WidgetryText-- @controls--
            @crate::transcription::panel(crate::transcription::Target::Saved(id))]
    }
}

pub(crate) fn request_change(world: &mut World, path: Option<PathBuf>) -> Result {
    let Some(session) = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|view| view.id)
    else {
        return Ok(());
    };
    if world.resource::<AudioDraftState>().busy()
        || world
            .resource::<crate::transcription::TranscriptionState>()
            .busy()
    {
        return Ok(());
    }
    let request = world
        .resource::<AgentHost>()
        .change_audio_draft(session, path);
    let mut state = world.resource_mut::<AudioDraftState>();
    state.feedback = None;
    match request {
        Ok(id) => state.pending = Some((id, session)),
        Err(failure) => state.feedback = Some((session, format!("无法更改音频附件：{failure}"))),
    }
    Ok(())
}

pub(crate) fn receipt(
    world: &mut World,
    operation: u64,
    session: SessionId,
    result: Result<Option<AudioAttachmentView>, String>,
) -> Result {
    let mut state = world.resource_mut::<AudioDraftState>();
    if state.pending != Some((operation, session)) {
        return Ok(());
    }
    state.pending = None;
    match result {
        Ok(draft) => crate::conversation::apply_legacy_audio(world, session, draft)?,
        Err(failure) => state.feedback = Some((session, format!("无法保存音频附件：{failure}"))),
    }
    Ok(())
}

fn synchronize(
    media: Res<AudioDraftState>,
    chat: Res<ChatState>,
    mut buttons: Query<&mut Node, (With<AddAudio>, Without<AudioDraftText>)>,
    mut texts: Query<(&mut Text, &mut Node), With<AudioDraftText>>,
) {
    let session = chat.session.as_ref().map(|session| session.id);
    let feedback = if media
        .pending
        .is_some_and(|(_, target)| Some(target) == session)
    {
        "正在保存音频附件…"
    } else {
        media
            .feedback
            .as_ref()
            .filter(|(target, _)| Some(*target) == session)
            .map_or("", |(_, message)| message.as_str())
    };
    for node in &mut buttons {
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(if session.is_some() {
                Display::Flex
            } else {
                Display::None
            });
    }
    for (mut text, node) in &mut texts {
        text.set_if_neq(Text::new(feedback));
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(if feedback.is_empty() {
                Display::None
            } else {
                Display::Flex
            });
    }
}
