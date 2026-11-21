use crate::account::{AccountOperation, AccountPhase, CodexStatus};
use crate::{AccountState, DataDirectoryReady};
use agentelier_account_bridge::{AccountEventKind, SourceSnapshot};
use agentelier_agent_host::{AccountEvent, AgentHost, HostFailed, ReceiveAgentEvents};
use bevy::prelude::*;
use bevy::winit::EventLoopProxyWrapper;
use tracing::{error, info};

pub(crate) fn install(app: &mut App) {
    app.add_observer(ready)
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

fn ready(
    event: On<DataDirectoryReady>,
    host: Res<AgentHost>,
    proxy: Res<EventLoopProxyWrapper>,
    mut state: ResMut<AccountState>,
) {
    if let Err(failure) = host.prepare(event.environment.clone(), &proxy) {
        error!(error = %failure, "无法启动执行宿主");
        state.error = Some(failure.to_string());
        state.runtime_failed = true;
    }
}

pub(crate) fn begin_login(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    let host = world.resource::<AgentHost>();
    if state.is_busy() || state.runtime_failed || host.is_stopping()? {
        return Ok(());
    }
    let result = host.begin_login();
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(id) => {
            state.clear_operation();
            state.operation = Some(AccountOperation {
                id,
                phase: AccountPhase::OpeningBrowser,
            });
            info!(operation_id = id, "用户请求使用 ChatGPT 登录");
        }
        Err(failure) => {
            error!(error = %failure, "无法开始浏览器登录");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

pub(crate) fn cancel_login(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    let Some(operation) = state.operation.as_ref() else {
        return Ok(());
    };
    if matches!(
        operation.phase,
        AccountPhase::Cancelling | AccountPhase::SigningOut
    ) || state.source_settling
        || state.runtime_failed
    {
        return Ok(());
    }
    let id = operation.id;
    let host = world.resource::<AgentHost>();
    if host.is_stopping()? {
        return Ok(());
    }
    let result = host.cancel_login(id);
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(()) => {
            state.operation = Some(AccountOperation {
                id,
                phase: AccountPhase::Cancelling,
            });
            state.authorization_url = None;
            state.error = None;
        }
        Err(failure) => {
            error!(operation_id = id, error = %failure, "无法取消浏览器登录");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

pub(crate) fn copy_login_url(world: &mut World) {
    let state = world.resource::<AccountState>();
    if state.source_settling || state.runtime_failed {
        return;
    }
    let Some(url) = state.authorization_url.clone() else {
        return;
    };
    let result = match world.get_resource_mut::<bevy::clipboard::Clipboard>() {
        Some(mut clipboard) => clipboard
            .set_text(&url)
            .map_err(|failure| failure.to_string()),
        None => Err("剪贴板服务不可用".into()),
    };
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(()) => {
            if let Some(operation) = &mut state.operation {
                operation.phase = AccountPhase::LinkCopied;
            }
            state.error = None;
        }
        Err(failure) => {
            error!(error = %failure, "无法复制授权链接");
            state.error = Some(format!("无法复制授权链接：{failure}"));
        }
    }
}

pub(crate) fn logout(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    let host = world.resource::<AgentHost>();
    if state.is_busy() || state.runtime_failed || host.is_stopping()? {
        return Ok(());
    }
    let result = host.logout();
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(id) => {
            state.clear_operation();
            state.operation = Some(AccountOperation {
                id,
                phase: AccountPhase::SigningOut,
            });
            info!(operation_id = id, "用户请求退出 ChatGPT 登录");
        }
        Err(failure) => {
            error!(error = %failure, "无法开始退出登录");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}

fn receive(
    mut events: MessageReader<AccountEvent>,
    mut failures: MessageReader<HostFailed>,
    mut state: ResMut<AccountState>,
    host: Res<AgentHost>,
) -> Result {
    if host.is_stopping()? {
        return Ok(());
    }
    for event in events.read() {
        if event.operation_id != 0
            && state.operation.as_ref().map(|operation| operation.id) != Some(event.operation_id)
        {
            continue;
        }
        match &event.kind {
            AccountEventKind::LoginWaiting(manual) => {
                let Some(operation) = &mut state.operation else {
                    continue;
                };
                if operation.phase != AccountPhase::OpeningBrowser {
                    continue;
                }
                if let Some(manual) = manual {
                    operation.phase = AccountPhase::ManualBrowser;
                    state.authorization_url = Some(manual.authorization_url.clone());
                    state.error = Some(format!("无法打开系统浏览器：{}", manual.reason));
                    error!(operation_id = event.operation_id, error = %manual.reason, "无法打开系统浏览器");
                } else {
                    operation.phase = AccountPhase::WaitingForBrowser;
                }
            }
            AccountEventKind::LoginCancelled => {
                state.clear_operation();
                state.notice = Some("已取消登录".into());
                info!(operation_id = event.operation_id, "已取消浏览器登录");
            }
            AccountEventKind::SourceSettling => {
                state.source_settling = true;
                state.authorization_url = None;
            }
            AccountEventKind::LogoutFinished {
                source,
                remote_warning,
            } => {
                state.clear_operation();
                state.error = match (source, remote_warning) {
                    (SourceSnapshot::Unavailable(local), Some(remote)) => {
                        Some(format!("{local}\n{remote}"))
                    }
                    (_, remote) => remote.clone(),
                };
                apply_source(&mut state, source, event.operation_id);
            }
            AccountEventKind::SourceChanged(result) => {
                if event.operation_id != 0 {
                    state.clear_operation();
                }
                match result {
                    Ok(snapshot) => {
                        state.source_settling = false;
                        apply_source(&mut state, snapshot, event.operation_id);
                    }
                    Err(failure) => {
                        error!(operation_id = event.operation_id, error = %failure, "浏览器登录未完成");
                        state.error = Some(failure.to_string());
                    }
                }
            }
        }
    }
    if let Some(failure) = failures.read().last() {
        state.clear_operation();
        state.error = Some(failure.reason.to_string());
        state.runtime_failed = true;
        error!(error = %failure.reason, "认证后台运行失败");
    }
    Ok(())
}

fn apply_source(state: &mut AccountState, snapshot: &SourceSnapshot, operation_id: u64) {
    state.codex = match snapshot {
        SourceSnapshot::Disconnected => None,
        SourceSnapshot::Codex(summary) => Some(CodexStatus::Read {
            subject: summary.subject.clone(),
            workspace_id: summary.workspace_id.clone(),
            email: summary.email.clone(),
        }),
        SourceSnapshot::Unavailable(message) => {
            error!(operation_id, error = %message, "本地 Codex OAuth 凭据不可用");
            Some(CodexStatus::Failed(message.clone()))
        }
    };
    if operation_id != 0 {
        info!(operation_id, "当前账号状态已更新");
    }
}
