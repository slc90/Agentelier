use crate::{SessionId, SessionView};
use agentelier_prompts::InstructionSnapshot;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredImage {
    pub session: SessionId,
    pub image: agentelier_tools::ImageDescriptor,
    pub summary: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub instructions: InstructionSnapshot,
    pub working_directory: PathBuf,
    pub tools: Vec<StoredTool>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InitialSession {
    pub session: SessionId,
    pub title: String,
    pub created_at_ms: u64,
    pub settings: StoredSettings,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session: SessionId,
    pub title: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_sequence: u64,
    pub deleted: bool,
}

impl InitialSession {
    pub(crate) fn from_view(
        view: &SessionView,
        tools: Vec<StoredTool>,
    ) -> Result<Self, crate::ConversationFailure> {
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| crate::ConversationFailure::new("系统时间早于会话时间起点"))?
            .as_millis()
            .try_into()
            .map_err(|_| crate::ConversationFailure::new("会话时间超出记录范围"))?;
        Ok(Self {
            session: view.id,
            title: "新会话".into(),
            created_at_ms,
            settings: StoredSettings {
                slug: view.settings.slug.clone(),
                display_name: view.settings.display_name.clone(),
                effort: view.settings.effort.clone(),
                image_input: view.settings.image_input,
                instructions: (*view.settings.instructions).clone(),
                working_directory: view.settings.working_directory.clone(),
                tools,
            },
        })
    }

    pub fn summary(&self, sequence: u64) -> SessionSummary {
        SessionSummary {
            session: self.session,
            title: self.title.clone(),
            created_at_ms: self.created_at_ms,
            last_activity_ms: self.created_at_ms,
            last_sequence: sequence,
            deleted: false,
        }
    }
}
