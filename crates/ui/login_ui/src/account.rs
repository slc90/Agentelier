use agentelier_account_bridge::QuotaStatus;
use agentelier_agent_host::HostActivity;
use agentelier_controls::{
    AgentelierAvatar, AgentelierButton, AgentelierButtonStyle, AvatarPresentation, AvatarSource,
    SyncAvatars,
};
use bevy::picking::{
    events::{PointerClick, PointerPress},
    pointer::PointerButton,
};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{
    Activate,
    popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide},
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::style::z_index;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::theme::WidgetryTheme;
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, owned_widgetry_window,
};

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
    pub(crate) logout_cleanup_failed: bool,
    #[reflect(ignore)]
    pub(crate) quota: Option<QuotaStatus>,
}

#[derive(Reflect, Clone)]
pub(crate) struct AccountOperation {
    pub(crate) id: u64,
    pub(crate) phase: AccountPhase,
}

#[derive(Reflect, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountPhase {
    PreparingLink,
    WaitingForBrowser,
    LinkCopied,
    Cancelling,
    SigningOut,
}

impl AccountState {
    pub fn quota(&self) -> Option<&QuotaStatus> {
        self.quota.as_ref().filter(|quota| {
            self.model_identity() == Some((quota.subject.as_str(), quota.workspace_id.as_str()))
        })
    }

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

    pub fn signed_in(&self) -> bool {
        matches!(self.codex, Some(CodexStatus::Read { .. }))
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
        if self.logout_cleanup_failed {
            return Some("登出未完成，请重试");
        }
        (!self.signed_in()).then_some("请先使用 ChatGPT 登录")
    }

    pub fn model_identity(&self) -> Option<(&str, &str)> {
        if self.logout_cleanup_failed {
            return None;
        }
        match self.codex.as_ref()? {
            CodexStatus::Read {
                subject,
                workspace_id,
                ..
            } => Some((subject, workspace_id)),
            CodexStatus::Failed(_) => None,
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
enum AuthorizationText {
    Status,
    Message,
}

impl AuthorizationText {
    fn content(self, state: &AccountState) -> String {
        match self {
            Self::Status => match state.phase() {
                Some(AccountPhase::PreparingLink) => "正在生成授权链接…",
                Some(AccountPhase::WaitingForBrowser) => "请复制授权链接并在浏览器中打开",
                Some(AccountPhase::LinkCopied) => "已复制授权链接，请在浏览器中打开",
                Some(AccountPhase::Cancelling) => "正在取消登录…",
                Some(AccountPhase::SigningOut) => "正在退出登录…",
                None if state.signed_in() => "已登录 ChatGPT",
                None => "未登录 ChatGPT",
            }
            .into(),
            Self::Message => state
                .error
                .as_ref()
                .or(state.notice.as_ref())
                .or(match &state.codex {
                    Some(CodexStatus::Failed(message)) => Some(message),
                    _ => None,
                })
                .cloned()
                .unwrap_or_default(),
        }
    }
}

#[derive(Component, Clone, Default)]
struct AuthorizationWindow;

#[derive(Component, Clone, Default)]
struct AuthorizationLink;

#[derive(Component, Clone, Default)]
struct CopyLink;

#[derive(Component, Clone, Default)]
struct LogoutMenu;

#[derive(Component, Clone, Default)]
struct LoginAvatar;

fn sync_login_avatar(
    account: Res<AccountState>,
    mut avatars: Query<&mut AvatarPresentation, With<LoginAvatar>>,
) {
    for mut avatar in &mut avatars {
        if account.signed_in() && matches!(avatar.source, AvatarSource::DefaultUser) {
            avatar.source = AvatarSource::User;
        } else if !account.signed_in() && matches!(avatar.source, AvatarSource::User) {
            avatar.source = AvatarSource::DefaultUser;
        }
    }
}

pub(crate) fn install(app: &mut App) {
    agentelier_controls::install_owned_window_close::<With<AuthorizationWindow>>(app);
    app.init_resource::<AccountState>()
        .register_type::<AccountState>()
        .add_observer(close_authorization)
        .add_systems(Update, dismiss_menu)
        .add_systems(PostUpdate, sync_login_avatar.before(SyncAvatars))
        .add_systems(
            PostUpdate,
            (update_authorization, update_link).before(UiSystems::Content),
        );
}

pub fn login_entry(avatar_size: f32) -> impl Scene {
    bsn! {
        @AgentelierButton {
            @style: AgentelierButtonStyle::Icon,
            @children: {Some(Box::new(bsn_list! {
                @AgentelierAvatar { @size: avatar_size, @source: AvatarSource::DefaultUser } LoginAvatar Name("WorkspaceLoginAvatar")
            }) as Box<dyn SceneList>)}
        }
        Name("WorkspaceLogin")
        on(open_authorization)
        on(secondary_click)
    }
}

fn open_authorization(
    _event: On<Activate>,
    state: Res<AccountState>,
    model: Res<HostActivity>,
    windows: Query<(), With<AuthorizationWindow>>,
    mut commands: Commands,
) {
    if state.signed_in() || model.model_busy {
        return;
    }
    if windows.is_empty() {
        commands.spawn_scene_with_error_handler(authorization_window());
    }
    commands.queue(crate::authentication::begin_login);
}

fn authorization_window() -> impl Scene {
    bsn! {
        AuthorizationWindow Name("AuthorizationWindow")
        @owned_widgetry_window(
            Window { title: "ChatGPT 授权".into(), resolution: (640, 330).into(), ..default() },
            WidgetryWindowControlsConfig { minimize_visible: false, maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(12), padding: UiRect::all(px(16)) }
                Children [
                    Text::default() WidgetryText template(|_| Ok(AuthorizationText::Status)) Name("AuthorizationStatus")--
                    @WidgetryReadOnlyTextField {
                        @multiline: true,
                        @auto_size: {Some(AutoSize { min_rows: 2, max_rows: 4 })},
                    }
                    AuthorizationLink Name("AuthorizationLink")
                    Node { width: percent(100), min_width: px(0), display: Display::None }--
                    @AgentelierButton {
                        @children: {Some(Box::new(bsn_list! { Text("复制授权链接") WidgetryText }) as Box<dyn SceneList>)}
                    }
                    CopyLink Name("CopyAuthorizationLink")
                    Node { align_self: AlignSelf::Start, display: Display::None }
                    on(copy_link)--
                    Text::default() WidgetryText template(|_| Ok(AuthorizationText::Message)) Name("AuthorizationMessage")
                    TextLayout::linebreak(LineBreak::AnyCharacter)
                ]
            },
        )
    }
}

fn copy_link(_event: On<Activate>, mut commands: Commands) {
    commands.queue(crate::authentication::copy_login_url);
}

fn close_authorization(_event: On<Despawn<AuthorizationWindow>>, mut commands: Commands) {
    commands.queue(crate::authentication::cancel_login);
}

fn update_authorization(
    state: Res<AccountState>,
    mut labels: Query<(&AuthorizationText, &mut Text)>,
    mut buttons: Query<&mut Node, With<CopyLink>>,
) {
    for (field, mut text) in &mut labels {
        text.set_if_neq(Text(field.content(&state)));
    }
    let display = link_display(&state);
    for mut node in &mut buttons {
        node.display = display;
    }
}

fn update_link(world: &mut World) -> Result {
    let Ok(field) = world
        .query_filtered::<Entity, With<AuthorizationLink>>()
        .single(world)
    else {
        return Ok(());
    };
    let state = world.resource::<AccountState>();
    let display = link_display(state);
    let text = state.authorization_url.clone().unwrap_or_default();
    let shown = WidgetryTextFieldHeadless::text(world, field)
        .map_err(|failure| failure.into_bevy_error())?;
    if shown != text {
        WidgetryTextFieldHeadless::set_text_in_world(world, field, &text)
            .map_err(|failure| failure.into_bevy_error())?;
    }
    if let Some(mut node) = world.get_mut::<Node>(field) {
        node.display = display;
    }
    Ok(())
}

fn link_display(state: &AccountState) -> Display {
    if state.authorization_url.is_some() && !state.source_settling && !state.runtime_failed {
        Display::Flex
    } else {
        Display::None
    }
}

fn secondary_click(
    mut event: On<PointerClick>,
    state: Res<AccountState>,
    model: Res<HostActivity>,
    theme: Res<WidgetryTheme>,
    menus: Query<Entity, With<LogoutMenu>>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Secondary || !state.signed_in() {
        return;
    }
    event.propagate(false);
    if let Ok(menu) = menus.single() {
        commands.entity(menu).try_despawn();
        return;
    }
    let anchor = event.entity;
    let disabled = state.is_busy() || state.runtime_failed || model.model_busy;
    let background = theme.window.frame.normal.background;
    let border = theme.window.frame.normal.border;
    commands.spawn_scene_with_error_handler(bsn! {
        LogoutMenu Name("AccountLogoutMenu") ChildOf(anchor) GlobalZIndex(z_index::POPUP)
        Popover {
            positions: {vec![PopoverPlacement { side: PopoverSide::Right, align: PopoverAlign::Start, gap: 4.0 }]},
            window_margin: 8.0,
        }
        BackgroundColor(background) BorderColor::all(border)
        Node { position_type: PositionType::Absolute, min_width: px(104), padding: UiRect::all(px(4)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)) }
        Children [ @logout_item(disabled) ]
    });
}

fn logout_item(disabled: bool) -> impl Scene {
    (
        bsn! {
            @AgentelierButton {
                @style: AgentelierButtonStyle::Secondary,
                @children: {Some(Box::new(bsn_list! { Text("登出") WidgetryText Pickable::IGNORE }) as Box<dyn SceneList>)}
            }
            Name("AccountLogout")
            Node { width: percent(100) }
            on(logout)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn dismiss_menu(
    mut press: MessageReader<PointerPress>,
    keyboard: Res<ButtonInput<KeyCode>>,
    parents: Query<&ChildOf>,
    menus: Query<(Entity, &ChildOf), With<LogoutMenu>>,
    state: Res<AccountState>,
    mut commands: Commands,
) {
    let outside = |event: &PointerPress, menu: Entity, anchor: Entity| {
        if event.entity == anchor && event.button == PointerButton::Secondary {
            return false;
        }
        event.entity != menu
            && !parents
                .iter_ancestors(event.entity)
                .any(|parent| parent == menu)
    };
    let presses = press.read().collect::<Vec<_>>();
    for (menu, anchor) in &menus {
        if !state.signed_in()
            || keyboard.just_pressed(KeyCode::Escape)
            || presses
                .iter()
                .any(|event| outside(event, menu, anchor.parent()))
        {
            commands.entity(menu).try_despawn();
        }
    }
}

fn logout(
    event: On<Activate>,
    state: Res<AccountState>,
    model: Res<HostActivity>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if !state.signed_in() || state.is_busy() || model.model_busy {
        return;
    }
    if let Ok(menu) = parents.get(event.entity) {
        commands.entity(menu.parent()).try_despawn();
    }
    commands.queue(crate::authentication::logout);
}
