use super::Command;
use agentelier_account::{
    Account, AccountCommand, CodexOAuthCredentials, LoginFailure, LoginPrompt,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct LoginTask {
    id: u64,
    cancellation: CancellationToken,
    prompt: Option<oneshot::Receiver<LoginPrompt>>,
    task: JoinHandle<Result<CodexOAuthCredentials, LoginFailure>>,
}

impl LoginTask {
    pub(super) fn start(id: u64, shutdown: &CancellationToken) -> Self {
        let (ready, prompt) = oneshot::channel();
        let cancellation = shutdown.child_token();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            Account::browser_login(&task_cancellation, |prompt| drop(ready.send(prompt))).await
        });
        Self {
            id,
            cancellation,
            prompt: Some(prompt),
            task,
        }
    }

    pub(super) fn cancel(&self, id: u64) {
        if self.id == id {
            self.cancellation.cancel();
        }
    }

    pub(super) async fn finish(self) {
        self.cancellation.cancel();
        if let Err(failure) = self.task.await {
            tracing::error!(operation_id = self.id, error = %failure, "浏览器登录后台任务异常结束");
        }
    }
}

pub(super) async fn next(pending: &mut Option<LoginTask>) -> Command {
    let Some(login) = pending.as_mut() else {
        return std::future::pending().await;
    };
    let id = login.id;
    if let Some(prompt) = &mut login.prompt {
        let received = prompt.await;
        login.prompt = None;
        if let Ok(prompt) = received {
            return Command::LoginWaiting(id, prompt);
        }
    }
    let completed = (&mut login.task).await;
    let outcome = if login.cancellation.is_cancelled() {
        Err(LoginFailure::Cancelled)
    } else {
        completed.unwrap_or_else(|_| Err(LoginFailure::Failed("浏览器登录后台任务异常结束".into())))
    };
    *pending = None;
    Command::Account(AccountCommand::CompleteLogin(id, outcome))
}
