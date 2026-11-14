use agentelier_bridge::{Connection, CredentialState as BridgeCredentials};
use agentelier_login_ui::{
    AccountState, AccountSummary, CredentialState, DataDirectoryReady, RestoreAccount,
};
use bevy::prelude::*;
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

#[derive(Resource, Clone, Default)]
pub(crate) struct AuthenticationBackend(Arc<Mutex<BackendState>>);

#[derive(Default)]
struct BackendState {
    connection: Option<Connection>,
    credentials_path: Option<PathBuf>,
    stopping: bool,
}

impl AuthenticationBackend {
    pub(crate) fn finish(&self) -> Result {
        let mut backend = self
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        backend.stopping = true;
        if let Some(connection) = &mut backend.connection {
            connection.finish().map_err(crate::diagnostics::operation)?;
        }
        Ok(())
    }

    pub(crate) fn begin_shutdown(&self) -> Result {
        let mut backend = self
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        backend.stopping = true;
        if let Some(connection) = &mut backend.connection {
            connection
                .begin_shutdown()
                .map_err(crate::diagnostics::operation)?;
            debug!("已停止认证业务接收与普通唤醒，等待执行体结束");
        }
        Ok(())
    }

    pub(crate) fn is_finished(&self) -> Result<bool> {
        let backend = self
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        Ok(backend
            .connection
            .as_ref()
            .is_none_or(Connection::is_finished))
    }
}

pub(crate) fn install(app: &mut App, backend: AuthenticationBackend) {
    app.insert_resource(backend)
        .add_observer(ready)
        .add_observer(retry)
        .add_systems(Update, receive);
}

fn ready(
    event: On<DataDirectoryReady>,
    backend: Res<AuthenticationBackend>,
    mut commands: Commands,
) -> Result {
    backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?
        .credentials_path = Some(event.environment.credentials_path().to_path_buf());
    commands.queue(request);
    Ok(())
}

fn retry(_event: On<RestoreAccount>, mut commands: Commands) {
    commands.queue(request);
}

fn request(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    if state.operation.is_some() || state.runtime_failed {
        return Ok(());
    }
    let backend = world.resource::<AuthenticationBackend>().clone();
    let mut backend = backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
    if backend.stopping {
        return Ok(());
    }
    let request = (|| {
        if backend.connection.is_none() {
            let path = backend
                .credentials_path
                .clone()
                .ok_or_else(|| io::Error::other("认证运行缺少当前凭据路径"))?;
            let proxy = world
                .get_resource::<EventLoopProxyWrapper>()
                .map(|proxy| (**proxy).clone())
                .ok_or_else(|| io::Error::other("认证运行缺少 Winit 唤醒通道"))?;
            backend.connection = Some(Connection::start(path, move || {
                proxy.send_event(WinitUserEvent::WakeUp).is_ok()
            })?);
        }
        backend
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("认证运行未建立"))?
            .restore()
            .map_err(crate::diagnostics::operation)
    })();
    let mut state = world.resource_mut::<AccountState>();
    match request {
        Ok(id) => {
            state.operation = Some(id);
            state.error = None;
            info!(operation_id = id, "开始读取当前数据目录的账号");
        }
        Err(failure) => {
            error!(error = %failure, "无法开始账号恢复");
            state.error = Some(failure.to_string());
            state.runtime_failed = true;
        }
    }
    Ok(())
}

fn receive(backend: Res<AuthenticationBackend>, mut state: ResMut<AccountState>) -> Result {
    if state.runtime_failed {
        return Ok(());
    }
    let mut backend = backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
    if backend.stopping {
        return Ok(());
    }
    let Some(connection) = &mut backend.connection else {
        return Ok(());
    };
    match connection.try_receive() {
        Ok(Some(event)) => {
            if state.operation != Some(event.operation_id) {
                return Err(io::Error::other("账号界面收到非当前操作的结果").into());
            }
            state.operation = None;
            match event.result {
                Ok((account, credentials)) => {
                    state.account = account.map(|account| AccountSummary {
                        subject: account.subject,
                        display_name: account.display_name,
                        email: account.email,
                    });
                    state.credentials = match credentials {
                        BridgeCredentials::NoRecord => CredentialState::NoRecord,
                        BridgeCredentials::HostOnly => CredentialState::HostOnly,
                        BridgeCredentials::SignedOut => CredentialState::SignedOut,
                        BridgeCredentials::StoredSession { expired } => {
                            CredentialState::StoredSession { expired }
                        }
                    };
                    state.error = None;
                    info!(operation_id = event.operation_id, "当前目录账号读取完成");
                }
                Err(failure) => {
                    error!(operation_id = event.operation_id, error = %failure, "当前目录账号读取失败");
                    state.error = Some(failure.to_string());
                }
            }
        }
        Ok(None) => {}
        Err(failure) => {
            error!(error = %failure, "认证后台运行失败");
            state.operation = None;
            state.error = Some(failure.to_string());
            state.runtime_failed = true;
        }
    }
    Ok(())
}
