use crate::ModelFailure;
use crate::model::protocol;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_EFFECTIVE_PERCENT: u64 = 95;
const AUTO_COMPACT_PERCENT: u64 = 90;
const PERCENT: u64 = 100;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextLimits {
    pub window: Option<u64>,
    pub effective: Option<u64>,
    pub automatic: Option<u64>,
    pub comp_hash: Option<String>,
}
impl ContextLimits {
    pub(crate) fn parse(model: &Value) -> Result<Self, ModelFailure> {
        let optional = |name: &str| -> Result<Option<u64>, ModelFailure> {
            match model.get(name).filter(|value| !value.is_null()) {
                None => Ok(None),
                Some(value) => value
                    .as_u64()
                    .filter(|value| *value > 0)
                    .map(Some)
                    .ok_or_else(|| protocol(&format!("模型窗口字段 {name} 不是正整数"))),
            }
        };
        let window = optional("context_window")?;
        let maximum = optional("max_context_window")?;
        if window
            .zip(maximum)
            .is_some_and(|(window, maximum)| window > maximum)
        {
            return Err(protocol("模型上下文窗口超过声明的最大窗口"));
        }
        let window = window.or(maximum);
        let percent =
            optional("effective_context_window_percent")?.unwrap_or(DEFAULT_EFFECTIVE_PERCENT);
        if percent > PERCENT {
            return Err(protocol("模型有效窗口比例超过 100%"));
        }
        let effective =
            window.map(|window| window / PERCENT * percent + window % PERCENT * percent / PERCENT);
        if effective == Some(0) {
            return Err(protocol("模型有效窗口为零"));
        }
        let automatic = match (optional("auto_compact_token_limit")?, window) {
            (explicit, Some(window)) => {
                let derived = window / PERCENT * AUTO_COMPACT_PERCENT
                    + window % PERCENT * AUTO_COMPACT_PERCENT / PERCENT;
                Some(explicit.map_or(derived, |value| value.min(derived)))
            }
            (explicit, None) => explicit,
        };
        if automatic == Some(0) {
            return Err(protocol("模型自动压缩阈值为零"));
        }
        let comp_hash = match model.get("comp_hash").filter(|value| !value.is_null()) {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| protocol("模型压缩兼容标识无效"))?
                    .into(),
            ),
        };
        Ok(Self {
            window,
            effective,
            automatic,
            comp_hash,
        })
    }
    pub fn available(&self) -> bool {
        self.compact_threshold().is_some()
    }
    pub fn compact_threshold(&self) -> Option<u64> {
        const MIN_COMPACT_THRESHOLD: u64 = 1_000;
        self.automatic
            .or(self.effective)
            .map(|limit| {
                self.effective
                    .map_or(limit, |effective| limit.min(effective))
            })
            .filter(|limit| *limit >= MIN_COMPACT_THRESHOLD)
    }
    pub fn reached(&self, tokens: u64) -> bool {
        self.effective.is_some_and(|limit| tokens >= limit)
            || self.automatic.is_some_and(|limit| tokens >= limit)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub raw: Value,
}
impl ResponseUsage {
    pub(crate) fn parse(response: &Value) -> Result<Option<Self>, ModelFailure> {
        let Some(usage) = response.get("usage").filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let number = |name: &str| {
            usage
                .get(name)
                .and_then(Value::as_u64)
                .ok_or_else(|| protocol(&format!("响应 usage 缺少有效 {name}")))
        };
        let input_tokens = number("input_tokens")?;
        let output_tokens = number("output_tokens")?;
        let total_tokens = number("total_tokens")?;
        if input_tokens.checked_add(output_tokens) != Some(total_tokens) {
            return Err(protocol("响应 usage 总量与输入输出不一致"));
        }
        let detail = |group: &str, name: &str, maximum: u64| -> Result<Option<u64>, ModelFailure> {
            let Some(value) = usage
                .get(group)
                .and_then(|group| group.get(name))
                .filter(|value| !value.is_null())
            else {
                return Ok(None);
            };
            value
                .as_u64()
                .filter(|value| *value <= maximum)
                .map(Some)
                .ok_or_else(|| protocol("响应 usage 细项无效"))
        };
        Ok(Some(Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens: detail("input_tokens_details", "cached_tokens", input_tokens)?,
            reasoning_output_tokens: detail(
                "output_tokens_details",
                "reasoning_tokens",
                output_tokens,
            )?,
            raw: usage.clone(),
        }))
    }
}
