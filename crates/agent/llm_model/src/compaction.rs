use crate::ModelFailure;
use crate::model::protocol;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Opaque protocol content must remain unchanged across persistence and replay.
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
            return Err(protocol("压缩项结构或不透明内容无效"));
        }
        Ok(Self(raw))
    }
}
impl<'de> Deserialize<'de> for CompactionItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Clone)]
pub struct CompactionMetadata {
    pub response_id: String,
    pub compatibility: Value,
}
