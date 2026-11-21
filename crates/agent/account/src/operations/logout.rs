use super::{Account, AccountEffect, AccountEvent, AccountEventKind, AccountFailure};
use crate::source::AccountSource;
use agentelier_login::{CodexOAuthCredentials, CredentialStore};

pub struct LogoutAttempt {
    operation_id: u64,
    credentials: Option<CodexOAuthCredentials>,
    store: CredentialStore,
}

pub struct LogoutCompletion {
    remote_warning: Option<String>,
    local: Result<(), String>,
}

impl LogoutAttempt {
    pub async fn run(self) -> LogoutCompletion {
        let remote_warning = match &self.credentials {
            Some(credentials) => match credentials.revoke().await {
                Ok(()) => {
                    tracing::debug!(
                        operation_id = self.operation_id,
                        "已撤销远端 Codex OAuth 续期授权"
                    );
                    None
                }
                Err(failure) => {
                    tracing::warn!(operation_id = self.operation_id, error = %failure, "远端 Codex OAuth 撤销未完成");
                    Some(format!("远端授权撤销未完成：{failure}"))
                }
            },
            None => None,
        };
        let local = tokio::task::spawn_blocking(move || self.store.clear())
            .await
            .map_err(|_| "本地凭据清理任务异常结束".to_owned())
            .and_then(|result| result.map_err(|failure| failure.to_string()));
        LogoutCompletion {
            remote_warning,
            local,
        }
    }
}

impl Account {
    pub(super) fn begin_logout(
        &mut self,
        operation_id: u64,
    ) -> Result<AccountEffect, AccountFailure> {
        self.credentials.update_source(false, true)?;
        self.signing_out = true;
        Ok(AccountEffect::LogoutSettling(operation_id))
    }

    pub fn logout_attempt(&self, operation_id: u64) -> LogoutAttempt {
        let credentials = self
            .renewal
            .unsaved
            .as_ref()
            .map(|rotation| &rotation.replacement)
            .or(self.current.as_ref())
            .cloned();
        LogoutAttempt {
            operation_id,
            credentials,
            store: self.store.clone(),
        }
    }

    pub(super) fn complete_logout(
        &mut self,
        operation_id: u64,
        result: Result<LogoutCompletion, String>,
    ) -> Result<AccountEffect, AccountFailure> {
        let complete = result.unwrap_or_else(|failure| LogoutCompletion {
            remote_warning: None,
            local: Err(failure),
        });
        let source = match complete.local {
            Ok(()) => {
                self.current = None;
                self.renewal.unsaved = None;
                AccountSource::Disconnected
            }
            Err(failure) => {
                tracing::error!(operation_id, error = %failure, "退出登录时本地凭据清理未完成");
                AccountSource::Unavailable(failure)
            }
        };
        self.snapshot = Some(source.clone());
        self.signing_out = false;
        self.credentials.finish_source_change()?;
        Ok(AccountEffect::Event(AccountEvent {
            operation_id,
            kind: AccountEventKind::LogoutFinished {
                source,
                remote_warning: complete.remote_warning,
            },
        }))
    }
}
