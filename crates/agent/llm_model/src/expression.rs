use crate::{ModelFailure, model::protocol};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use uuid::Uuid;

const MAX_EXCERPTS: usize = 5;
const MAX_EXCERPT_CHARS: usize = 48;
const MAX_TOTAL_CHARS: usize = 320;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionExcerpt {
    pub turn: u64,
    pub message: String,
    pub part: usize,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionContext {
    pub session: Uuid,
    pub window: u64,
    pub directory: PathBuf,
    pub excerpts: Vec<ExpressionExcerpt>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ExpressionMaterial {
    pub context: ExpressionContext,
    pub input_index: usize,
    pub text: String,
    pub hash: String,
}

impl ExpressionContext {
    fn validate(&self) -> Result<(), ModelFailure> {
        let mut seen = HashSet::new();
        if self.session.is_nil()
            || self.window == 0
            || !self.directory.is_absolute()
            || self.excerpts.is_empty()
            || self.excerpts.len() > MAX_EXCERPTS
            || self
                .excerpts
                .iter()
                .map(|item| item.text.chars().count())
                .sum::<usize>()
                > MAX_TOTAL_CHARS
            || self.excerpts.iter().any(|item| {
                item.turn == 0
                    || item.message.is_empty()
                    || !seen.insert(item.turn)
                    || expression_excerpt(&item.text).as_deref() != Some(item.text.as_str())
            })
        {
            return Err(protocol("表达连贯材料超限或来源标识无效"));
        }
        Ok(())
    }

    fn render(&self) -> String {
        let excerpts = self
            .excerpts
            .iter()
            .map(|item| &item.text)
            .collect::<Vec<_>>();
        format!(
            "These short openings are historical excerpts of public replies from the current conversation before compaction. Use them only to keep the exchange coherent and avoid accidentally repeating an opening. Preserve any terminology the current task requires.\n{}",
            serde_json::json!(excerpts)
        )
    }

    pub fn estimated_tokens(&self) -> u64 {
        crate::estimated_text_tokens(&self.render())
            .saturating_add(crate::estimated_text_tokens("developer"))
    }
}

impl ExpressionMaterial {
    pub(crate) fn new(
        context: ExpressionContext,
        input_index: usize,
    ) -> Result<Self, ModelFailure> {
        context.validate()?;
        let text = context.render();
        let hash = agentelier_prompts::content_hash(&text);
        Ok(Self {
            context,
            input_index,
            text,
            hash,
        })
    }

    pub(crate) fn validate(&self, index: usize) -> Result<(), ModelFailure> {
        self.context.validate()?;
        if self.input_index != index
            || self.text != self.context.render()
            || self.hash != agentelier_prompts::content_hash(&self.text)
        {
            return Err(protocol("表达连贯材料的位置或摘要不一致"));
        }
        Ok(())
    }
}

pub fn expression_excerpt(text: &str) -> Option<String> {
    let first = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let lower = first.to_lowercase();
    if first.starts_with(['#', '-', '*', '>', '"', '\''])
        || first
            .chars()
            .any(|character| character.is_ascii_digit() || character.is_control())
        || first.contains(['`', '\\', '/', '@', '=', '{', '}', '[', ']', '<', '>'])
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word.len() >= 24)
        || [
            "password",
            "passwd",
            "secret",
            "authorization",
            "bearer",
            "cookie",
            "credential",
            "access token",
            "token",
            "api key",
            "session key",
            "private",
            "contact",
            "address",
            "phone",
            "diagnosis",
            "口令",
            "密码",
            "令牌",
            "密钥",
            "私密",
            "地址",
            "住址",
            "电话",
            "诊断",
            "身份证",
            "银行卡",
            "病历",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return None;
    }
    let end = first
        .char_indices()
        .nth(MAX_EXCERPT_CHARS)
        .map_or(first.len(), |(end, _)| end);
    let prefix = first[..end].trim_end();
    (prefix
        .chars()
        .filter(|character| character.is_alphabetic())
        .take(4)
        .count()
        >= 4)
        .then(|| prefix.to_owned())
}
