use crate::account::{CodexSource, CodexStatus};
use crate::{AccountState, DataDirectoryReady};
use agentelier_account_bridge::{AccountEventKind, SourceSnapshot};
use agentelier_agent_host::{
    AccountEvent, AgentHost, HostFailed, ReceiveAgentEvents, SourceChange,
};
use bevy::prelude::*;
use bevy::winit::EventLoopProxyWrapper;
use std::io;
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
    if let Err(failure) = host.prepare(event.environment.root().to_path_buf(), &proxy) {
        error!(error = %failure, "无法启动执行宿主");
        state.error = Some(failure.to_string());
        state.runtime_failed = true;
    }
}

pub(crate) fn change_source(world: &mut World, change: SourceChange) -> Result {
    let state = world.resource::<AccountState>();
    let host = world.resource::<AgentHost>();
    if state.is_busy() || state.runtime_failed || host.is_stopping()? {
        return Ok(());
    }
    let result = host.change_source(change);
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
            info!(operation_id = id, "用户请求更新当前凭据来源");
        }
        Err(failure) => {
            error!(error = %failure, "无法开始更新凭据来源");
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
        if event.operation_id != 0 && state.operation != Some(event.operation_id) {
            return Err(io::Error::other("账号界面收到非当前操作的结果").into());
        }
        match &event.kind {
            AccountEventKind::SourceSettling => state.source_settling = true,
            AccountEventKind::SourceChanged(result) => {
                if event.operation_id != 0 {
                    state.operation = None;
                }
                match result {
                    Ok(snapshot) => {
                        state.source_settling = false;
                        apply_source(&mut state, snapshot, event.operation_id);
                    }
                    Err(failure) => {
                        error!(operation_id = event.operation_id, error = %failure, "凭据来源未能保存");
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
        state.source_settling = false;
        error!(error = %failure.reason, "认证后台运行失败");
    }
    Ok(())
}

fn apply_source(state: &mut AccountState, snapshot: &SourceSnapshot, operation_id: u64) {
    state.codex = match snapshot {
        SourceSnapshot::Disconnected => None,
        SourceSnapshot::Codex { path, status } => Some(CodexSource {
            path: path.clone(),
            status: match status {
                Ok(summary) => CodexStatus::Read {
                    subject: summary.subject.clone(),
                    workspace_id: summary.workspace_id.clone(),
                    email: summary.email.clone(),
                    expired: summary.expired,
                },
                Err(message) => {
                    error!(operation_id, error = %message, "Codex 凭据文件不可用");
                    CodexStatus::Failed(message.clone())
                }
            },
        }),
    };
    if operation_id != 0 {
        info!(
            operation_id,
            codex_selected = state.codex.is_some(),
            "当前凭据来源已保存"
        );
    }
}
