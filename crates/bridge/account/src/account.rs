use std::{error::Error, fmt};

pub enum SourceSnapshot {
    Disconnected,
    Codex(CodexSummary),
    Unavailable(String),
}

pub struct CodexSummary {
    pub subject: String,
    pub workspace_id: String,
    pub email: Option<String>,
}

pub enum AccountEventKind {
    LoginWaiting {
        authorization_url: String,
        browser_error: Option<String>,
    },
    SourceSettling,
    SourceChanged(Result<SourceSnapshot, BridgeFailure>),
}

#[derive(Clone, Debug)]
pub struct BridgeFailure(String);
impl fmt::Display for BridgeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for BridgeFailure {}

pub fn convert(kind: agentelier_account::AccountEventKind) -> AccountEventKind {
    match kind {
        agentelier_account::AccountEventKind::LoginWaiting(prompt) => {
            AccountEventKind::LoginWaiting {
                authorization_url: prompt.authorization_url,
                browser_error: prompt.browser_error,
            }
        }
        agentelier_account::AccountEventKind::SourceSettling => AccountEventKind::SourceSettling,
        agentelier_account::AccountEventKind::SourceChanged(result) => {
            AccountEventKind::SourceChanged(result.map(convert_source).map_err(BridgeFailure))
        }
    }
}

fn convert_source(source: agentelier_account::AccountSource) -> SourceSnapshot {
    match source {
        agentelier_account::AccountSource::Disconnected => SourceSnapshot::Disconnected,
        agentelier_account::AccountSource::Codex(summary) => SourceSnapshot::Codex(CodexSummary {
            subject: summary.identity.subject,
            workspace_id: summary.identity.workspace_id,
            email: summary.email,
        }),
        agentelier_account::AccountSource::Unavailable(failure) => {
            SourceSnapshot::Unavailable(failure)
        }
    }
}
