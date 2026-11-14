use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;

const ACCOUNT_GAP: f32 = 12.0;
const RETRY_HEIGHT: f32 = 40.0;
const RETRY_WIDTH: f32 = 144.0;

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct AccountState {
    pub account: Option<AccountSummary>,
    pub credentials: CredentialState,
    pub operation: Option<u64>,
    pub error: Option<String>,
    pub runtime_failed: bool,
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
    StoredSession {
        expired: bool,
    },
}

#[derive(Event)]
pub struct RestoreAccount;

#[derive(Component, Clone, Default)]
struct AccountIdentity;
#[derive(Component, Clone, Default)]
struct AccountStatus;
#[derive(Component, Clone, Default)]
struct AccountError;
#[derive(Component, Clone, Default)]
struct AccountRetry;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AccountState>()
        .register_type::<AccountState>()
        .add_systems(PostUpdate, update_panel.before(UiSystems::Content));
}

fn identity(state: &AccountState) -> String {
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
    if state.operation.is_some() {
        return "正在读取账号…";
    }
    if state.error.is_some() {
        return "账号读取失败";
    }
    match state.credentials {
        CredentialState::Unknown => "正在读取账号…",
        CredentialState::NoRecord | CredentialState::HostOnly | CredentialState::SignedOut => {
            "未登录"
        }
        CredentialState::StoredSession { expired: true } => "已恢复账号，会话已过期",
        CredentialState::StoredSession { expired: false } => "已恢复账号会话",
    }
}

pub(crate) fn account_panel(state: &AccountState) -> impl Scene {
    let name = identity(state);
    let status = status(state);
    let error = state.error.clone().unwrap_or_default();
    let disabled = state.operation.is_some() || state.runtime_failed;
    let retry_button = (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! {Text("重新读取账号") WidgetryText}) as Box<dyn SceneList>)}
            }
            AccountRetry Name("AccountRestore")
            Node { min_width: px(RETRY_WIDTH), height: px(RETRY_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0 }
            on(retry)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    );
    bsn! {
        Name("AccountPanel")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(ACCOUNT_GAP), flex_shrink: 0.0 }
        Children [
            Text("账号") WidgetryText--
            Text(name) WidgetryText AccountIdentity Name("AccountIdentity")--
            Text(status) WidgetryText AccountStatus Name("AccountStatus")--
            Text(error) WidgetryText AccountError Name("AccountError")
            TextLayout::linebreak(LineBreak::AnyCharacter)
            Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            {retry_button}
        ]
    }
}

fn retry(_event: On<Activate>, state: Res<AccountState>, mut commands: Commands) {
    if state.operation.is_none() && !state.runtime_failed {
        commands.trigger(RestoreAccount);
    }
}

fn update_panel(
    state: Res<AccountState>,
    mut text: Query<(
        &mut Text,
        Has<AccountIdentity>,
        Has<AccountStatus>,
        Has<AccountError>,
    )>,
    buttons: Query<Entity, With<AccountRetry>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for (mut text, is_identity, is_status, is_error) in &mut text {
        if is_identity {
            text.0 = identity(&state);
        } else if is_status {
            text.0 = status(&state).into();
        } else if is_error {
            text.0 = state.error.clone().unwrap_or_default();
        }
    }
    for entity in &buttons {
        if state.operation.is_some() || state.runtime_failed {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
    }
}
