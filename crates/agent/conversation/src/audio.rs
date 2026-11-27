use crate::{Conversation, ConversationFailure, SessionId};
use agentelier_llm_model::{TranscriptEvidence, TranscriptReference};
use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_AUDIO_NAME_CHARS: usize = 255;

pub type AudioId = uuid::Uuid;
pub type TranscriptId = uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionEngine {
    Codex,
}

impl TranscriptionEngine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TranscriptionSource {
    pub id: TranscriptId,
    pub audio: AudioId,
    pub digest: String,
    pub engine: TranscriptionEngine,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum TranscriptionOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
    Interrupted,
}

impl TranscriptionOutcome {
    pub(crate) fn valid_terminal(&self) -> bool {
        match self {
            Self::Completed(text) | Self::Failed(text) => {
                !text.trim().is_empty() && text.len() <= agentelier_llm_model::MAX_TRANSCRIPT_BYTES
            }
            Self::Cancelled => true,
            Self::Interrupted => false,
        }
    }
}

#[derive(Clone)]
pub struct AudioTranscription {
    pub source: TranscriptionSource,
    pub outcome: TranscriptionOutcome,
}

impl AudioTranscription {
    pub(crate) fn evidence(
        &self,
        session: SessionId,
        reference: &TranscriptReference,
    ) -> Result<TranscriptEvidence, ConversationFailure> {
        if reference.session != session
            || reference.transcript != self.source.id
            || reference.audio != self.source.audio
            || reference.audio_sha256 != self.source.digest
            || reference.engine != self.source.engine.name()
        {
            return Err(ConversationFailure::new("文稿版本与音频来源不一致"));
        }
        let TranscriptionOutcome::Completed(text) = &self.outcome else {
            return Err(ConversationFailure::new("所选转写没有完整文稿"));
        };
        TranscriptEvidence::new(reference.clone(), text).map_err(crate::result_budget::model_error)
    }
}

impl crate::SessionView {
    fn audio_attachment(&self, audio: AudioId) -> Option<&AudioAttachment> {
        self.audio_draft
            .iter()
            .chain(
                self.turn
                    .iter()
                    .map(Box::as_ref)
                    .chain(&self.previous_turns)
                    .flat_map(|turn| turn.submission.audio()),
            )
            .chain(self.queue.iter().flat_map(|input| input.submission.audio()))
            .find(|item| item.id == audio)
    }

    pub(crate) fn transcript_reference(
        &self,
        id: TranscriptId,
    ) -> Result<TranscriptReference, ConversationFailure> {
        let result = self
            .transcriptions
            .iter()
            .find(|item| item.source.id == id)
            .ok_or_else(|| ConversationFailure::new("所选文稿不属于当前会话"))?;
        let TranscriptionOutcome::Completed(text) = &result.outcome else {
            return Err(ConversationFailure::new("所选转写没有完整文稿"));
        };
        let audio = self
            .audio_attachment(result.source.audio)
            .filter(|item| item.digest == result.source.digest);
        let Some(audio) = audio else {
            let reference = self
                .turn
                .iter()
                .map(Box::as_ref)
                .chain(&self.previous_turns)
                .filter_map(|turn| turn.submission.transcript.as_ref())
                .find(|source| source.transcript == id)
                .ok_or_else(|| ConversationFailure::new("所选文稿缺少原始音频附件"))?;
            result.evidence(self.id, reference)?;
            return Ok(reference.clone());
        };
        Ok(TranscriptReference::new(
            self.id,
            audio.id,
            id,
            audio.digest.clone(),
            audio.name.clone(),
            result.source.engine.name().into(),
            text,
        ))
    }

    pub(crate) fn transcript_evidence(
        &self,
        reference: &TranscriptReference,
    ) -> Result<TranscriptEvidence, ConversationFailure> {
        self.transcriptions
            .iter()
            .find(|item| item.source.id == reference.transcript)
            .ok_or_else(|| ConversationFailure::new("会话中没有所选文稿版本"))?
            .evidence(self.id, reference)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioAttachment {
    pub id: AudioId,
    pub name: String,
    pub format: AudioFormat,
    pub digest: String,
    pub encoded_bytes: usize,
    pub duration_ms: u64,
}

impl AudioAttachment {
    pub fn validate(&self) -> Result<(), ConversationFailure> {
        let name = &self.name;
        if self.id.is_nil()
            || name.trim().is_empty()
            || name.chars().count() > MAX_AUDIO_NAME_CHARS
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !(1..=MAX_AUDIO_BYTES).contains(&self.encoded_bytes)
            || self.duration_ms == 0
        {
            return Err(ConversationFailure::new("音频附件描述符无效"));
        }
        Ok(())
    }
}

impl Conversation {
    pub fn saved_audio(&self, session: SessionId, audio: AudioId) -> Option<AudioAttachment> {
        let view = &self
            .session
            .as_ref()
            .filter(|chat| chat.view.id == session)?
            .view;
        view.audio_attachment(audio)
            .or_else(|| {
                view.turn
                    .iter()
                    .map(Box::as_ref)
                    .chain(&view.previous_turns)
                    .flat_map(|turn| &turn.tools)
                    .filter(|tool| tool.retained)
                    .filter_map(|tool| tool.published_audio.as_ref())
                    .find(|item| item.id == audio)
            })
            .cloned()
    }

    pub fn prepare_transcription(
        &self,
        session: SessionId,
        audio: AudioId,
    ) -> Result<
        (
            AudioAttachment,
            TranscriptionSource,
            agentelier_llm_model::CodexIdentity,
        ),
        ConversationFailure,
    > {
        let view = &self
            .session
            .as_ref()
            .filter(|chat| chat.view.id == session)
            .ok_or_else(|| ConversationFailure::new("当前会话已变化"))?
            .view;
        let codex = view
            .settings
            .codex
            .clone()
            .ok_or_else(|| ConversationFailure::new("该会话没有绑定 Codex 凭据"))?;
        let attachment = view
            .audio_attachment(audio)
            .ok_or_else(|| ConversationFailure::new("当前会话没有此音频附件"))?;
        let source = TranscriptionSource {
            id: TranscriptId::new_v4(),
            audio,
            digest: attachment.digest.clone(),
            engine: TranscriptionEngine::Codex,
        };
        Ok((attachment.clone(), source, codex))
    }

    pub fn apply_transcription(&mut self, session: SessionId, result: AudioTranscription) {
        if let Some(chat) = self.session.as_mut().filter(|chat| chat.view.id == session) {
            let entries = std::sync::Arc::make_mut(&mut chat.view.transcriptions);
            if let Some(existing) = entries
                .iter_mut()
                .find(|item| item.source.id == result.source.id)
            {
                *existing = result;
            } else {
                entries.push(result);
            }
        }
    }

    pub fn is_current(&self, session: SessionId) -> bool {
        self.session
            .as_ref()
            .is_some_and(|chat| chat.view.id == session)
    }

    pub fn apply_audio_draft(
        &mut self,
        session: SessionId,
        draft: Option<AudioAttachment>,
    ) -> bool {
        if let Some(chat) = self.session.as_mut().filter(|chat| chat.view.id == session) {
            chat.view.audio_draft = draft;
            return true;
        }
        false
    }
}
