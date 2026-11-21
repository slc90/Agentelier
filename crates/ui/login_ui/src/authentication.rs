use crate::account::CodexStatus;
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
            state.operation = Some(id);
            state.error = None;
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
    let Some(id) = state.operation else {
        return Ok(());
    };
    let host = world.resource::<AgentHost>();
    if !state.source_settling
        && !state.runtime_failed
        && !host.is_stopping()?
        && let Err(failure) = host.cancel_login(id)
    {
        error!(operation_id = id, error = %failure, "无法取消浏览器登录");
        world.resource_mut::<AccountState>().error = Some(failure.to_string());
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
        if event.operation_id != 0 && state.operation != Some(event.operation_id) {
            continue;
        }
        match &event.kind {
            AccountEventKind::LoginWaiting { browser_error, .. } => {
                state.waiting = true;
                if let Some(failure) = browser_error {
                    error!(operation_id = event.operation_id, error = %failure, "无法打开系统浏览器");
                    state.error = Some(format!("无法打开系统浏览器：{failure}"));
                }
            }
            AccountEventKind::SourceSettling => state.source_settling = true,
            AccountEventKind::SourceChanged(result) => {
                if event.operation_id != 0 {
                    state.operation = None;
                    state.waiting = false;
                    state.error = None;
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
        state.operation = None;
        state.error = Some(failure.reason.to_string());
        state.runtime_failed = true;
        state.waiting = false;
        state.source_settling = false;
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
