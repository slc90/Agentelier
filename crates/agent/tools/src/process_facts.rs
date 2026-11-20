use crate::{ToolResult, ToolStatus};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOrigin {
    pub session: Uuid,
    pub turn: u64,
    pub request: u32,
    pub call_id: String,
    pub task_id: Option<Uuid>,
    pub instance: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProcessFact {
    pub session_id: String,
    pub origin: ProcessOrigin,
    pub change: ProcessChange,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessChange {
    Started,
    Exited {
        status: ToolStatus,
        cleanup_confirmed: bool,
        output: String,
    },
    Consumed,
}

impl ProcessChange {
    pub(crate) fn exited(result: &ToolResult) -> Self {
        Self::Exited {
            status: result.status,
            cleanup_confirmed: !result.cleanup_failed,
            output: result.output.summary().to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPhase {
    Running,
    Exited,
    CleanupUnconfirmed,
    HistoricalUnconfirmed,
    Consumed,
}

impl ProcessPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::CleanupUnconfirmed => "cleanup_unconfirmed",
            Self::HistoricalUnconfirmed => "historical_unconfirmed",
            Self::Consumed => "consumed",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ProcessView {
    pub session_id: String,
    pub origin: ProcessOrigin,
    pub phase: ProcessPhase,
    pub exit_code: Option<i64>,
    pub truncated: bool,
    pub decoding_replaced: bool,
    pub readable: bool,
}

impl ProcessView {
    pub fn registered(fact: &ProcessFact) -> Self {
        Self {
            origin: fact.origin.clone(),
            session_id: fact.session_id.clone(),
            phase: ProcessPhase::Running,
            exit_code: None,
            truncated: false,
            decoding_replaced: false,
            readable: false,
        }
    }

    pub fn apply(&mut self, change: &ProcessChange) {
        match change {
            ProcessChange::Started => {}
            ProcessChange::Exited {
                cleanup_confirmed,
                output,
                ..
            } => {
                let body = serde_json::from_str::<serde_json::Value>(output).unwrap_or_default();
                let data = &body["data"];
                self.exit_code = data["exit_code"].as_i64();
                self.truncated = data["truncated"].as_bool().unwrap_or(false);
                self.decoding_replaced = data["decoding_replaced"].as_bool().unwrap_or(false);
                self.phase = if *cleanup_confirmed {
                    ProcessPhase::Exited
                } else {
                    ProcessPhase::CleanupUnconfirmed
                };
                self.readable = *cleanup_confirmed;
            }
            ProcessChange::Consumed => {
                self.phase = ProcessPhase::Consumed;
                self.readable = false;
            }
        }
    }
}

pub type SaveProcessFact = Arc<
    dyn Fn(ProcessFact) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>> + Send + Sync,
>;
