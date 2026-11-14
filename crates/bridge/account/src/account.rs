use std::{error::Error, fmt};

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub enum CredentialState {
    NoRecord,
    HostOnly,
    SignedOut,
    ReauthenticationRequired,
    StoredSession {
        expired: bool,
        subscription_enabled: bool,
    },
}
#[derive(Debug)]
pub struct AccountSnapshot {
    pub account: Option<AccountSummary>,
    pub credentials: CredentialState,
}
pub struct AuthorizationLink(String);
impl AuthorizationLink {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
pub enum AccountEventKind {
    SignOutProgress(SignOutStage),
    SignOutFailed(BridgeFailure),
    SignedOut {
        snapshot: AccountSnapshot,
        remote: RemoteRevocation,
    },
    SessionStatus {
        snapshot: AccountSnapshot,
        activity: SessionActivity,
        failure: Option<SessionFailure>,
    },
    BrowserReady(AuthorizationLink),
    Exchanging,
    Saving,
    SaveFailed(BridgeFailure),
    Completed(Result<AccountSnapshot, BridgeFailure>),
    Cancelled,
}
#[derive(Clone, Copy, Debug)]
pub enum SignOutStage {
    Settling,
    Revoking,
    Clearing,
}
#[derive(Clone, Debug)]
pub enum RemoteRevocation {
    Confirmed,
    Unconfirmed(String),
    NotApplicable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionActivity {
    Idle,
    Checking,
    Refreshing,
    Saving,
    SaveFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionFailureKind {
    Unavailable,
    Temporary,
    Uncertain,
    ReauthenticationRequired,
    ClientConfiguration,
    Storage,
}
#[derive(Clone, Debug)]
pub struct SessionFailure {
    pub kind: SessionFailureKind,
    pub retry_at: Option<u64>,
    pub message: String,
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
        agentelier_account::AccountEventKind::SignOutProgress(stage) => {
            AccountEventKind::SignOutProgress(match stage {
                agentelier_account::SignOutStage::Settling => SignOutStage::Settling,
                agentelier_account::SignOutStage::Revoking => SignOutStage::Revoking,
                agentelier_account::SignOutStage::Clearing => SignOutStage::Clearing,
            })
        }
        agentelier_account::AccountEventKind::SignOutFailed(failure) => {
            AccountEventKind::SignOutFailed(BridgeFailure(failure.to_string()))
        }
        agentelier_account::AccountEventKind::SignedOut { snapshot, remote } => {
            AccountEventKind::SignedOut {
                snapshot: convert_snapshot(snapshot),
                remote: match remote {
                    agentelier_account::RemoteRevocation::Confirmed => RemoteRevocation::Confirmed,
                    agentelier_account::RemoteRevocation::Unconfirmed(reason) => {
                        RemoteRevocation::Unconfirmed(reason)
                    }
                    agentelier_account::RemoteRevocation::NotApplicable => {
                        RemoteRevocation::NotApplicable
                    }
                },
            }
        }
        agentelier_account::AccountEventKind::SessionStatus {
            snapshot,
            activity,
            failure,
        } => AccountEventKind::SessionStatus {
            snapshot: convert_snapshot(snapshot),
            activity: match activity {
                agentelier_account::SessionActivity::Idle => SessionActivity::Idle,
                agentelier_account::SessionActivity::Checking => SessionActivity::Checking,
                agentelier_account::SessionActivity::Refreshing => SessionActivity::Refreshing,
                agentelier_account::SessionActivity::Saving => SessionActivity::Saving,
                agentelier_account::SessionActivity::SaveFailed => SessionActivity::SaveFailed,
            },
            failure: failure.map(|failure| SessionFailure {
                kind: match failure.kind {
                    agentelier_account::SessionFailureKind::Temporary => {
                        SessionFailureKind::Temporary
                    }
                    agentelier_account::SessionFailureKind::Uncertain
                    | agentelier_account::SessionFailureKind::Identity => {
                        SessionFailureKind::Uncertain
                    }
                    agentelier_account::SessionFailureKind::ReauthenticationRequired => {
                        SessionFailureKind::ReauthenticationRequired
                    }
                    agentelier_account::SessionFailureKind::ClientConfiguration => {
                        SessionFailureKind::ClientConfiguration
                    }
                    agentelier_account::SessionFailureKind::Storage => SessionFailureKind::Storage,
                    _ => SessionFailureKind::Unavailable,
                },
                retry_at: failure.retry_at,
                message: failure.to_string(),
            }),
        },
        agentelier_account::AccountEventKind::BrowserReady(url) => {
            AccountEventKind::BrowserReady(AuthorizationLink(url))
        }
        agentelier_account::AccountEventKind::Exchanging => AccountEventKind::Exchanging,
        agentelier_account::AccountEventKind::Saving => AccountEventKind::Saving,
        agentelier_account::AccountEventKind::SaveFailed(failure) => {
            AccountEventKind::SaveFailed(BridgeFailure(failure.to_string()))
        }
        agentelier_account::AccountEventKind::Cancelled => AccountEventKind::Cancelled,
        agentelier_account::AccountEventKind::Completed(result) => AccountEventKind::Completed(
            result
                .map(convert_snapshot)
                .map_err(|error| BridgeFailure(error.to_string())),
        ),
    }
}

fn convert_snapshot(restored: agentelier_account::RestoredAccount) -> AccountSnapshot {
    let account = restored.account.map(|account| AccountSummary {
        subject: account.subject,
        display_name: account.display_name,
        email: account.email,
    });
    let credentials = match restored.credentials {
        agentelier_account::CredentialState::NoRecord => CredentialState::NoRecord,
        agentelier_account::CredentialState::HostOnly => CredentialState::HostOnly,
        agentelier_account::CredentialState::SignedOut => CredentialState::SignedOut,
        agentelier_account::CredentialState::ReauthenticationRequired => {
            CredentialState::ReauthenticationRequired
        }
        agentelier_account::CredentialState::StoredSession {
            expired,
            subscription_enabled,
        } => CredentialState::StoredSession {
            expired,
            subscription_enabled,
        },
    };
    AccountSnapshot {
        account,
        credentials,
    }
}
