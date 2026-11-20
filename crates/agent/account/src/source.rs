use crate::{
    AccountFailure, CodexIdentity, CredentialIntent, SessionFailure, SessionFailureKind,
    UsableCredentials,
};
use agentelier_login::{CodexAccountSummary, CodexReadFailure, read_codex_credentials};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type SaveSource = dyn Fn(Option<&Path>) -> io::Result<()> + Send + Sync;

#[derive(Clone)]
pub struct SourceSettings {
    selected_file: Option<PathBuf>,
    save: Arc<SaveSource>,
}

impl SourceSettings {
    pub fn new(
        selected_file: Option<PathBuf>,
        save: impl Fn(Option<&Path>) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            selected_file,
            save: Arc::new(save),
        }
    }

    pub fn codex_selected(&self) -> bool {
        self.selected_file.is_some()
    }

    pub(crate) fn read(&self) -> AccountSource {
        self.selected_file
            .as_ref()
            .map_or(AccountSource::Siwc, |path| AccountSource::Codex {
                path: path.clone(),
                status: read_codex_credentials(path).map(|file| file.summary),
            })
    }

    pub(crate) fn acquire(
        &self,
        intent: CredentialIntent,
    ) -> Result<(AccountSource, Result<UsableCredentials, SessionFailure>), AccountFailure> {
        let path = self
            .selected_file
            .as_ref()
            .ok_or(AccountFailure::CredentialState)?;
        let (status, credentials) = match read_codex_credentials(path) {
            Ok(file) => (
                Ok(file.summary.clone()),
                UsableCredentials::from_codex(file),
            ),
            Err(error) => (
                Err(error),
                Err(SessionFailure::new(
                    SessionFailureKind::Identity,
                    error.to_string(),
                )),
            ),
        };
        let credentials = credentials.and_then(|current| {
            if let CredentialIntent::Recover(rejected) = intent {
                if !rejected.same_account(current.version()) {
                    return Err(SessionFailure::new(
                        SessionFailureKind::Identity,
                        "Codex 凭据的账号或工作区已变化，原请求已停止",
                    ));
                }
                if &rejected == current.version() {
                    return Err(SessionFailure::new(
                        SessionFailureKind::ReauthenticationRequired,
                        "Codex 文件尚未提供新的凭据，请在 Codex 中恢复授权",
                    ));
                }
            }
            Ok(current)
        });
        Ok((
            AccountSource::Codex {
                path: path.clone(),
                status,
            },
            credentials,
        ))
    }

    pub(crate) fn change(&mut self, change: SourceChange) -> io::Result<()> {
        let selected = match change {
            SourceChange::Select(path) if path.is_absolute() => Some(path),
            SourceChange::Select(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Codex 凭据文件必须使用绝对路径",
                ));
            }
            SourceChange::Disconnect => None,
        };
        if self.selected_file != selected {
            (self.save)(selected.as_deref())?;
            self.selected_file = selected;
        }
        Ok(())
    }
}

pub enum SourceChange {
    Select(PathBuf),
    Disconnect,
}

#[derive(Clone, PartialEq, Eq)]
pub enum AccountSource {
    Siwc,
    Codex {
        path: PathBuf,
        status: Result<CodexAccountSummary, CodexReadFailure>,
    },
}

impl AccountSource {
    pub fn codex_identity(&self) -> Option<&CodexIdentity> {
        match self {
            Self::Codex {
                status: Ok(summary),
                ..
            } if !summary.expired => Some(&summary.identity),
            _ => None,
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Siwc => None,
            Self::Codex { path, .. } => Some(path),
        }
    }
}
