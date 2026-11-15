use crate::conversation::ChatState;
use agentelier_conversation_bridge::{InputDecision, InputReceipt};
use bevy::prelude::*;

#[derive(Clone, Reflect)]
pub(crate) struct PendingDraft {
    pub id: u64,
    pub text: String,
    pub revision: u64,
}
#[derive(Clone, Copy, Reflect)]
pub enum ChatSteerState {
    Accepted,
    Applied,
    NotApplied,
}
#[derive(Clone, Reflect)]
pub struct ChatSteer {
    pub id: u64,
    pub text: String,
    pub state: ChatSteerState,
    pub applied_request: Option<u32>,
}
pub(crate) fn receipt(state: &mut ChatState, receipt: InputReceipt) -> Option<PendingDraft> {
    match receipt.decision {
        InputDecision::Accepted => {
            tracing::info!(input_id = receipt.id, "补充输入已接纳，等待生效");
            state.input_feedback =
                Some(format!("补充输入 {} 已接纳，等待步骤边界生效", receipt.id));
            state
                .pending_inputs
                .iter()
                .position(|input| input.id == receipt.id)
                .map(|index| state.pending_inputs.remove(index))
        }
        InputDecision::Applied => {
            tracing::info!(input_id = receipt.id, "补充输入已用于模型请求");
            state.input_feedback = Some(format!("补充输入 {} 已用于新的模型请求", receipt.id));
            None
        }
        InputDecision::Rejected(reason) => {
            tracing::info!(input_id = receipt.id, reason, "补充输入未被接纳");
            state.pending_inputs.retain(|input| input.id != receipt.id);
            state.input_feedback = Some(format!("补充输入未接纳：{reason}；草稿保留"));
            None
        }
    }
}
