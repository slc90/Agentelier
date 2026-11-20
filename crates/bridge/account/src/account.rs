use std::{error::Error, fmt, path::PathBuf};

pub enum SourceSnapshot {
    Disconnected,
    Codex {
        path: PathBuf,
        status: Result<CodexSummary, String>,
    },
}

pub struct CodexSummary {
    pub subject: String,
    pub workspace_id: String,
    pub email: Option<String>,
    pub expired: bool,
}

pub enum AccountEventKind {
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
        agentelier_account::AccountEventKind::SourceSettling => AccountEventKind::SourceSettling,
        agentelier_account::AccountEventKind::SourceChanged(result) => {
            AccountEventKind::SourceChanged(result.map(convert_source).map_err(BridgeFailure))
        }
    }
}

fn convert_source(source: agentelier_account::AccountSource) -> SourceSnapshot {
    match source {
        agentelier_account::AccountSource::Disconnected => SourceSnapshot::Disconnected,
        agentelier_account::AccountSource::Codex { path, status } => SourceSnapshot::Codex {
            path,
            status: status
                .map(|summary| CodexSummary {
                    subject: summary.identity.subject,
                    workspace_id: summary.identity.workspace_id,
                    email: summary.email,
                    expired: summary.expired,
                })
                .map_err(|failure| failure.to_string()),
        },
    }
}
