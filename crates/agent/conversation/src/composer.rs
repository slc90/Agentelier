use crate::{AudioAttachment, ConversationFailure, SessionId, TranscriptId, TranscriptReference};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, PathBuf};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerElement {
    pub id: uuid::Uuid,
    #[serde(flatten)]
    pub content: ComposerContent,
}

impl ComposerElement {
    pub fn new(content: ComposerContent) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            content,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComposerContent {
    Text {
        text: String,
    },
    Skill {
        name: String,
        identity: SkillIdentity,
    },
    Mcp {
        label: String,
        identity: McpIdentity,
    },
    File {
        path: PathBuf,
    },
    Image {
        image: InputImage,
    },
    Audio {
        audio: AudioAttachment,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillIdentity {
    File(PathBuf),
    Embedded(agentelier_skills::EmbeddedSkill),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpIdentity {
    pub server: uuid::Uuid,
    pub tool: Option<McpToolIdentity>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolIdentity {
    pub target: String,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputImage {
    pub id: uuid::Uuid,
    pub name: String,
    pub image: agentelier_tools::ImageDescriptor,
}

#[derive(Clone, Default)]
pub struct ComposerSubmission {
    pub draft_owner: Option<SessionId>,
    pub elements: Vec<ComposerElement>,
    pub transcript: Option<TranscriptId>,
}

impl ComposerSubmission {
    pub fn text(text: String) -> Self {
        Self {
            elements: vec![ComposerElement::new(ComposerContent::Text { text })],
            transcript: None,
            draft_owner: None,
        }
    }

    pub fn requires_queue(&self) -> bool {
        self.transcript.is_some()
            || self.elements.iter().any(|element| match &element.content {
                ComposerContent::Text { text } => !agentelier_skills::mentions(text).is_empty(),
                _ => true,
            })
    }

    pub(crate) fn resolve(
        self,
        view: &crate::SessionView,
    ) -> Result<ComposerMessage, ConversationFailure> {
        let transcript = self
            .transcript
            .map(|id| view.transcript_reference(id))
            .transpose()?;
        let mut message = ComposerMessage::new(self.elements, transcript);
        message.draft_owner = self.draft_owner;
        message.validate()?;
        for audio in message.audio() {
            if self.draft_owner.is_none() && view.audio_draft.as_ref() != Some(audio) {
                return Err(ConversationFailure::new(
                    "草稿音频附件已变化，请核对后重新发送",
                ));
            }
        }
        Ok(message)
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredComposerMessage")]
pub struct ComposerMessage {
    elements: Vec<ComposerElement>,
    #[serde(skip)]
    draft_owner: Option<SessionId>,
    #[serde(skip)]
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<TranscriptReference>,
}

#[derive(Deserialize)]
struct StoredComposerMessage {
    #[serde(default)]
    elements: Option<Vec<ComposerElement>>,
    #[serde(default, alias = "input")]
    text: String,
    #[serde(default)]
    audio: Option<AudioAttachment>,
    #[serde(default)]
    transcript: Option<TranscriptReference>,
}

impl From<StoredComposerMessage> for ComposerMessage {
    fn from(stored: StoredComposerMessage) -> Self {
        let elements = stored.elements.unwrap_or_else(|| {
            let mut elements = Vec::new();
            if !stored.text.is_empty() {
                elements.push(ComposerElement {
                    id: uuid::Uuid::from_u128(1),
                    content: ComposerContent::Text { text: stored.text },
                });
            }
            if let Some(audio) = stored.audio {
                elements.push(ComposerElement {
                    id: uuid::Uuid::from_u128(2),
                    content: ComposerContent::Audio { audio },
                });
            }
            elements
        });
        Self::new(elements, stored.transcript)
    }
}

impl ComposerMessage {
    fn new(elements: Vec<ComposerElement>, transcript: Option<TranscriptReference>) -> Self {
        let mut text = String::new();
        for element in &elements {
            match &element.content {
                ComposerContent::Text { text: part } => text.push_str(part),
                ComposerContent::Skill { name, .. } => {
                    text.push('$');
                    text.push_str(name);
                }
                ComposerContent::Mcp { label, .. } => {
                    text.push('$');
                    text.push_str(label);
                }
                ComposerContent::File { path } => {
                    let path = path.to_string_lossy();
                    if path.contains(char::is_whitespace) {
                        text.push('"');
                        text.push_str(&path);
                        text.push('"');
                    } else {
                        text.push_str(&path);
                    }
                }
                ComposerContent::Audio { .. } | ComposerContent::Image { .. } => {}
            }
        }
        Self {
            elements,
            text,
            transcript,
            draft_owner: None,
        }
    }

    pub fn draft_owner(&self) -> Option<SessionId> {
        self.draft_owner
    }

    pub fn images(&self) -> impl Iterator<Item = &InputImage> {
        self.elements
            .iter()
            .filter_map(|element| match &element.content {
                ComposerContent::Image { image } => Some(image),
                _ => None,
            })
    }

    pub fn elements(&self) -> &[ComposerElement] {
        &self.elements
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn audio(&self) -> impl Iterator<Item = &AudioAttachment> {
        self.elements
            .iter()
            .filter_map(|element| match &element.content {
                ComposerContent::Audio { audio } => Some(audio),
                _ => None,
            })
    }

    pub fn validate(&self) -> Result<(), ConversationFailure> {
        let mut ids = HashSet::new();
        for element in &self.elements {
            if element.id.is_nil() || !ids.insert(element.id) {
                return Err(ConversationFailure::new("消息元素身份为空或重复"));
            }
            let valid = match &element.content {
                ComposerContent::Text { .. } => true,
                ComposerContent::Skill { name, identity } => {
                    !name.trim().is_empty()
                        && match identity {
                            SkillIdentity::File(path) => path.is_absolute(),
                            SkillIdentity::Embedded(skill) => skill.name() == name,
                        }
                }
                ComposerContent::Mcp { label, identity } => {
                    !label.trim().is_empty()
                        && !identity.server.is_nil()
                        && identity
                            .tool
                            .as_ref()
                            .is_none_or(|tool| !tool.target.is_empty() && !tool.name.is_empty())
                }
                ComposerContent::File { path } => {
                    !path.as_os_str().is_empty()
                        && path
                            .components()
                            .all(|part| matches!(part, Component::Normal(_)))
                }
                ComposerContent::Image { image } => {
                    image
                        .image
                        .validate()
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    !image.id.is_nil()
                        && !image.name.trim().is_empty()
                        && !image.name.contains(['/', '\\'])
                        && !image.name.contains(char::is_control)
                }
                ComposerContent::Audio { audio } => {
                    audio.validate()?;
                    true
                }
            };
            if !valid {
                return Err(ConversationFailure::new("消息包含无效的引用或附件身份"));
            }
        }
        if self.text.trim().is_empty()
            && !self.elements.iter().any(|element| {
                matches!(
                    element.content,
                    ComposerContent::Image { .. } | ComposerContent::Audio { .. }
                )
            })
        {
            return Err(ConversationFailure::new("不能发送空白消息"));
        }
        Ok(())
    }

    pub(crate) fn uses_audio_draft(&self, draft: Option<&AudioAttachment>) -> bool {
        draft.is_some_and(|draft| self.audio().any(|audio| audio == draft))
    }
}

impl ComposerMessage {
    pub(crate) fn validate_transcript_session(
        &self,
        session: SessionId,
    ) -> Result<(), ConversationFailure> {
        if self
            .transcript
            .as_ref()
            .is_some_and(|reference| reference.session != session)
        {
            return Err(ConversationFailure::new("文稿版本不属于当前会话"));
        }
        Ok(())
    }
}
