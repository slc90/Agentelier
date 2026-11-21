use crate::{Conversation, ConversationFailure, SessionId};
use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_SESSION_AUDIO_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_AUDIO_NAME_CHARS: usize = 255;

pub type AudioId = uuid::Uuid;
pub type TranscriptId = uuid::Uuid;
pub const MAX_SESSION_TRANSCRIPTIONS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionEngine {
    Codex,
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Wav,
    Mp3,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
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
        if view.transcriptions.len() >= MAX_SESSION_TRANSCRIPTIONS {
            return Err(ConversationFailure::new("本会话已达到 64 次音频转写上限"));
        }
        let attachment = view
            .audio_draft
            .iter()
            .chain(
                view.turn
                    .iter()
                    .map(Box::as_ref)
                    .chain(&view.previous_turns)
                    .filter_map(|turn| turn.audio.as_ref()),
            )
            .find(|item| item.id == audio)
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
