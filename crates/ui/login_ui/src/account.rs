use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::{WidgetryButton, WidgetryButtonLoading};
use bevy_widgetry::text::WidgetryText;

const ACCOUNT_GAP: f32 = 12.0;
const BUTTON_HEIGHT: f32 = 40.0;
const BUTTON_WIDTH: f32 = 168.0;

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct AccountState {
    pub(crate) codex: Option<CodexStatus>,
    pub(crate) source_settling: bool,
    pub(crate) operation: Option<AccountOperation>,
    #[reflect(ignore)]
    pub(crate) authorization_url: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) notice: Option<String>,
    pub runtime_failed: bool,
}

#[derive(Reflect, Clone)]
pub(crate) struct AccountOperation {
    pub(crate) id: u64,
    pub(crate) phase: AccountPhase,
}

#[derive(Reflect, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountPhase {
    OpeningBrowser,
    WaitingForBrowser,
    ManualBrowser,
    LinkCopied,
    Cancelling,
    SigningOut,
}

impl AccountState {
    pub(crate) fn phase(&self) -> Option<AccountPhase> {
        self.operation.as_ref().map(|operation| operation.phase)
    }

    pub(crate) fn clear_operation(&mut self) {
        self.operation = None;
        self.authorization_url = None;
        self.source_settling = false;
        self.error = None;
        self.notice = None;
    }

    pub fn is_busy(&self) -> bool {
        self.operation.is_some() || self.source_settling
    }

    pub fn model_unavailable_reason(&self) -> Option<&'static str> {
        if self.phase() == Some(AccountPhase::SigningOut) {
            return Some("正在退出登录");
        }
        if self.source_settling {
            return Some("正在回收旧账号的任务并保存待办");
        }
        if self.operation.is_some() {
            return Some("正在使用 ChatGPT 登录");
        }
        match self.codex.as_ref() {
            Some(CodexStatus::Read { .. }) => None,
            Some(CodexStatus::Failed(_)) | None => Some("请先使用 ChatGPT 登录"),
        }
    }

    pub fn model_identity(&self) -> Option<(&str, &str)> {
        match self.codex.as_ref()? {
            CodexStatus::Read {
                subject,
                workspace_id,
                ..
            } => Some((subject, workspace_id)),
            _ => None,
        }
    }
}

#[derive(Reflect, Clone)]
pub(crate) enum CodexStatus {
    Read {
        subject: String,
        workspace_id: String,
        email: Option<String>,
    },
    Failed(String),
}

#[derive(Component, Clone, Copy)]
enum ButtonAction {
    Login,
    CancelLogin,
    CopyLoginUrl,
    Logout,
}

impl ButtonAction {
    fn label(self) -> &'static str {
        match self {
            Self::Login => "使用 ChatGPT 登录",
            Self::CancelLogin => "取消登录",
            Self::CopyLoginUrl => "复制授权链接",
            Self::Logout => "退出登录",
        }
    }

    fn visible(self, state: &AccountState) -> bool {
        match self {
            Self::Login => state.model_identity().is_none(),
            Self::CancelLogin => {
                state.operation.is_some()
                    && state.phase() != Some(AccountPhase::SigningOut)
                    && !state.source_settling
            }
            Self::CopyLoginUrl => state.authorization_url.is_some() && !state.source_settling,
            Self::Logout => state.codex.is_some(),
        }
    }

    fn disabled(self, state: &AccountState, model_busy: bool) -> bool {
        state.runtime_failed
            || match self {
                Self::Login | Self::Logout => state.is_busy() || model_busy,
                Self::CancelLogin => state.phase() == Some(AccountPhase::Cancelling),
                Self::CopyLoginUrl => state.phase() == Some(AccountPhase::Cancelling),
            }
    }
}

#[derive(Component, Clone, Copy)]
enum AccountText {
    Status,
    Identity,
    Message,
}

impl AccountText {
    fn content(self, state: &AccountState) -> String {
        let source = state.codex.as_ref();
        match self {
            Self::Status => match state.phase() {
                Some(AccountPhase::SigningOut) => "正在退出登录并收尾…",
                _ if state.source_settling => "正在回收旧账号的任务并保存待办…",
                Some(AccountPhase::OpeningBrowser) => "正在打开浏览器…",
                Some(AccountPhase::WaitingForBrowser) => "正在等待浏览器授权…",
                Some(AccountPhase::ManualBrowser) => "请复制授权链接并在浏览器中打开",
                Some(AccountPhase::LinkCopied) => "已复制授权链接，请在浏览器中打开",
                Some(AccountPhase::Cancelling) => "正在取消登录…",
                None => match source {
                    Some(CodexStatus::Read { .. }) => "已登录 ChatGPT",
                    Some(CodexStatus::Failed(_)) => "登录凭据不可用",
                    None => "未登录 ChatGPT",
                },
            }
            .into(),
            Self::Identity => match source {
                Some(CodexStatus::Read {
                    subject,
                    workspace_id,
                    email,
                    ..
                }) => {
                    format!(
                        "{} · 工作区 {workspace_id}",
                        email.as_deref().unwrap_or(subject)
                    )
                }
                _ => String::new(),
            },
            Self::Message => match (state.error.as_ref().or(state.notice.as_ref()), source) {
                (Some(message), _) | (None, Some(CodexStatus::Failed(message))) => message.clone(),
                _ => String::new(),
            },
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<AccountState>()
        .register_type::<AccountState>()
        .add_systems(PostUpdate, update_panel.before(UiSystems::Content));
}

fn button(
    state: &AccountState,
    action: ButtonAction,
    name: &'static str,
    model_busy: bool,
) -> impl Scene {
    let label = action.label();
    let visibility = display(action.visible(state));
    let disabled = action.disabled(state, model_busy);
    (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! {
                    Text(label) WidgetryText
                }) as Box<dyn SceneList>)}
            }
            Name(name) template(move |_| Ok(action))
            Node { min_width: px(BUTTON_WIDTH), height: px(BUTTON_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0, display: visibility }
            on(activate)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn text(state: &AccountState, field: AccountText, name: &'static str) -> impl Scene {
    let value = field.content(state);
    let visibility = display(!value.is_empty());
    bsn! {
        Text(value) WidgetryText Name(name) template(move |_| Ok(field))
        TextLayout::linebreak(LineBreak::AnyCharacter)
        Node { width: percent(100), min_width: px(0), display: visibility }
    }
}

pub(crate) fn account_panel(state: &AccountState, model_busy: bool) -> impl Scene {
    bsn! {
        Name("AccountPanel")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(ACCOUNT_GAP) }
        Children [
            @text(state, AccountText::Status, "AccountStatus")--
            @text(state, AccountText::Identity, "AccountIdentity")--
            @text(state, AccountText::Message, "AccountMessage")--
            Node { flex_wrap: FlexWrap::Wrap, column_gap: px(ACCOUNT_GAP), row_gap: px(ACCOUNT_GAP) }
            Children [
                @button(state, ButtonAction::Login, "AccountLogin", model_busy)--
                @button(state, ButtonAction::CancelLogin, "AccountCancelLogin", model_busy)--
                @button(state, ButtonAction::CopyLoginUrl, "AccountCopyLoginUrl", model_busy)--
                @button(state, ButtonAction::Logout, "AccountLogout", model_busy)
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
    if action.disabled(&state, model.model_busy) || !action.visible(&state) {
        return Ok(());
    }
    match action {
        ButtonAction::Login => commands.queue(crate::authentication::begin_login),
        ButtonAction::CancelLogin => commands.queue(crate::authentication::cancel_login),
        ButtonAction::CopyLoginUrl => commands.queue(crate::authentication::copy_login_url),
        ButtonAction::Logout => commands.queue(crate::authentication::logout),
    }
    Ok(())
}

fn update_panel(
    state: Res<AccountState>,
    model: Res<agentelier_agent_host::HostActivity>,
    mut text: Query<(&AccountText, &mut Text, Option<&mut Node>), Without<ButtonAction>>,
    mut buttons: Query<(Entity, &ButtonAction, &mut Node)>,
    mut commands: Commands,
) {
    if !state.is_changed() && !model.is_changed() {
        return;
    }
    for (field, mut text, node) in &mut text {
        text.set_if_neq(Text(field.content(&state)));
        if let Some(mut node) = node {
            node.display = display(!text.0.is_empty());
        }
    }
    for (entity, action, mut node) in &mut buttons {
        node.display = display(action.visible(&state));
        if action.disabled(&state, model.model_busy) {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
        WidgetryButtonLoading::set(
            &mut commands,
            entity,
            match action {
                ButtonAction::Login => matches!(
                    state.phase(),
                    Some(
                        AccountPhase::OpeningBrowser
                            | AccountPhase::WaitingForBrowser
                            | AccountPhase::ManualBrowser
                            | AccountPhase::LinkCopied
                    )
                ),
                ButtonAction::Logout => state.phase() == Some(AccountPhase::SigningOut),
                ButtonAction::CancelLogin => state.phase() == Some(AccountPhase::Cancelling),
                ButtonAction::CopyLoginUrl => false,
            },
        );
    }
}

fn display(visible: bool) -> Display {
    if visible {
        Display::Flex
    } else {
        Display::None
    }
}
