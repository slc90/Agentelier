use super::{Account, AccountEffect, AccountFailure};
use crate::source::AccountSource;
use crate::{CredentialIntent, SessionFailure, UsableCredentials};
use agentelier_login::{CodexOAuthCredentials, RefreshFailure};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

type Outcome = Result<UsableCredentials, SessionFailure>;

pub struct RefreshAttempt(CodexOAuthCredentials);

pub struct RefreshCompletion {
    previous: CodexOAuthCredentials,
    result: Result<CodexOAuthCredentials, RefreshFailure>,
}

impl RefreshAttempt {
    pub async fn run(self) -> RefreshCompletion {
        let result = self.0.refresh().await;
        RefreshCompletion {
            previous: self.0,
            result,
        }
    }
}

pub(super) struct Waiter {
    pub(super) current: CancellationToken,
    pub(super) intent: CredentialIntent,
    pub(super) result: watch::Sender<Option<Outcome>>,
}

impl Waiter {
    fn answer(self, outcome: Outcome) {
        self.result.send_replace(Some(outcome));
    }
}

pub(super) struct Rotation {
    previous: CodexOAuthCredentials,
    replacement: CodexOAuthCredentials,
}

#[derive(Default)]
pub(super) struct Renewal {
    waiters: Vec<Waiter>,
    pub(super) unsaved: Option<Rotation>,
}

impl Account {
    pub(super) async fn request_credentials(
        &mut self,
        waiter: Waiter,
    ) -> Result<AccountEffect, AccountFailure> {
        if waiter.current.is_cancelled() {
            waiter.answer(Err(SessionFailure::new("凭据来源已变化，原请求已停止")));
            return Ok(AccountEffect::None);
        }
        let Some(current) = self.current.as_ref() else {
            waiter.answer(Err(SessionFailure::new("请先使用 ChatGPT 登录")));
            return Ok(AccountEffect::None);
        };
        if let CredentialIntent::Recover(rejected) = &waiter.intent
            && rejected.identity() != &current.summary().identity
        {
            waiter.answer(Err(SessionFailure::new(
                "Codex 账号或工作区已变化，原请求已停止",
            )));
            return Ok(AccountEffect::None);
        }
        if !self.renewal.waiters.is_empty() {
            self.renewal.waiters.push(waiter);
            return Ok(AccountEffect::None);
        }
        if let Some(rotation) = self.renewal.unsaved.take() {
            return self.commit_rotation(rotation, vec![waiter]).await;
        }
        match current.is_expired() {
            Ok(false) => {
                let outcome = UsableCredentials::from_oauth(current);
                if !matches!(
                    (&waiter.intent, &outcome),
                    (CredentialIntent::Recover(rejected), Ok(value)) if rejected == value.version()
                ) {
                    waiter.answer(outcome);
                    return Ok(AccountEffect::None);
                }
            }
            Ok(true) => {}
            Err(failure) => {
                waiter.answer(Err(SessionFailure::new(failure.to_string())));
                return Ok(AccountEffect::None);
            }
        }
        self.renewal.waiters.push(waiter);
        Ok(AccountEffect::Refresh(RefreshAttempt(current.clone())))
    }

    pub(super) async fn complete_refresh(
        &mut self,
        result: Result<RefreshCompletion, String>,
    ) -> Result<AccountEffect, AccountFailure> {
        let waiters = std::mem::take(&mut self.renewal.waiters);
        let RefreshCompletion { previous, result } = match result {
            Ok(completion) => completion,
            Err(failure) => {
                tracing::error!(error = %failure, "凭据续期后台任务失败");
                return Ok(fail_waiters(waiters, &failure));
            }
        };
        if self.current.as_ref() != Some(&previous) {
            return Ok(fail_waiters(waiters, "凭据来源已变化，续期结果已失效"));
        }
        match result {
            Ok(replacement) => {
                self.commit_rotation(
                    Rotation {
                        previous,
                        replacement,
                    },
                    waiters,
                )
                .await
            }
            Err(failure) => {
                let reason = failure.to_string();
                let effect = fail_waiters(waiters, &reason);
                if failure.needs_login() {
                    self.observe(0, AccountSource::Unavailable(reason), false)
                } else {
                    Ok(effect)
                }
            }
        }
    }

    async fn commit_rotation(
        &mut self,
        rotation: Rotation,
        waiters: Vec<Waiter>,
    ) -> Result<AccountEffect, AccountFailure> {
        let store = self.store.clone();
        let (rotation, saved) = tokio::task::spawn_blocking(move || {
            let saved = store.replace_if_current(&rotation.previous, &rotation.replacement);
            (rotation, saved)
        })
        .await
        .map_err(|_| AccountFailure::Task)?;
        match saved {
            Ok(true) => {
                let updated = rotation.replacement;
                let outcome = UsableCredentials::from_oauth(&updated);
                let snapshot = AccountSource::Codex(updated.summary().clone());
                self.current = Some(updated);
                tracing::debug!("Codex OAuth 凭据已续期并保存");
                for waiter in waiters {
                    let outcome = outcome.clone().and_then(|current| {
                        if let CredentialIntent::Recover(rejected) = &waiter.intent
                            && rejected == current.version()
                        {
                            return Err(SessionFailure::new("续期后服务未返回新的 Access Token"));
                        }
                        Ok(current)
                    });
                    waiter.answer(outcome);
                }
                self.observe(0, snapshot, false)
            }
            Ok(false) => {
                let reason = "本地 OAuth 凭据已变化，续期结果未保存，请重新登录";
                fail_waiters(waiters, reason);
                self.observe(0, AccountSource::Unavailable(reason.into()), false)
            }
            Err(failure) => {
                // A successful remote rotation may already have consumed the previous refresh token.
                tracing::error!(error = %failure, "续期后的 Codex OAuth 凭据无法保存");
                self.renewal.unsaved = Some(rotation);
                Ok(fail_waiters(waiters, &failure.to_string()))
            }
        }
    }
}

fn fail_waiters(waiters: Vec<Waiter>, reason: &str) -> AccountEffect {
    let failure = SessionFailure::new(reason);
    for waiter in waiters {
        waiter.answer(Err(failure.clone()));
    }
    AccountEffect::None
}
