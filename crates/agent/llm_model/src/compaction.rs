use crate::model::protocol;
use crate::{ModelFailure, ResponseUsage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Opaque Remote V2 output. Its original protocol fields are retained without logging or projection.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct CompactionItem(pub(crate) Value);
impl CompactionItem {
    pub(crate) fn parse(raw: Value) -> Result<Self, ModelFailure> {
        if raw.get("type").and_then(Value::as_str) != Some("compaction")
            || raw
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            || raw
                .get("id")
                .is_some_and(|id| !id.is_null() && id.as_str().is_none_or(str::is_empty))
        {
            return Err(protocol("Remote V2 压缩项结构或不透明内容无效"));
        }
        Ok(Self(raw))
    }
}
impl<'de> Deserialize<'de> for CompactionItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
pub struct CompactionResult {
    pub item: CompactionItem,
    pub response_id: String,
    pub usage: Option<ResponseUsage>,
    pub compatibility: Value,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestPurpose {
    Generation,
    Compaction,
}
pub(crate) enum StreamResult {
    Generation(crate::GenerationResult),
    Compaction(CompactionResult),
}
