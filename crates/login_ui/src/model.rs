use crate::{AccountPhase, AccountState, CredentialState, SessionActivity};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::{WidgetryButton, WidgetryButtonLoading};
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;

const PANEL_GAP: f32 = 12.0;
const BUTTON_WIDTH: f32 = 168.0;
const BUTTON_HEIGHT: f32 = 40.0;

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct ModelState {
    pub operation: Option<u64>,
    pub model: Option<String>,
    pub text: Option<String>,
    pub error: Option<String>,
}
impl ModelState {
    pub fn can_start(&self, account: &AccountState) -> bool {
        self.operation.is_none()
            && account.operation.is_none()
            && account.phase == AccountPhase::Idle
            && !account.runtime_failed
            && account.session_activity == SessionActivity::Idle
            && matches!(
                account.credentials,
                CredentialState::StoredSession {
                    subscription_enabled: true,
                    ..
                }
            )
    }
}
#[derive(Event)]
pub struct VerifyModel;
#[derive(Component, Clone, Default)]
struct VerifyButton;
#[derive(Component, Clone, Default)]
struct ModelStatus;
#[derive(Component, Clone, Default)]
struct ModelText;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ModelState>()
        .register_type::<ModelState>()
        .add_systems(PostUpdate, update.before(UiSystems::Content));
}
fn status(state: &ModelState, account: &AccountState) -> String {
    if state.operation.is_some() {
        return "正在验证模型连接…".into();
    }
    if let Some(error) = &state.error {
        return error.clone();
    }
    if let Some(model) = &state.model {
        return format!("连接完成 · {model}");
    }
    if !matches!(
        account.credentials,
        CredentialState::StoredSession {
            subscription_enabled: true,
            ..
        }
    ) {
        return "请先登录并开启订阅授权".into();
    }
    "使用当前账号发送一次独立的简短请求".into()
}
pub(crate) fn panel(state: &ModelState, account: &AccountState) -> impl Scene {
    let status = status(state, account);
    let text = state.text.clone().unwrap_or_default();
    let disabled = !state.can_start(account);
    bsn! {
        Name("ModelConnectionPanel")
        Node { width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(PANEL_GAP) }
        Children [
            @verify_button(disabled)--
            Text(status) WidgetryText ModelStatus Name("ModelConnectionStatus")
            TextLayout::linebreak(LineBreak::AnyCharacter)
            Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            @WidgetryScrollArea {
                @children: {Some(Box::new(bsn_list! {
                    Text(text) WidgetryText ModelText Name("ModelConnectionText")
                    TextLayout::linebreak(LineBreak::AnyCharacter)
                    Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
                }) as Box<dyn SceneList>)}
            }
            Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
        ]
    }
}
fn verify_button(disabled: bool) -> impl Scene {
    (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! { Text("验证模型连接") WidgetryText }) as Box<dyn SceneList>)}
            }
            Name("VerifyModelConnection") VerifyButton
            Node { min_width: px(BUTTON_WIDTH), height: px(BUTTON_HEIGHT), align_self: AlignSelf::Start, flex_shrink: 0.0 }
            on(activate)
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}
fn activate(
    event: On<Activate>,
    buttons: Query<(), With<VerifyButton>>,
    state: Res<ModelState>,
    account: Res<AccountState>,
    mut commands: Commands,
) {
    if buttons.contains(event.entity) && state.can_start(&account) {
        commands.trigger(VerifyModel);
    }
}
fn update(
    state: Res<ModelState>,
    account: Res<AccountState>,
    mut buttons: Query<Entity, With<VerifyButton>>,
    mut texts: Query<(&mut Text, Has<ModelStatus>, Has<ModelText>)>,
    mut commands: Commands,
) {
    if !state.is_changed() && !account.is_changed() {
        return;
    }
    for button in &mut buttons {
        if state.can_start(&account) {
            commands.entity(button).remove::<InteractionDisabled>();
        } else {
            commands.entity(button).insert(InteractionDisabled);
        }
        WidgetryButtonLoading::set(&mut commands, button, state.operation.is_some());
    }
    for (mut text, is_status, is_text) in &mut texts {
        if is_status {
            text.0 = status(&state, &account);
        } else if is_text {
            text.0 = state.text.clone().unwrap_or_default();
        }
    }
}
