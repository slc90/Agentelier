use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{AudioAttachmentView, SessionId};
use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy::window::PrimaryWindow;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogFilter, WidgetryFileDialogFilterId,
    WidgetryFileDialogModality, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent, WidgetryFileDialogWindow,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use std::path::PathBuf;

#[derive(Clone, Reflect)]
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
        let format = if self.mime == "audio/wav" {
            "WAV"
        } else {
            "MP3"
        };
        format!("{} · {format} · {seconds:.2} 秒 · {kib:.1} KiB", self.name)
    }
}

#[derive(Resource, Default)]
pub(crate) struct AudioDraftState {
    picker: Option<(Entity, SessionId)>,
    pending: Option<(u64, SessionId)>,
    feedback: Option<(SessionId, String)>,
}

impl AudioDraftState {
    pub(crate) fn busy(&self) -> bool {
        self.picker.is_some() || self.pending.is_some()
    }

    pub(crate) fn host_failed(&mut self, reason: &str) {
        if let Some((_, session)) = self.pending.take() {
            self.feedback = Some((session, format!("音频保存未确认：{reason}")));
        }
    }
}

#[derive(Component, Clone, Default)]
struct ChooseAudio;
#[derive(Component, Clone, Default)]
struct RemoveAudio;
#[derive(Component, Clone, Default)]
struct AudioDraftText;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AudioDraftState>()
        .add_systems(Update, synchronize);
}

pub(crate) fn panel() -> impl Scene {
    bsn! {
        Node { width: percent(100), min_width: px(0), column_gap: px(8), row_gap: px(4), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0, align_items: AlignItems::Center }
        Children [
            @AgentelierButton { @children: {Some(Box::new(bsn_list!{Text("添加音频") WidgetryText}) as Box<dyn SceneList>)} }
            ChooseAudio Name("ChooseAudioAttachment") on(choose)--
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

fn choose(
    _: On<Activate>,
    chat: Res<ChatState>,
    mut media: ResMut<AudioDraftState>,
    transcription: Res<crate::transcription::TranscriptionState>,
    primary: Single<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    let Some(session) = chat.session.as_ref().map(|chat| chat.id) else {
        return;
    };
    if media.busy() || transcription.busy() {
        return;
    }
    let mut window = WidgetryFileDialogWindow {
        parent: Some(*primary),
        modality: WidgetryFileDialogModality::Modal,
        ..default()
    };
    window.native.title = "选择音频附件".into();
    let picker = commands
        .spawn_scene_with_error_handler(bsn! {
            @WidgetryFileDialog {
                @mode: WidgetryFileDialogMode::PickFile,
                @window: {Some(window)},
                @storage_scope: {Some("chat-audio-attachment".into())},
                @filters: {vec![WidgetryFileDialogFilter {
                    id: WidgetryFileDialogFilterId("audio".into()),
                    label: "WAV / MP3".into(),
                    suffixes: vec!["wav".into(), "mp3".into()],
                    default_extension: None,
                }]},
            }
            Name("AudioAttachmentPicker") on(selected)
        })
        .id();
    media.picker = Some((picker, session));
    media.feedback = None;
}

fn selected(
    event: On<WidgetryFileDialogResultEvent>,
    host: Res<AgentHost>,
    mut state: ResMut<AudioDraftState>,
    mut commands: Commands,
) {
    let Some((_, session)) = state.picker.filter(|(entity, _)| *entity == event.entity) else {
        return;
    };
    state.picker = None;
    commands.entity(event.entity).despawn();
    match &event.result {
        WidgetryFileDialogResult::File(path) => {
            submit(&host, &mut state, session, Some(path.clone()))
        }
        WidgetryFileDialogResult::Cancelled => {
            state.feedback = Some((session, "已取消选择音频".into()))
        }
        _ => state.feedback = Some((session, "文件选择器未返回音频文件".into())),
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
        submit(&host, &mut state, session.id, None);
    }
}

fn submit(
    host: &AgentHost,
    state: &mut AudioDraftState,
    session: SessionId,
    path: Option<PathBuf>,
) {
    state.feedback = None;
    match host.change_audio_draft(session, path) {
        Ok(id) => state.pending = Some((id, session)),
        Err(failure) => state.feedback = Some((session, format!("无法处理音频附件：{failure}"))),
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
    mut media: ResMut<AudioDraftState>,
    chat: Res<ChatState>,
    transcription: Res<crate::transcription::TranscriptionState>,
    pickers: Query<(), With<WidgetryFileDialog>>,
    mut texts: Query<&mut Text, With<AudioDraftText>>,
    mut buttons: Query<
        (
            Entity,
            Has<InteractionDisabled>,
            Has<RemoveAudio>,
            &mut Node,
        ),
        Or<(With<ChooseAudio>, With<RemoveAudio>)>,
    >,
    mut commands: Commands,
) {
    if let Some((entity, session)) = media.picker
        && !pickers.contains(entity)
    {
        media.picker = None;
        media.feedback = Some((session, "音频选择器已关闭".into()));
    }
    let session = chat.session.as_ref();
    let draft = session.and_then(|session| session.audio_draft.as_ref());
    let metadata = draft
        .map(|audio| format!("随下一条直接发送的消息附上 {}", audio.label()))
        .unwrap_or_default();
    let feedback = if media.pending.is_some() {
        "正在处理音频附件…"
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
    for mut output in &mut texts {
        output.set_if_neq(Text::new(&text));
    }
    for (entity, disabled, remove, mut node) in &mut buttons {
        if remove {
            let display = if draft.is_some() {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
        }
        let next = session.is_none() || media.busy() || transcription.busy();
        if next && !disabled {
            commands.entity(entity).insert(InteractionDisabled);
        }
        if !next && disabled {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
}
