use crate::{SessionId, TurnId};

pub const MAX_PENDING_STEERS: usize = 8;

pub struct SteerInput {
    pub id: u64,
    pub session: SessionId,
    pub turn: TurnId,
    pub operation_id: u64,
    pub text: String,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum SteerState {
    Accepted,
    Applied,
    NotApplied,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SteerView {
    pub id: u64,
    pub text: String,
    pub state: SteerState,
    pub applied_request: Option<u32>,
}
pub enum InputDecision {
    Accepted,
    Applied,
    Rejected(String),
}
pub struct InputReceipt {
    pub id: u64,
    pub session: SessionId,
    pub decision: InputDecision,
}
