use super::{ProtocolHistory, ProtocolItem};
use crate::ModelFailure;
use crate::model::protocol;
use agentelier_tools::ToolPayload;

const TEXT_BYTES_PER_TOKEN: u64 = 4;
const RESIZED_IMAGE_ESTIMATE_BYTES: u64 = 7373;
const REASONING_ENVELOPE_ESTIMATE_BYTES: u64 = 650;

pub fn text_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(TEXT_BYTES_PER_TOKEN)
}

fn item_tokens(item: &ProtocolItem) -> u64 {
    match item {
        ProtocolItem::User(text) => text_tokens(text),
        ProtocolItem::Assistant { raw, .. } => raw
            .get("content")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|part| {
                text_tokens(
                    part.get("text")
                        .or_else(|| part.get("refusal"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .fold(0, u64::saturating_add),
        ProtocolItem::FunctionCall { raw, name, .. } => {
            text_tokens(name).saturating_add(text_tokens(
                raw.get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ))
        }
        ProtocolItem::FunctionResult { call_id, output } => {
            text_tokens(call_id).saturating_add(match output {
                ToolPayload::Text(text) => text_tokens(text),
                ToolPayload::Image { summary, .. } => text_tokens(summary)
                    .saturating_add(RESIZED_IMAGE_ESTIMATE_BYTES.div_ceil(TEXT_BYTES_PER_TOKEN)),
            })
        }
        ProtocolItem::Reasoning { raw, .. } => {
            let encoded = raw
                .get("encrypted_content")
                .and_then(serde_json::Value::as_str)
                .map_or(0, str::len) as u64;
            // This envelope heuristic is an estimate, never a tokenizer count or decrypted content.
            (encoded.saturating_mul(3) / 4)
                .saturating_sub(REASONING_ENVELOPE_ESTIMATE_BYTES)
                .div_ceil(TEXT_BYTES_PER_TOKEN)
        }
    }
}
impl ProtocolHistory {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn estimated_tokens_since(&self, boundary: usize) -> Result<u64, ModelFailure> {
        let tail = self
            .0
            .get(boundary..)
            .ok_or_else(|| protocol("上下文用量边界超过工作历史"))?;
        Ok(tail.iter().map(item_tokens).fold(0, u64::saturating_add))
    }
}
impl super::ProtocolOutput {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
