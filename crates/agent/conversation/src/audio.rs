use crate::{Conversation, ConversationFailure, SessionId};
use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_SESSION_AUDIO_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_AUDIO_NAME_CHARS: usize = 255;

pub type AudioId = uuid::Uuid;

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
