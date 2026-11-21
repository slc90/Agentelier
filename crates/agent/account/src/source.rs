use crate::CodexIdentity;
use agentelier_login::CodexAccountSummary;

#[derive(Clone, PartialEq, Eq)]
pub enum AccountSource {
    Disconnected,
    Codex(CodexAccountSummary),
    Unavailable(String),
}

impl AccountSource {
    pub fn codex_identity(&self) -> Option<&CodexIdentity> {
        match self {
            Self::Codex(summary) if !summary.expired => Some(&summary.identity),
            _ => None,
        }
    }
}
