use super::{ProtocolHistory, ProtocolItem, ProtocolOutput};
use agentelier_tools::{ImageDescriptor, ImagePayload, ToolPayload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArchivedPayload {
    Text {
        text: String,
    },
    Image {
        summary: String,
        image: ImageDescriptor,
    },
}

impl ArchivedPayload {
    pub fn capture(payload: &ToolPayload) -> Self {
        match payload {
            ToolPayload::Text(text) => Self::Text { text: text.clone() },
            ToolPayload::Image { summary, image } => Self::Image {
                summary: summary.clone(),
                image: image.descriptor(),
            },
        }
    }

    fn restore(
        self,
        load: &mut impl FnMut(&ImageDescriptor, &str) -> Result<Arc<ImagePayload>, crate::ModelFailure>,
    ) -> Result<ToolPayload, crate::ModelFailure> {
        Ok(match self {
            Self::Text { text } => ToolPayload::Text(text),
            Self::Image { summary, image } => {
                let payload = load(&image, &summary)?;
                if payload.descriptor() != image {
                    return Err(crate::model::protocol("恢复附件与协议引用不一致"));
                }
                ToolPayload::Image {
                    summary,
                    image: payload,
                }
            }
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArchivedItem {
    Compaction {
        item: crate::CompactionItem,
    },
    User {
        text: String,
    },
    FunctionCall {
        raw: Value,
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
        output: ArchivedPayload,
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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProtocolArchive {
    items: Vec<ArchivedItem>,
}

impl ProtocolArchive {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn validate_compaction(&self) -> Result<(), crate::ModelFailure> {
        if !matches!(self.items.last(), Some(ArchivedItem::Compaction { .. }))
            || self.items[..self.items.len().saturating_sub(1)]
                .iter()
                .any(|item| !matches!(item, ArchivedItem::User { .. }))
        {
            return Err(crate::model::protocol(
                "压缩替换历史必须是有序用户消息与唯一末尾压缩项",
            ));
        }
        Ok(())
    }

    pub fn call_ids(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            ArchivedItem::FunctionCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
    }

    pub fn seal_interrupted(
        &mut self,
        output: ProtocolArchive,
        mut results: std::collections::BTreeMap<String, ArchivedPayload>,
        started: &std::collections::HashSet<String>,
        input: Option<&str>,
    ) -> Result<(), crate::ModelFailure> {
        let calls = output.call_ids().map(str::to_owned).collect::<Vec<_>>();
        if calls.is_empty() {
            return Err(crate::model::protocol(
                "未完成的普通输出不能作为工具步骤恢复",
            ));
        }
        let unique = calls.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != calls.len()
            || results.keys().any(|id| !calls.contains(id))
            || started.iter().any(|id| !calls.contains(id))
            || output.items.iter().any(|item| {
                matches!(
                    item,
                    ArchivedItem::User { .. }
                        | ArchivedItem::FunctionResult { .. }
                        | ArchivedItem::Compaction { .. }
                )
            })
        {
            return Err(crate::model::protocol("中断响应中的函数身份或结果不一致"));
        }
        if let Some(input) = input {
            self.items.push(ArchivedItem::User { text: input.into() });
        }
        self.items.extend(output.items);
        for call_id in calls {
            let output = results.remove(&call_id).unwrap_or_else(|| ArchivedPayload::Text { text: serde_json::json!({"ok":false,"error":{"code":"aborted","message":if started.contains(&call_id) {"上次运行中断，执行结果未确认；不表示文件副作用已撤销"} else {"上次运行中断，此调用未执行"}}}).to_string() });
            self.items
                .push(ArchivedItem::FunctionResult { call_id, output });
        }
        Ok(())
    }
}

fn capture(items: &[ProtocolItem]) -> ProtocolArchive {
    ProtocolArchive {
        items: items
            .iter()
            .map(|item| match item {
                ProtocolItem::User(text) => ArchivedItem::User { text: text.clone() },
                ProtocolItem::Compaction(item) => ArchivedItem::Compaction { item: item.clone() },
                ProtocolItem::FunctionCall { raw, call_id, name } => ArchivedItem::FunctionCall {
                    raw: raw.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                },
                ProtocolItem::FunctionResult { call_id, output } => ArchivedItem::FunctionResult {
                    call_id: call_id.clone(),
                    output: ArchivedPayload::capture(output),
                },
                ProtocolItem::Assistant { raw, phase } => ArchivedItem::Assistant {
                    raw: raw.clone(),
                    phase: phase.clone(),
                },
                ProtocolItem::Reasoning { raw, encrypted } => ArchivedItem::Reasoning {
                    raw: raw.clone(),
                    encrypted: *encrypted,
                },
            })
            .collect(),
    }
}

impl ProtocolHistory {
    pub fn archive(&self) -> ProtocolArchive {
        capture(&self.0)
    }

    pub fn from_archive(
        archive: ProtocolArchive,
        mut load: impl FnMut(&ImageDescriptor, &str) -> Result<Arc<ImagePayload>, crate::ModelFailure>,
    ) -> Result<Self, crate::ModelFailure> {
        let mut items = Vec::with_capacity(archive.items.len());
        let mut pending = std::collections::VecDeque::new();
        for item in archive.items {
            let item = match item {
                ArchivedItem::Compaction { item } => {
                    if !pending.is_empty() {
                        return Err(crate::model::protocol("压缩项插入未封闭调用"));
                    }
                    ProtocolItem::Compaction(item)
                }
                ArchivedItem::User { text } => {
                    if !pending.is_empty() {
                        return Err(crate::model::protocol("历史用户消息插入未封闭调用"));
                    }
                    ProtocolItem::User(text)
                }
                ArchivedItem::FunctionCall { raw, call_id, name } => {
                    if call_id.is_empty()
                        || name.is_empty()
                        || raw.get("type").and_then(Value::as_str) != Some("function_call")
                        || raw.get("call_id").and_then(Value::as_str) != Some(call_id.as_str())
                        || raw.get("name").and_then(Value::as_str) != Some(name.as_str())
                        || !raw.get("arguments").is_some_and(Value::is_string)
                        || pending.contains(&call_id)
                    {
                        return Err(crate::model::protocol("历史函数调用身份或协议不一致"));
                    }
                    pending.push_back(call_id.clone());
                    ProtocolItem::FunctionCall { raw, call_id, name }
                }
                ArchivedItem::FunctionResult { call_id, output } => {
                    if pending.pop_front().as_deref() != Some(call_id.as_str()) {
                        return Err(crate::model::protocol("历史函数结果未按调用身份封闭"));
                    }
                    ProtocolItem::FunctionResult {
                        call_id,
                        output: output.restore(&mut load)?,
                    }
                }
                ArchivedItem::Assistant { raw, phase } => {
                    if raw.get("type").and_then(Value::as_str) != Some("message")
                        || raw.get("role").and_then(Value::as_str) != Some("assistant")
                        || !raw.get("content").is_some_and(Value::is_array)
                        || raw.get("phase").and_then(Value::as_str) != phase.as_deref()
                    {
                        return Err(crate::model::protocol("历史助手协议项无效"));
                    }
                    ProtocolItem::Assistant { raw, phase }
                }
                ArchivedItem::Reasoning { raw, encrypted } => {
                    if raw.get("type").and_then(Value::as_str) != Some("reasoning")
                        || !encrypted
                        || !raw
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                    {
                        return Err(crate::model::protocol("历史 reasoning 项不可续接"));
                    }
                    ProtocolItem::Reasoning { raw, encrypted }
                }
            };
            items.push(item);
        }
        if !pending.is_empty() {
            return Err(crate::model::protocol("历史函数调用缺少匹配结果"));
        }
        Ok(Self(items))
    }
}

impl ProtocolOutput {
    pub fn archive(&self) -> ProtocolArchive {
        capture(&self.0)
    }
}
