use crate::{SessionId, SessionView};
use agentelier_prompts::InstructionSnapshot;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredState {
    #[serde(default)]
    pub project_error: Option<String>,
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
            project_error: match view.project_context.as_ref() {
                crate::ProjectContextState::Failed { message, .. } => Some(message.clone()),
                _ => None,
            },
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
pub struct ResponseAcceptance {
    pub append: agentelier_llm_model::HistoryAppend,
    pub delivery: crate::ResponseDelivery,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredResult {
    pub response_sequence: u64,
    pub call_id: String,
    pub evidence: RecoveryEvidence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryEvidence {
    NotExecuted,
    Unconfirmed,
    Saved { sequence: u64 },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordEvent {
    ProjectPublished {
        state: StoredState,
        publication: Box<agentelier_llm_model::ProjectPublication>,
        history: Option<agentelier_llm_model::ProtocolArchive>,
    },
    RequestPrepared {
        state: StoredState,
    },
    RequestProgress {
        state: StoredState,
    },
    Compacted {
        state: StoredState,
        checkpoint: Box<crate::CompactionCheckpoint>,
    },
    QueueSuspended {
        state: StoredState,
    },
    Recovered {
        state: StoredState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history: Option<agentelier_llm_model::ProtocolArchive>,
        notice: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        results: Option<Vec<RecoveredResult>>,
    },
    Accepted {
        state: StoredState,
    },
    Response {
        state: StoredState,
        output: agentelier_llm_model::ProtocolArchive,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registrations: Option<crate::ResponseTasks>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        acceptance: Option<ResponseAcceptance>,
    },
    ToolStarted {
        state: StoredState,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution: Option<crate::TaskExecution>,
    },
    ToolReturned {
        state: StoredState,
        call_id: String,
        status: agentelier_tools::ToolStatus,
        output: agentelier_llm_model::ArchivedPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution: Option<crate::TaskExecution>,
    },
    ResultAppended {
        state: StoredState,
        call_id: String,
        saved_sequence: u64,
        append: agentelier_llm_model::HistoryAppend,
    },
    ResponseClosed {
        state: StoredState,
        response_sequence: u64,
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
            Self::ProjectPublished { state, .. }
            | Self::RequestPrepared { state }
            | Self::RequestProgress { state }
            | Self::Compacted { state, .. }
            | Self::QueueSuspended { state }
            | Self::Recovered { state, .. }
            | Self::Accepted { state }
            | Self::Response { state, .. }
            | Self::ToolStarted { state, .. }
            | Self::ToolReturned { state, .. }
            | Self::ResultAppended { state, .. }
            | Self::ResponseClosed { state, .. }
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
    #[serde(skip)]
    pub sequence: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<agentelier_llm_model::MemorySnapshot>,
    #[serde(default)]
    pub context_limits: Box<agentelier_llm_model::ContextLimits>,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub instructions: InstructionSnapshot,
    pub working_directory: PathBuf,
    #[serde(
        default,
        skip_serializing_if = "agentelier_tools::ToolContract::is_flat"
    )]
    pub tool_contract: agentelier_tools::ToolContract,
    pub tools: Vec<StoredTool>,
}

pub use agentelier_tools::ToolDefinition as StoredTool;

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
        tools: &agentelier_tools::ToolCatalog,
        memory: Option<agentelier_llm_model::MemorySnapshot>,
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
                memory,
                context_limits: view.settings.context_limits.clone(),
                slug: view.settings.slug.clone(),
                display_name: view.settings.display_name.clone(),
                effort: view.settings.effort.clone(),
                image_input: view.settings.image_input,
                instructions: (*view.settings.instructions).clone(),
                working_directory: view.settings.working_directory.clone(),
                tool_contract: tools.contract(),
                tools: tools.definitions().cloned().collect(),
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
