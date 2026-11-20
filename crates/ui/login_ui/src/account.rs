use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::{WidgetryButton, WidgetryButtonLoading};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry_path_display::display_path;
use std::path::PathBuf;

const ACCOUNT_GAP: f32 = 12.0;
const BUTTON_HEIGHT: f32 = 40.0;
const BUTTON_WIDTH: f32 = 168.0;

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct AccountState {
    pub(crate) codex: Option<CodexSource>,
    pub account: Option<AccountSummary>,
    pub credentials: CredentialState,
    pub operation: Option<u64>,
    pub phase: AccountPhase,
    pub error: Option<String>,
    pub feedback: Option<String>,
    pub browser_opening: bool,
    pub runtime_failed: bool,
    pub session_activity: SessionActivity,
    pub session_client_error: bool,
    pub session_retry_at: Option<u64>,
    pub session_error: Option<String>,
}

impl AccountState {
    pub fn uses_codex(&self) -> bool {
        self.codex.is_some()
    }

    pub fn model_unavailable_reason(&self) -> Option<&'static str> {
        match self.codex.as_ref().map(|source| &source.status) {
            Some(CodexStatus::Read { expired: false, .. }) => None,
            Some(CodexStatus::Read { expired: true, .. }) => Some("请在 Codex 中恢复授权"),
            Some(CodexStatus::Failed(_)) => Some("请选择有效的 Codex 凭据文件"),
            None if matches!(
                self.credentials,
                CredentialState::StoredSession {
                    subscription_enabled: true,
                    ..
                }
            ) =>
            {
                None
            }
            None => Some("请先登录并开启订阅授权"),
        }
    }

    pub fn model_account_key(&self) -> Option<String> {
        match self.codex.as_ref().map(|source| &source.status) {
            Some(CodexStatus::Read {
                subject,
                workspace_id,
                expired: false,
                ..
            }) => Some(format!("codex/{subject}/{workspace_id}")),
            Some(_) => None,
            None if matches!(self.credentials, CredentialState::StoredSession { .. }) => self
                .account
                .as_ref()
                .map(|account| format!("siwc/{}", account.subject)),
            None => None,
        }
    }
}

#[derive(Reflect, Clone)]
pub(crate) struct CodexSource {
    pub(crate) path: PathBuf,
    pub(crate) status: CodexStatus,
}

#[derive(Reflect, Clone)]
pub(crate) enum CodexStatus {
    Read {
        subject: String,
        workspace_id: String,
        email: Option<String>,
        expired: bool,
    },
    Failed(String),
}
#[derive(Reflect, Clone)]
pub struct AccountSummary {
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}
#[derive(Reflect, Clone, Default)]
pub enum CredentialState {
    #[default]
    Unknown,
    NoRecord,
    HostOnly,
    SignedOut,
    ReauthenticationRequired,
    StoredSession {
        expired: bool,
        subscription_enabled: bool,
    },
}
#[derive(Reflect, Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountPhase {
    #[default]
    Idle,
    Reading,
    ChangingSource,
    Preparing,
    Waiting,
    Exchanging,
    Saving,
    SaveFailed,
    Cancelling,
    SigningOut,
    Revoking,
    Clearing,
    SignOutFailed,
}
#[derive(Reflect, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionActivity {
    #[default]
    Idle,
    Checking,
    Refreshing,
    Saving,
    SaveFailed,
}
#[derive(Event)]
pub struct RestoreAccount;
#[derive(Event, Clone, Copy, PartialEq, Eq)]
pub enum LoginAction {
    Login,
    Consent,
    OpenBrowser,
    CopyLink,
    Cancel,
    RetrySave,
    CheckSession,
    SignOut,
}
#[derive(Component, Clone, Copy)]
enum ButtonAction {
    Restore,
    Login(LoginAction),
    SelectCodex,
    DisconnectCodex,
}
#[derive(Component, Clone, Copy)]
struct ButtonLabel(ButtonAction);
#[derive(Component, Clone, Default)]
struct AccountIdentity;
#[derive(Component, Clone, Default)]
struct AccountStatus;
#[derive(Component, Clone, Default)]
struct AccountError;
#[derive(Component, Clone, Default)]
struct AccountFeedback;
#[derive(Component, Clone, Default)]
struct AccountSourceLabel;
#[derive(Component, Clone, Default)]
struct AccountSourcePath;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AccountState>()
        .register_type::<AccountState>()
        .add_systems(PostUpdate, update_panel.before(UiSystems::Content));
}
fn identity(state: &AccountState) -> String {
    if let Some(source) = &state.codex {
        return match &source.status {
            CodexStatus::Read {
                subject,
                workspace_id,
                email,
                ..
            } => {
                format!(
                    "{} · 工作区 {workspace_id}",
                    email.as_deref().unwrap_or(subject)
                )
            }
            CodexStatus::Failed(_) => String::new(),
        };
    }
    if matches!(
        state.credentials,
        CredentialState::SignedOut | CredentialState::NoRecord | CredentialState::HostOnly
    ) {
        return String::new();
    }
    state
        .account
        .as_ref()
        .map(|account| match (&account.display_name, &account.email) {
            (Some(name), Some(email)) => format!("{name} · {email}"),
            (Some(name), None) => name.clone(),
            (None, Some(email)) => email.clone(),
            (None, None) => account.subject.clone(),
        })
        .unwrap_or_default()
}
fn status(state: &AccountState) -> &'static str {
    match state.phase {
        AccountPhase::Reading => return "正在读取账号…",
        AccountPhase::ChangingSource => return "正在更新凭据来源…",
        AccountPhase::Preparing => return "正在准备登录…",
        AccountPhase::Waiting => return "请在浏览器中完成登录",
        AccountPhase::Exchanging => return "正在交换授权码并校验身份…",
        AccountPhase::Saving => return "正在保存登录凭据…",
        AccountPhase::SaveFailed => return "登录结果尚未保存，请重试保存",
        AccountPhase::Cancelling => return "正在取消登录…",
        AccountPhase::SigningOut => return "正在退出登录，等待当前操作结束…",
        AccountPhase::Revoking => return "正在退出登录，撤销远端会话…",
        AccountPhase::Clearing => return "正在退出登录，清除本地凭据…",
        AccountPhase::SignOutFailed => return "退出未完成，无法清除本地凭据",
        AccountPhase::Idle => {}
    }
    if let Some(source) = &state.codex {
        return match source.status {
            CodexStatus::Read { expired: false, .. } => "已读取外部凭据",
            CodexStatus::Read { expired: true, .. } => "外部凭据已过期，请在 Codex 中恢复授权",
            CodexStatus::Failed(_) => "外部凭据不可用",
        };
    }
    match state.session_activity {
        SessionActivity::Checking => return "正在检查可用会话…",
        SessionActivity::Refreshing => return "正在刷新账号会话…",
        SessionActivity::Saving => return "正在保存更新后的凭据…",
        SessionActivity::SaveFailed => return "凭据更新尚未保存，请重试保存",
        SessionActivity::Idle => {}
    }
    match state.credentials {
        CredentialState::Unknown if state.error.is_some() => "无法恢复账号",
        CredentialState::Unknown => "正在读取账号…",
        CredentialState::NoRecord | CredentialState::HostOnly | CredentialState::SignedOut => {
            "未登录"
        }
        CredentialState::ReauthenticationRequired => "会话已失效，请重新登录",
        CredentialState::StoredSession { expired: true, .. } => "会话已过期，可检查会话以尝试续期",
        CredentialState::StoredSession {
            subscription_enabled: false,
            ..
        } => "账号已登录，订阅授权未开启",
        CredentialState::StoredSession { .. } => "账号已登录，订阅授权已开启",
    }
}

fn source_label(state: &AccountState) -> &'static str {
    if state.uses_codex() {
        "当前来源：Codex OAuth（外部凭据）"
    } else {
        "当前来源：Sign in with ChatGPT（SIWC）"
    }
}

fn source_path(state: &AccountState) -> String {
    state
        .codex
        .as_ref()
        .map(|source| display_path(&source.path))
        .unwrap_or_default()
}

fn error_text(state: &AccountState) -> String {
    let source_error = match state.codex.as_ref().map(|source| &source.status) {
        Some(CodexStatus::Failed(message)) => Some(message),
        Some(CodexStatus::Read { .. }) => None,
        None => state.session_error.as_ref(),
    };
    state
        .error
        .as_ref()
        .or(source_error)
        .cloned()
        .unwrap_or_default()
}

fn label(state: &AccountState, action: ButtonAction) -> &'static str {
    match action {
        ButtonAction::SelectCodex if state.uses_codex() => "更改 Codex 凭据文件",
        ButtonAction::SelectCodex => "选择 Codex 凭据文件",
        ButtonAction::DisconnectCodex => "断开 Codex 来源",
        ButtonAction::Restore => "重新读取账号",
        ButtonAction::Login(LoginAction::Login)
            if matches!(state.credentials, CredentialState::StoredSession { .. }) =>
        {
            "重新登录"
        }
        ButtonAction::Login(LoginAction::Login) => "使用 ChatGPT 登录",
        ButtonAction::Login(LoginAction::Consent) => "开启订阅授权",
        ButtonAction::Login(LoginAction::OpenBrowser) => "重新打开浏览器",
        ButtonAction::Login(LoginAction::CopyLink) => "复制登录链接",
        ButtonAction::Login(LoginAction::Cancel) => "取消登录",
        ButtonAction::Login(LoginAction::RetrySave) => "重试保存",
        ButtonAction::Login(LoginAction::CheckSession)
            if state.session_activity == SessionActivity::SaveFailed =>
        {
            "重试保存凭据"
        }
        ButtonAction::Login(LoginAction::CheckSession) => "检查会话",
        ButtonAction::Login(LoginAction::SignOut) if state.phase == AccountPhase::SignOutFailed => {
            "重试退出登录"
        }
        ButtonAction::Login(LoginAction::SignOut) => "退出登录",
    }
}
fn button_state(state: &AccountState, action: ButtonAction) -> (bool, bool, bool) {
    let idle = state.phase == AccountPhase::Idle;
    let changing_source = state.phase == AccountPhase::ChangingSource;
    if (state.uses_codex() || changing_source)
        && matches!(action, ButtonAction::Restore | ButtonAction::Login(_))
    {
        return (false, true, false);
    }
    let ending = matches!(
        state.phase,
        AccountPhase::SigningOut
            | AccountPhase::Revoking
            | AccountPhase::Clearing
            | AccountPhase::SignOutFailed
    );
    let (visible, loading) = match action {
        ButtonAction::SelectCodex => (idle || changing_source, changing_source),
        ButtonAction::DisconnectCodex => (idle && state.uses_codex(), false),
        ButtonAction::Restore => (
            idle || state.phase == AccountPhase::Reading,
            state.phase == AccountPhase::Reading,
        ),
        ButtonAction::Login(LoginAction::Login) => (
            idle || state.phase == AccountPhase::Preparing,
            state.phase == AccountPhase::Preparing,
        ),
        ButtonAction::Login(LoginAction::Consent) => (
            idle && matches!(
                state.credentials,
                CredentialState::StoredSession {
                    subscription_enabled: false,
                    ..
                }
            ),
            false,
        ),
        ButtonAction::Login(LoginAction::OpenBrowser | LoginAction::CopyLink) => {
            (state.phase == AccountPhase::Waiting, false)
        }
        ButtonAction::Login(LoginAction::Cancel) => (
            !idle && state.phase != AccountPhase::Reading && !ending,
            state.phase == AccountPhase::Cancelling,
        ),
        ButtonAction::Login(LoginAction::RetrySave) => (
            matches!(state.phase, AccountPhase::SaveFailed | AccountPhase::Saving),
            state.phase == AccountPhase::Saving,
        ),
        ButtonAction::Login(LoginAction::CheckSession) => (
            idle && state.account.is_some()
                && !matches!(state.credentials, CredentialState::SignedOut),
            matches!(
                state.session_activity,
                SessionActivity::Checking | SessionActivity::Refreshing | SessionActivity::Saving
            ),
        ),
        ButtonAction::Login(LoginAction::SignOut) => (
            ending
                || matches!(
                    state.credentials,
                    CredentialState::StoredSession { .. }
                        | CredentialState::ReauthenticationRequired
                ),
            ending && state.phase != AccountPhase::SignOutFailed,
        ),
    };
    let browser_busy =
        matches!(action, ButtonAction::Login(LoginAction::OpenBrowser)) && state.browser_opening;
    (
        visible,
        state.runtime_failed
            || loading
            || browser_busy
            || (ending && !matches!(action, ButtonAction::Login(LoginAction::SignOut)))
            || (state.session_activity != SessionActivity::Idle
                && !matches!(
                    action,
                    ButtonAction::Login(LoginAction::CheckSession | LoginAction::SignOut)
                ))
            || (state.session_client_error
                && matches!(
                    action,
                    ButtonAction::Login(LoginAction::Login | LoginAction::Consent)
                )),
        loading || browser_busy,
    )
}
fn button(
    state: &AccountState,
    action: ButtonAction,
    name: &'static str,
    model_busy: bool,
) -> impl Scene {
    let text = label(state, action);
    let (visible, disabled, _) = button_state(state, action);
    let disabled = disabled || model_blocks(model_busy, action);
    let display = if visible {
        Display::Flex
    } else {
        Display::None
    };
    (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! {
                    Text(text) WidgetryText template(move |_| Ok(ButtonLabel(action)))
                }) as Box<dyn SceneList>)}
            }
            Name(name) template(move |_| Ok(action))
            Node { min_width: px(BUTTON_WIDTH), height: px(BUTTON_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0, display: display }
            on(activate)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}
fn model_blocks(busy: bool, action: ButtonAction) -> bool {
    busy && !matches!(action, ButtonAction::Login(LoginAction::SignOut))
}
pub(crate) fn account_panel(state: &AccountState, model_busy: bool) -> impl Scene {
    let name = identity(state);
    let status = status(state);
    let error = error_text(state);
    let source = source_label(state);
    let path = source_path(state);
    let path_display = optional_display(&path);
    let feedback = state.feedback.clone().unwrap_or_default();
    bsn! {
        Name("AccountPanel")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ACCOUNT_GAP), flex_shrink: 0.0 }
        Children [
            Text(source) WidgetryText AccountSourceLabel Name("AccountSourceLabel")--
            Text(path) WidgetryText AccountSourcePath Name("AccountSourcePath")
            TextLayout::linebreak(LineBreak::AnyCharacter)
            Node { width: percent(100), min_width: px(0), display: path_display }--
            Text(name) WidgetryText AccountIdentity Name("AccountIdentity")
            TextLayout::linebreak(LineBreak::AnyCharacter) Node { width: percent(100), min_width: px(0) }--
            Text(status) WidgetryText AccountStatus Name("AccountStatus")--
            Text(feedback) WidgetryText AccountFeedback Name("AccountFeedback")--
            Text(error) WidgetryText AccountError Name("AccountError")
            TextLayout::linebreak(LineBreak::AnyCharacter) Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            Node { width: percent(100), column_gap: px(ACCOUNT_GAP), row_gap: px(ACCOUNT_GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button(state, ButtonAction::Login(LoginAction::Login), "AccountLogin", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::Consent), "AccountConsent", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::CheckSession), "AccountCheckSession", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::SignOut), "AccountSignOut", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::OpenBrowser), "AccountOpenBrowser", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::CopyLink), "AccountCopyLink", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::RetrySave), "AccountRetrySave", model_busy)--
                @button(state, ButtonAction::Login(LoginAction::Cancel), "AccountCancel", model_busy)--
                @button(state, ButtonAction::Restore, "AccountRestore", model_busy)--
                @button(state, ButtonAction::SelectCodex, "AccountSelectCodex", model_busy)--
                @button(state, ButtonAction::DisconnectCodex, "AccountDisconnectCodex", model_busy)
            ]
        ]
    }
}
fn activate(
    event: On<Activate>,
    actions: Query<&ButtonAction>,
    state: Res<AccountState>,
    model: Res<agentelier_agent_host::HostActivity>,
    mut commands: Commands,
) -> Result {
    let action = *actions.get(event.entity)?;
    let (visible, disabled, _) = button_state(&state, action);
    if visible && !disabled && !model_blocks(model.model_busy, action) {
        match action {
            ButtonAction::Restore => {
                commands.trigger(RestoreAccount);
            }
            ButtonAction::Login(action) => {
                commands.trigger(action);
            }
            ButtonAction::SelectCodex => commands.queue(crate::directory_settings::pick_codex),
            ButtonAction::DisconnectCodex => commands.queue(|world: &mut World| {
                crate::authentication::change_source(
                    world,
                    agentelier_agent_host::SourceChange::Disconnect,
                )
            }),
        }
    }
    Ok(())
}
fn update_panel(
    state: Res<AccountState>,
    model: Res<agentelier_agent_host::HostActivity>,
    mut text: Query<
        (
            &mut Text,
            Has<AccountIdentity>,
            Has<AccountStatus>,
            Has<AccountError>,
            Has<AccountFeedback>,
            Option<&ButtonLabel>,
        ),
        (Without<AccountSourceLabel>, Without<AccountSourcePath>),
    >,
    mut buttons: Query<(Entity, &ButtonAction, &mut Node)>,
    mut sources: Query<
        (&mut Text, &mut Node, Has<AccountSourcePath>),
        (
            Without<ButtonAction>,
            Or<(With<AccountSourceLabel>, With<AccountSourcePath>)>,
        ),
    >,
    mut commands: Commands,
) {
    if !state.is_changed() && !model.is_changed() {
        return;
    }
    for (mut text, is_identity, is_status, is_error, is_feedback, button) in &mut text {
        if is_identity {
            text.0 = identity(&state);
        } else if is_status {
            text.0 = status(&state).into();
        } else if is_error {
            text.0 = error_text(&state);
        } else if is_feedback {
            text.0 = state.feedback.clone().unwrap_or_default();
        } else if let Some(button) = button {
            text.0 = label(&state, button.0).into();
        }
    }
    for (mut text, mut node, is_path) in &mut sources {
        if is_path {
            text.0 = source_path(&state);
            node.display = optional_display(&text.0);
        } else {
            text.0 = source_label(&state).into();
        }
    }
    for (entity, action, mut node) in &mut buttons {
        let (visible, disabled, loading) = button_state(&state, *action);
        let disabled = disabled || model_blocks(model.model_busy, *action);
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if disabled {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
        WidgetryButtonLoading::set(&mut commands, entity, loading);
    }
}

fn optional_display(text: &str) -> Display {
    if text.is_empty() {
        Display::None
    } else {
        Display::Flex
    }
}
