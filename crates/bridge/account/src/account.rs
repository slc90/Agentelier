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
    LoginWaiting(Option<ManualLogin>),
    LoginCancelled,
    SourceSettling,
    SourceChanged(Result<SourceSnapshot, BridgeFailure>),
    LogoutFinished {
        source: SourceSnapshot,
        remote_warning: Option<String>,
    },
}

pub struct ManualLogin {
    pub authorization_url: String,
    pub reason: String,
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
            AccountEventKind::LoginWaiting(match prompt {
                agentelier_account::LoginPrompt::Opened => None,
                agentelier_account::LoginPrompt::Manual {
                    authorization_url,
                    reason,
                } => Some(ManualLogin {
                    authorization_url,
                    reason,
                }),
            })
        }
        agentelier_account::AccountEventKind::LoginCancelled => AccountEventKind::LoginCancelled,
        agentelier_account::AccountEventKind::SourceSettling => AccountEventKind::SourceSettling,
        agentelier_account::AccountEventKind::SourceChanged(result) => {
            AccountEventKind::SourceChanged(result.map(convert_source).map_err(BridgeFailure))
        }
        agentelier_account::AccountEventKind::LogoutFinished {
            source,
            remote_warning,
        } => AccountEventKind::LogoutFinished {
            source: convert_source(source),
            remote_warning,
        },
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
