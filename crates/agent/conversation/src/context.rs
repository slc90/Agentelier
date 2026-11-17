use crate::ConversationFailure;
use agentelier_llm_model::{ContextLimits, ProtocolHistory, ResponseUsage, estimated_text_tokens};
use agentelier_prompts::InstructionSnapshot;
use agentelier_tools::ToolCatalog;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct UsageFact {
    pub window: u64,
    pub turn: u64,
    pub request: u32,
    pub response_id: Option<String>,
    pub usage: ResponseUsage,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct UsageAnchor {
    #[serde(default)]
    pub task_material_tokens: u64,
    pub fact: UsageFact,
    pub covered_items: usize,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ContextState {
    #[serde(default)]
    pub task_material_tokens: u64,
    pub window: u64,
    pub limits: ContextLimits,
    pub latest: Option<UsageFact>,
    pub anchor: Option<UsageAnchor>,
    pub tokens: u64,
    pub estimated_tokens: u64,
}
impl ContextState {
    pub fn refresh(
        &mut self,
        history: &ProtocolHistory,
        instructions: &InstructionSnapshot,
        tools: &ToolCatalog,
    ) -> Result<(), ConversationFailure> {
        let (base, boundary) = if let Some(anchor) = &self.anchor {
            if anchor.fact.window != self.window || anchor.covered_items > history.len() {
                return Err(ConversationFailure::new("上下文计量锚点与工作窗口不匹配"));
            }
            (
                anchor
                    .fact
                    .usage
                    .total_tokens
                    .saturating_sub(anchor.task_material_tokens)
                    .saturating_add(self.task_material_tokens),
                anchor.covered_items,
            )
        } else {
            let mut fixed = instructions
                .messages()
                .iter()
                .map(|message| {
                    estimated_text_tokens(&message.text)
                        .saturating_add(estimated_text_tokens(&message.role))
                })
                .fold(0, u64::saturating_add);
            for tool in tools.definitions() {
                let parameters = serde_json::to_string(&tool.parameters)
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                fixed = fixed
                    .saturating_add(estimated_text_tokens(tool.name))
                    .saturating_add(estimated_text_tokens(&tool.description))
                    .saturating_add(estimated_text_tokens(&parameters));
            }
            (fixed.saturating_add(self.task_material_tokens), 0)
        };
        let tail = history
            .estimated_tokens_since(boundary)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.tokens = base.saturating_add(tail);
        self.estimated_tokens = if self.anchor.is_some() {
            tail
        } else {
            self.tokens
        };
        Ok(())
    }
}
