use super::{ProtocolHistory, ProtocolItem};
use crate::ModelFailure;
use crate::model::protocol;
use agentelier_tools::ToolPayload;
use serde_json::{Value, json};

pub(super) const TEXT_BYTES_PER_TOKEN: u64 = 4;
const RESIZED_IMAGE_ESTIMATE_BYTES: u64 = 7373;
const REASONING_ENVELOPE_ESTIMATE_BYTES: u64 = 650;
const TOOL_RESULT_ENVELOPE_ESTIMATE_TOKENS: u64 = 16;

pub fn text_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(TEXT_BYTES_PER_TOKEN)
}

fn item_tokens(item: &ProtocolItem) -> u64 {
    match item {
        ProtocolItem::Project(item) => text_tokens(&item.text).saturating_add(text_tokens("user")),
        ProtocolItem::User(text) => text_tokens(text),
        ProtocolItem::Compaction(item) => (item
            .0
            .get("encrypted_content")
            .and_then(serde_json::Value::as_str)
            .map_or(0, str::len) as u64)
            .div_ceil(3)
            .div_ceil(TEXT_BYTES_PER_TOKEN),
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
            result_tokens(call_id, output.summary(), output.image().is_some())
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

pub fn result_tokens(call_id: &str, summary: &str, image: bool) -> u64 {
    result_token_budget(call_id, summary.len(), image)
}

pub fn result_token_budget(call_id: &str, text_bytes: usize, image: bool) -> u64 {
    text_tokens(call_id)
        .saturating_add(TOOL_RESULT_ENVELOPE_ESTIMATE_TOKENS)
        .saturating_add((text_bytes as u64).div_ceil(TEXT_BYTES_PER_TOKEN))
        .saturating_add(if image {
            RESIZED_IMAGE_ESTIMATE_BYTES.div_ceil(TEXT_BYTES_PER_TOKEN)
        } else {
            0
        })
}

#[derive(Default)]
struct CountBytes(usize);

impl std::io::Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn count(value: &Value) -> Result<usize, ModelFailure> {
    let mut bytes = CountBytes::default();
    serde_json::to_writer(&mut bytes, value).map_err(|failure| protocol(&failure.to_string()))?;
    Ok(bytes.0)
}

pub fn result_wire_bytes(call_id: &str, output: &ToolPayload) -> Result<usize, ModelFailure> {
    let (body, encoded_bytes) = match output {
        ToolPayload::Text(text) => (json!(text), 0),
        ToolPayload::GeneratedImage { .. } => return Err(protocol("生成原图尚未确认保存")),
        ToolPayload::Image { summary, image } => (
            json!([{"type":"input_text","text":summary},{"type":"input_image","image_url":format!("data:{};base64,", image.mime()),"detail":"high"}]),
            image.bytes().len().div_ceil(3).saturating_mul(4),
        ),
    };
    Ok(
        count(&json!({"type":"function_call_output","call_id":call_id,"output":body}))?
            .saturating_add(encoded_bytes)
            .saturating_add(1),
    )
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

    // Each input item contributes its JSON bytes and one separator in the enclosing request.
    pub fn input_wire_bytes(&self) -> Result<usize, ModelFailure> {
        self.0.iter().try_fold(0usize, |total, item| {
            let bytes = match item {
                ProtocolItem::Project(item) => count(&json!({"role":"user","content":item.text}))?,
                ProtocolItem::User(input) => count(&json!({"role":"user","content":input}))?,
                ProtocolItem::Compaction(item) => count(&item.0)?,
                ProtocolItem::Assistant { raw, .. }
                | ProtocolItem::Reasoning { raw, .. }
                | ProtocolItem::FunctionCall { raw, .. } => count(raw)?,
                ProtocolItem::FunctionResult { call_id, output } => {
                    return Ok(total.saturating_add(result_wire_bytes(call_id, output)?));
                }
            };
            Ok(total.saturating_add(bytes).saturating_add(1))
        })
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
