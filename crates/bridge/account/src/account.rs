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
    LoginWaiting(ManualLogin),
    LoginCancelled,
    SourceSettling,
    SourceChanged(Result<SourceSnapshot, BridgeFailure>),
    Quota(Box<QuotaStatus>),
    LogoutFinished {
        source: SourceSnapshot,
        remote_warning: Option<String>,
    },
}

#[derive(Clone)]
pub struct QuotaWindow {
    pub remaining_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone)]
pub struct QuotaStatus {
    pub subject: String,
    pub workspace_id: String,
    pub fetched_at: Option<u64>,
    pub five_hours: Option<QuotaWindow>,
    pub week: Option<QuotaWindow>,
    pub stale: bool,
}

pub struct ManualLogin {
    pub authorization_url: String,
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
            AccountEventKind::LoginWaiting(ManualLogin {
                authorization_url: prompt.authorization_url,
            })
        }
        agentelier_account::AccountEventKind::LoginCancelled => AccountEventKind::LoginCancelled,
        agentelier_account::AccountEventKind::SourceSettling => AccountEventKind::SourceSettling,
        agentelier_account::AccountEventKind::SourceChanged(result) => {
            AccountEventKind::SourceChanged(result.map(convert_source).map_err(BridgeFailure))
        }
        agentelier_account::AccountEventKind::Quota(update) => {
            let agentelier_account::QuotaUpdate {
                identity,
                snapshot,
                stale,
            } = *update;
            let (fetched_at, five_hours, week) = snapshot.map_or((None, None, None), |snapshot| {
                (
                    Some(snapshot.fetched_at),
                    snapshot.five_hours,
                    snapshot.week,
                )
            });
            AccountEventKind::Quota(Box::new(QuotaStatus {
                subject: identity.subject,
                workspace_id: identity.workspace_id,
                fetched_at,
                five_hours: five_hours.map(quota_window),
                week: week.map(quota_window),
                stale,
            }))
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

fn quota_window(window: agentelier_account::QuotaWindow) -> QuotaWindow {
    QuotaWindow {
        remaining_percent: 100.0 - window.used_percent,
        resets_at: window.resets_at,
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
