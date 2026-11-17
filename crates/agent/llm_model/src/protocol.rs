use agentelier_tools::ToolPayload;
use serde_json::Value;
mod archive;
mod estimate;
pub use archive::{ArchivedPayload, ProtocolArchive};
pub use estimate::text_tokens as estimated_text_tokens;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum ProtocolItemSummary {
    Compaction,
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}

#[derive(Clone)]
pub(crate) enum ProtocolItem {
    Compaction(crate::CompactionItem),
    User(String),
    FunctionCall {
        raw: Value,
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
        output: ToolPayload,
    },
    Assistant {
        raw: Value,
        phase: Option<String>,
    },
    Reasoning {
        raw: Value,
        encrypted: bool,
    },
}

pub struct ProtocolOutput(pub(crate) Vec<ProtocolItem>);

impl ProtocolOutput {
    pub(crate) fn is_replayable(&self) -> bool {
        self.0.iter().all(|item| {
            !matches!(
                item,
                ProtocolItem::Reasoning {
                    encrypted: false,
                    ..
                }
            )
        })
    }
}

#[derive(Clone, Default)]
pub struct ProtocolHistory(Vec<ProtocolItem>);

impl ProtocolHistory {
    pub fn with_user(&self, input: &str) -> Self {
        let mut history = self.clone();
        history.0.push(ProtocolItem::User(input.to_owned()));
        history
    }

    pub fn seal(
        &mut self,
        input: Option<&str>,
        output: ProtocolOutput,
        results: Vec<(String, ToolPayload)>,
    ) -> Result<(), crate::ModelFailure> {
        let calls: Vec<_> = output
            .0
            .iter()
            .filter_map(|item| match item {
                ProtocolItem::FunctionCall { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        if calls.len() != results.len()
            || calls
                .iter()
                .zip(&results)
                .any(|(expected, (actual, _))| *expected != actual)
        {
            return Err(crate::model::protocol("工具调用与结果未封闭"));
        }
        if let Some(input) = input {
            self.0.push(ProtocolItem::User(input.to_owned()));
        }
        self.0.extend(output.0);
        self.0.extend(
            results
                .into_iter()
                .map(|(call_id, output)| ProtocolItem::FunctionResult { call_id, output }),
        );
        Ok(())
    }

    pub fn summary(&self) -> Vec<ProtocolItemSummary> {
        self.0
            .iter()
            .map(|item| match item {
                ProtocolItem::User(_) => ProtocolItemSummary::User,
                ProtocolItem::Compaction(_) => ProtocolItemSummary::Compaction,
                ProtocolItem::FunctionCall { call_id, name, .. } => {
                    ProtocolItemSummary::FunctionCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                    }
                }
                ProtocolItem::FunctionResult { call_id, .. } => {
                    ProtocolItemSummary::FunctionResult {
                        call_id: call_id.clone(),
                    }
                }
                ProtocolItem::Assistant { phase, .. } => ProtocolItemSummary::Assistant {
                    phase: phase.clone(),
                },
                ProtocolItem::Reasoning { encrypted, .. } => ProtocolItemSummary::Reasoning {
                    encrypted: *encrypted,
                },
            })
            .collect()
    }

    pub fn image_bytes_with<'a>(&'a self, pending: impl Iterator<Item = &'a ToolPayload>) -> usize {
        let mut seen = std::collections::HashSet::new();
        self.0
            .iter()
            .filter_map(|item| match item {
                ProtocolItem::FunctionResult { output, .. } => output.image(),
                _ => None,
            })
            .chain(pending.filter_map(ToolPayload::image))
            .filter(|image| seen.insert(std::sync::Arc::as_ptr(image)))
            .map(|image| image.bytes().len())
            .sum()
    }

    pub(crate) fn into_input(self) -> Vec<Value> {
        self.0
            .into_iter()
            .map(|item| match item {
                ProtocolItem::Compaction(item) => item.0,
                ProtocolItem::User(input) => serde_json::json!({"role": "user", "content": input}),
                ProtocolItem::Assistant { raw, .. } | ProtocolItem::Reasoning { raw, .. } | ProtocolItem::FunctionCall {raw, ..} => raw,
                ProtocolItem::FunctionResult {call_id, output} => {
                    let body = match output {
                        ToolPayload::Text(text) => Value::String(text),
                        ToolPayload::Image {summary,image} => {
                            use base64::Engine;
                            let url = format!("data:{};base64,{}", image.mime(), base64::engine::general_purpose::STANDARD.encode(image.bytes()));
                            let (width,height) = image.dimensions();
                            tracing::debug!(call_id, sha256=image.digest(), mime=image.mime(), width, height, encoded_bytes=image.bytes().len(), "图片工具结果正在转换为协议内容数组");
                            serde_json::json!([{"type":"input_text","text":summary},{"type":"input_image","image_url":url,"detail":"high"}])
                        }
                    };
                    serde_json::json!({"type":"function_call_output","call_id":call_id,"output":body})
                },
            })
            .collect()
    }
}
