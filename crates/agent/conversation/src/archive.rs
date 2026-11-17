use crate::{SessionId, SessionView};
use agentelier_prompts::InstructionSnapshot;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredState {
    #[serde(default)]
    pub context: Box<crate::ContextState>,
    pub session: SessionId,
    pub turn: Option<Box<crate::TurnView>>,
    pub previous_turns: Vec<crate::TurnView>,
    pub committed_items: Vec<crate::ProtocolItemSummary>,
    pub execution_blocked: bool,
    pub queue: Vec<crate::QueuedInput>,
    pub queue_paused: bool,
}

impl From<&SessionView> for StoredState {
    fn from(view: &SessionView) -> Self {
        Self {
            context: view.context.clone(),
            session: view.id,
            turn: view.turn.clone(),
            previous_turns: view.previous_turns.clone(),
            committed_items: view.committed_items.clone(),
            execution_blocked: view.execution_blocked,
            queue: view.queue.clone(),
            queue_paused: view.queue_paused,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordEvent {
    QueueSuspended {
        state: StoredState,
    },
    Recovered {
        state: StoredState,
        history: agentelier_llm_model::ProtocolArchive,
        notice: String,
    },
    Accepted {
        state: StoredState,
    },
    Response {
        state: StoredState,
        output: agentelier_llm_model::ProtocolArchive,
    },
    ToolStarted {
        state: StoredState,
        call_id: String,
    },
    ToolReturned {
        state: StoredState,
        call_id: String,
        status: agentelier_tools::ToolStatus,
        output: agentelier_llm_model::ArchivedPayload,
    },
    Sealed {
        state: StoredState,
        history: agentelier_llm_model::ProtocolArchive,
    },
    SteerAccepted {
        state: StoredState,
    },
    SteerApplied {
        state: StoredState,
        history: agentelier_llm_model::ProtocolArchive,
    },
    QueueChanged {
        state: StoredState,
        decision: crate::QueueDecision,
    },
    Finalized {
        state: StoredState,
        history: agentelier_llm_model::ProtocolArchive,
    },
    Finished {
        state: StoredState,
        history: Option<agentelier_llm_model::ProtocolArchive>,
    },
}

impl RecordEvent {
    pub fn state(&self) -> &StoredState {
        match self {
            Self::QueueSuspended { state }
            | Self::Recovered { state, .. }
            | Self::Accepted { state }
            | Self::Response { state, .. }
            | Self::ToolStarted { state, .. }
            | Self::ToolReturned { state, .. }
            | Self::Sealed { state, .. }
            | Self::SteerAccepted { state }
            | Self::SteerApplied { state, .. }
            | Self::QueueChanged { state, .. }
            | Self::Finalized { state, .. }
            | Self::Finished { state, .. } => state,
        }
    }
}

#[derive(Clone)]
pub struct ArchiveEntry {
    pub summary: SessionSummary,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SessionRecord {
    pub session: SessionId,
    pub at_ms: u64,
    pub event: RecordEvent,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredImage {
    pub session: SessionId,
    pub image: agentelier_tools::ImageDescriptor,
    pub summary: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSettings {
    #[serde(default)]
    pub context_limits: Box<agentelier_llm_model::ContextLimits>,
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
                context_limits: view.settings.context_limits.clone(),
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
