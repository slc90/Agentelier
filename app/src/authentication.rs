use agentelier_bridge::{
    AccountEventKind, AuthorizationLink, Connection, CredentialState as BridgeCredentials,
};
use agentelier_login_ui::{
    AccountPhase, AccountState, AccountSummary, CredentialState, DataDirectoryReady, LoginAction,
    ModelState, RestoreAccount, SessionActivity, VerifyModel,
};
use bevy::prelude::*;
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
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
    authorization: Option<(u64, AuthorizationLink)>,
    browser: Option<BrowserPending>,
}
struct BrowserPending {
    operation_id: u64,
    job_id: JobId,
    receiver: JobReceiver<io::Result<()>>,
    reply: Option<Arc<io::Result<()>>>,
}
#[derive(Clone, Copy)]
enum Request {
    Restore,
    Login { consent: bool },
}

impl AuthenticationBackend {
    pub(crate) fn finish(&self) -> Result {
        let mut backend = self
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        backend.stopping = true;
        backend.authorization = None;
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
        backend.authorization = None;
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
        .add_observer(login_action)
        .add_observer(verify_model)
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
    commands.queue(|world: &mut World| request(world, Request::Restore));
    Ok(())
}
fn retry(_event: On<RestoreAccount>, mut commands: Commands) {
    commands.queue(|world: &mut World| request(world, Request::Restore));
}
fn login_action(event: On<LoginAction>, mut commands: Commands) {
    let action = *event;
    commands.queue(move |world: &mut World| action_request(world, action));
}
fn request(world: &mut World, request: Request) -> Result {
    if world.resource::<ModelState>().operation.is_some() {
        return Ok(());
    }
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
    let result = (|| {
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
        let connection = backend
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("认证运行未建立"))?;
        match request {
            Request::Restore => connection.restore(),
            Request::Login { consent } => connection.login(consent),
        }
        .map_err(crate::diagnostics::operation)
    })();
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(id) => {
            backend.authorization = None;
            state.operation = Some(id);
            state.error = None;
            state.feedback = None;
            state.phase = match request {
                Request::Restore => AccountPhase::Reading,
                Request::Login { .. } => AccountPhase::Preparing,
            };
            match request {
                Request::Restore => info!(operation_id = id, "开始读取当前数据目录的账号"),
                Request::Login { consent } => info!(operation_id = id, consent, "开始浏览器登录"),
            }
        }
        Err(failure) => {
            error!(error = %failure, "无法开始账号操作");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}
fn action_request(world: &mut World, action: LoginAction) -> Result {
    match action {
        LoginAction::Login => return request(world, Request::Login { consent: false }),
        LoginAction::Consent => return request(world, Request::Login { consent: true }),
        LoginAction::OpenBrowser => return launch_browser(world),
        LoginAction::CopyLink => return copy_link(world),
        LoginAction::CheckSession => return check_session(world),
        LoginAction::SignOut => return sign_out(world),
        _ => {}
    }
    let state = world.resource::<AccountState>();
    let Some(id) = state.operation else {
        return Ok(());
    };
    let valid = match action {
        LoginAction::RetrySave => state.phase == AccountPhase::SaveFailed,
        LoginAction::Cancel => !matches!(
            state.phase,
            AccountPhase::Idle | AccountPhase::Reading | AccountPhase::Cancelling
        ),
        _ => false,
    };
    if !valid {
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
    let connection = backend
        .connection
        .as_mut()
        .ok_or_else(|| io::Error::other("认证运行未建立"))?;
    let result = if action == LoginAction::RetrySave {
        connection.retry_save(id)
    } else {
        connection.cancel(id)
    };
    let manager = world.resource::<JobManager>().clone();
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(()) => {
            state.phase = if action == LoginAction::RetrySave {
                AccountPhase::Saving
            } else {
                AccountPhase::Cancelling
            };
            state.error = None;
            state.feedback = None;
            if action == LoginAction::Cancel {
                backend.authorization = None;
                if let Some(browser) = &backend.browser {
                    manager
                        .cancel(browser.job_id)
                        .map_err(|failure| failure.into_bevy_error())?;
                }
                info!(operation_id = id, "用户请求取消登录，等待实际结束");
            } else {
                info!(operation_id = id, "用户重试保存已取得的登录结果");
            }
        }
        Err(failure) => {
            error!(operation_id = id, error = %failure, "登录控制操作失败");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}
fn receive(world: &mut World) -> Result {
    let backend = world.resource::<AuthenticationBackend>().clone();
    let (events, failure) = {
        let mut backend = backend
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        if backend.stopping {
            drop(backend);
            return receive_browser(world);
        }
        let mut events = Vec::new();
        let mut failure = None;
        if !world.resource::<AccountState>().runtime_failed
            && let Some(connection) = &mut backend.connection
        {
            loop {
                match connection.try_receive() {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => break,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
        }
        (events, failure)
    };
    let mut open = false;
    for event in events {
        match event.kind {
            AccountEventKind::ModelCompleted(result) => {
                receive_model(world, event.operation_id, result)?;
                continue;
            }
            AccountEventKind::ModelCancelled => {
                let mut model = world.resource_mut::<ModelState>();
                if model.operation == Some(event.operation_id) {
                    *model = ModelState::default();
                }
                debug!(operation_id = event.operation_id, "模型验证已实际取消");
                continue;
            }
            _ => {}
        }
        if event.operation_id != 0
            && world.resource::<AccountState>().operation != Some(event.operation_id)
        {
            return Err(io::Error::other("账号界面收到非当前操作的结果").into());
        }
        let mut backend = backend
            .0
            .lock()
            .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
        let mut state = world.resource_mut::<AccountState>();
        match event.kind {
            AccountEventKind::ModelCompleted(_) | AccountEventKind::ModelCancelled => {}
            AccountEventKind::SignOutProgress(stage) => {
                backend.authorization = None;
                open = false;
                state.phase = match stage {
                    agentelier_bridge::SignOutStage::Settling => AccountPhase::SigningOut,
                    agentelier_bridge::SignOutStage::Revoking => AccountPhase::Revoking,
                    agentelier_bridge::SignOutStage::Clearing => AccountPhase::Clearing,
                };
            }
            AccountEventKind::SignOutFailed(failure) => {
                state.phase = AccountPhase::SignOutFailed;
                state.error = Some(failure.to_string());
                error!(operation_id = event.operation_id, error = %failure, "退出未完成，等待重试本地清理");
            }
            AccountEventKind::SignedOut { snapshot, remote } => {
                backend.authorization = None;
                open = false;
                apply_snapshot(&mut state, snapshot);
                state.phase = AccountPhase::Idle;
                state.operation = None;
                state.error = None;
                state.session_error = None;
                state.session_activity = SessionActivity::Idle;
                state.session_client_error = false;
                state.session_retry_at = None;
                state.feedback = Some(match remote {
                    agentelier_bridge::RemoteRevocation::Unconfirmed(reason) => {
                        format!("已退出本机登录；远端撤销未确认：{reason}")
                    }
                    agentelier_bridge::RemoteRevocation::Confirmed
                    | agentelier_bridge::RemoteRevocation::NotApplicable => "已退出登录".into(),
                });
                info!(
                    operation_id = event.operation_id,
                    "退出登录完成，本地会话已清除并保留账号登记"
                );
            }
            AccountEventKind::SessionStatus {
                snapshot,
                activity,
                failure,
            } => {
                let was_checking = state.session_activity != SessionActivity::Idle;
                apply_snapshot(&mut state, snapshot);
                state.session_activity = match activity {
                    agentelier_bridge::SessionActivity::Idle => SessionActivity::Idle,
                    agentelier_bridge::SessionActivity::Checking => SessionActivity::Checking,
                    agentelier_bridge::SessionActivity::Refreshing => SessionActivity::Refreshing,
                    agentelier_bridge::SessionActivity::Saving => SessionActivity::Saving,
                    agentelier_bridge::SessionActivity::SaveFailed => SessionActivity::SaveFailed,
                };
                if matches!(
                    activity,
                    agentelier_bridge::SessionActivity::Idle
                        | agentelier_bridge::SessionActivity::SaveFailed
                ) {
                    state.session_client_error = failure.as_ref().is_some_and(|failure| {
                        failure.kind == agentelier_bridge::SessionFailureKind::ClientConfiguration
                    });
                    state.session_retry_at = failure.as_ref().and_then(|failure| failure.retry_at);
                    state.session_error = failure.map(|failure| failure.message);
                }
                if was_checking
                    && activity == agentelier_bridge::SessionActivity::Idle
                    && state.session_error.is_none()
                {
                    state.feedback = Some("会话凭据可用".into());
                    info!("当前账号会话检查完成，凭据可用");
                }
            }
            AccountEventKind::BrowserReady(link) => {
                if state.phase != AccountPhase::Cancelling {
                    backend.authorization = Some((event.operation_id, link));
                    state.phase = AccountPhase::Waiting;
                    open = true;
                    info!(
                        operation_id = event.operation_id,
                        "授权监听已就绪，等待用户完成浏览器登录"
                    );
                }
            }
            AccountEventKind::Exchanging => {
                backend.authorization = None;
                open = false;
                if state.phase != AccountPhase::Cancelling {
                    state.phase = AccountPhase::Exchanging;
                }
            }
            AccountEventKind::Saving => {
                backend.authorization = None;
                open = false;
                if state.phase != AccountPhase::Cancelling {
                    state.phase = AccountPhase::Saving;
                }
            }
            AccountEventKind::SaveFailed(failure) => {
                state.error = Some(failure.to_string());
                if state.phase != AccountPhase::Cancelling {
                    state.phase = AccountPhase::SaveFailed;
                }
                error!(operation_id = event.operation_id, error = %failure, "登录凭据尚未保存，保留待保存结果");
            }
            AccountEventKind::Completed(result) => {
                let was_reading = state.phase == AccountPhase::Reading;
                backend.authorization = None;
                open = false;
                state.operation = None;
                state.phase = AccountPhase::Idle;
                state.feedback = None;
                match result {
                    Ok(snapshot) => {
                        apply_snapshot(&mut state, snapshot);
                        if !was_reading {
                            state.session_activity = SessionActivity::Idle;
                            state.session_client_error = false;
                            state.session_retry_at = None;
                            state.session_error = None;
                        }
                        state.error = None;
                        info!(
                            operation_id = event.operation_id,
                            "账号操作完成，已取得持久化状态"
                        );
                    }
                    Err(failure) => {
                        error!(operation_id = event.operation_id, error = %failure, "账号操作失败，原会话保持不变");
                        state.error = Some(failure.to_string());
                    }
                }
            }
            AccountEventKind::Cancelled => {
                backend.authorization = None;
                open = false;
                state.operation = None;
                state.phase = AccountPhase::Idle;
                state.error = None;
                state.feedback = Some("登录已取消".into());
                info!(
                    operation_id = event.operation_id,
                    "登录已实际取消，原会话保持不变"
                );
            }
        }
    }
    if let Some(failure) = failure {
        let mut model = world.resource_mut::<ModelState>();
        model.operation = None;
        model.error = Some(failure.to_string());
        let mut state = world.resource_mut::<AccountState>();
        state.operation = None;
        state.phase = AccountPhase::Idle;
        state.error = Some(failure.to_string());
        state.runtime_failed = true;
        error!(error = %failure, "认证后台运行失败");
        open = false;
    }
    if open {
        launch_browser(world)?;
    }
    receive_browser(world)
}
fn apply_snapshot(state: &mut AccountState, snapshot: agentelier_bridge::AccountSnapshot) {
    state.account = snapshot.account.map(|account| AccountSummary {
        subject: account.subject,
        display_name: account.display_name,
        email: account.email,
    });
    state.credentials = match snapshot.credentials {
        BridgeCredentials::NoRecord => CredentialState::NoRecord,
        BridgeCredentials::HostOnly => CredentialState::HostOnly,
        BridgeCredentials::SignedOut => CredentialState::SignedOut,
        BridgeCredentials::ReauthenticationRequired => CredentialState::ReauthenticationRequired,
        BridgeCredentials::StoredSession {
            expired,
            subscription_enabled,
        } => CredentialState::StoredSession {
            expired,
            subscription_enabled,
        },
    };
}
fn sign_out(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    if state.runtime_failed
        || matches!(
            state.phase,
            AccountPhase::SigningOut | AccountPhase::Revoking | AccountPhase::Clearing
        )
    {
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
    let connection = backend
        .connection
        .as_mut()
        .ok_or_else(|| io::Error::other("认证运行未建立"))?;
    match connection.sign_out() {
        Ok(id) => {
            backend.authorization = None;
            *world.resource_mut::<ModelState>() = ModelState::default();
            let mut state = world.resource_mut::<AccountState>();
            state.operation = Some(id);
            state.phase = AccountPhase::SigningOut;
            state.error = None;
            state.feedback = None;
            state.session_error = None;
            info!(operation_id = id, "用户请求退出登录，停止接纳新凭据请求");
            if let Some(browser) = &backend.browser {
                world
                    .resource::<JobManager>()
                    .cancel(browser.job_id)
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
        Err(failure) => {
            error!(error = %failure, "无法请求退出登录");
            world.resource_mut::<AccountState>().error = Some(failure.to_string());
        }
    }
    Ok(())
}
fn verify_model(_event: On<VerifyModel>, mut commands: Commands) {
    commands.queue(|world: &mut World| -> Result {
        if !world
            .resource::<ModelState>()
            .can_start(world.resource::<AccountState>())
        {
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
        let connection = backend
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("认证运行未建立"))?;
        let mut state = world.resource_mut::<ModelState>();
        *state = ModelState::default();
        match connection.verify_model() {
            Ok(id) => {
                state.operation = Some(id);
                info!(operation_id = id, "开始验证模型连接");
            }
            Err(failure) => {
                state.error = Some(failure.to_string());
                error!(error = %failure, "无法开始模型连接验证");
            }
        }
        Ok(())
    });
}
fn receive_model(
    world: &mut World,
    id: u64,
    result: std::result::Result<agentelier_bridge::ModelReply, agentelier_bridge::ModelFailure>,
) -> Result {
    let mut state = world.resource_mut::<ModelState>();
    if state.operation != Some(id) {
        return Err(io::Error::other("模型结果不属于当前验证操作").into());
    }
    state.operation = None;
    match result {
        Ok(reply) => {
            info!(operation_id = id, model = reply.model, "模型连接验证完成");
            state.model = Some(reply.model);
            state.text = Some(if reply.text.is_empty() {
                "请求已完成，但没有文本内容".into()
            } else {
                reply.text
            });
        }
        Err(failure) => {
            error!(operation_id = id, kind = ?failure.kind, status = failure.status, code = failure.code, request_id = failure.request_id, "模型连接验证失败");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}
fn check_session(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    if state.phase != AccountPhase::Idle
        || state.runtime_failed
        || matches!(
            state.session_activity,
            SessionActivity::Checking | SessionActivity::Refreshing | SessionActivity::Saving
        )
    {
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
    let connection = backend
        .connection
        .as_mut()
        .ok_or_else(|| io::Error::other("认证运行未建立"))?;
    let result = connection.check_session();
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(()) => {
            state.session_activity = SessionActivity::Checking;
            state.error = None;
            state.feedback = None;
            info!("用户检查当前账号会话");
        }
        Err(failure) => {
            error!(error = %failure, "无法请求账号会话检查");
            state.error = Some(failure.to_string());
        }
    }
    Ok(())
}
fn launch_browser(world: &mut World) -> Result {
    let state = world.resource::<AccountState>();
    if state.phase != AccountPhase::Waiting {
        return Ok(());
    }
    let id = state.operation;
    let backend = world.resource::<AuthenticationBackend>().clone();
    let mut backend = backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
    if backend.stopping || backend.browser.is_some() {
        return Ok(());
    }
    let Some((operation_id, url)) = backend
        .authorization
        .as_ref()
        .filter(|(operation_id, _)| Some(*operation_id) == id)
    else {
        return Ok(());
    };
    let operation_id = *operation_id;
    let url = url.as_str().to_owned();
    let submission = world.resource::<JobManager>().spawn_thread(
        JobOptions {
            label: Some("打开授权浏览器".into()),
            ..default()
        },
        move |context| {
            if context.is_cancelled() {
                context.acknowledge_cancel();
                return Ok(());
            }
            let result = open_browser(&url);
            if context.send_blocking(result)? == SendStatus::Canceled {
                context.acknowledge_cancel();
            }
            Ok(())
        },
    );
    match submission {
        Ok((job_id, receiver)) => {
            backend.browser = Some(BrowserPending {
                operation_id,
                job_id,
                receiver,
                reply: None,
            });
            world.resource_mut::<AccountState>().browser_opening = true;
        }
        Err(failure) => {
            world.resource_mut::<AccountState>().error = Some(format!(
                "无法打开浏览器：{failure}；可以重新打开或复制登录链接"
            ));
            error!(operation_id, error = %failure, "无法提交打开浏览器的操作");
        }
    }
    Ok(())
}
fn open_browser(url: &str) -> io::Result<()> {
    use winsafe::co;
    let _com = winsafe::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
        .map_err(io::Error::other)?;
    winsafe::ShellExecuteEx(&winsafe::SHELLEXECUTEINFO {
        mask: co::SEE_MASK::NOASYNC | co::SEE_MASK::FLAG_NO_UI,
        verb: Some("open"),
        file: url,
        show: co::SW::SHOWNORMAL,
        ..default()
    })
    .map_err(|error| io::Error::other(format!("无法打开系统默认浏览器：{error}")))
}
fn copy_link(world: &mut World) -> Result {
    if world.resource::<AccountState>().phase != AccountPhase::Waiting {
        return Ok(());
    }
    let backend = world.resource::<AuthenticationBackend>().clone();
    let backend = backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
    if backend.stopping {
        return Ok(());
    }
    let Some((id, link)) = &backend.authorization else {
        return Ok(());
    };
    if world.resource::<AccountState>().operation != Some(*id) {
        return Ok(());
    }
    let result = world
        .resource_mut::<bevy::clipboard::Clipboard>()
        .set_text(link.as_str());
    let mut state = world.resource_mut::<AccountState>();
    match result {
        Ok(()) => {
            state.error = None;
            state.feedback = Some("登录链接已复制".into());
            info!(operation_id = *id, "用户复制当前登录链接");
        }
        Err(_) => {
            state.error = Some("无法写入系统剪贴板，请重试或重新打开浏览器".into());
            error!(operation_id = *id, "无法复制登录链接到系统剪贴板");
        }
    }
    Ok(())
}
fn receive_browser(world: &mut World) -> Result {
    let backend = world.resource::<AuthenticationBackend>().clone();
    let mut backend = backend
        .0
        .lock()
        .map_err(|_| io::Error::other("认证后台状态锁已损坏"))?;
    let Some(mut pending) = backend.browser.take() else {
        return Ok(());
    };
    let outcome = loop {
        match pending
            .receiver
            .try_recv()
            .map_err(|failure| failure.into_bevy_error())?
        {
            JobRead::Item(reply) => pending.reply = Some(reply),
            JobRead::Empty => {
                backend.browser = Some(pending);
                return Ok(());
            }
            JobRead::Finished(outcome) => break outcome,
            JobRead::Closed => return Err(io::Error::other("打开浏览器的任务缺少终态").into()),
        }
    };
    world
        .resource::<JobManager>()
        .remove(pending.job_id)
        .map_err(|failure| failure.into_bevy_error())?;
    let result = match outcome {
        JobOutcome::Completed => pending
            .reply
            .as_ref()
            .ok_or_else(|| io::Error::other("打开浏览器的任务缺少结果"))?
            .as_ref()
            .as_ref()
            .map_err(|error| error.to_string()),
        JobOutcome::Failed(failure) => Err(failure.to_string()),
        JobOutcome::Canceled => {
            debug!(
                operation_id = pending.operation_id,
                "打开浏览器的辅助任务已取消"
            );
            Ok(&())
        }
    };
    if let Err(failure) = &result {
        error!(operation_id = pending.operation_id, error = %failure, "打开系统默认浏览器失败");
    }
    let open_current = {
        let mut state = world.resource_mut::<AccountState>();
        state.browser_opening = false;
        if state.operation == Some(pending.operation_id) {
            if let Err(failure) = result
                && state.phase == AccountPhase::Waiting
            {
                state.error = Some(format!(
                    "无法打开浏览器：{failure}；可以重新打开或复制登录链接"
                ));
            }
            false
        } else {
            state.phase == AccountPhase::Waiting && state.operation.is_some()
        }
    };
    drop(backend);
    if open_current {
        launch_browser(world)?;
    }
    Ok(())
}
