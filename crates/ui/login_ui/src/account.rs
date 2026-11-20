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
    pub(crate) source_settling: bool,
    pub(crate) operation: Option<u64>,
    pub(crate) error: Option<String>,
    pub runtime_failed: bool,
}

impl AccountState {
    pub fn is_busy(&self) -> bool {
        self.operation.is_some() || self.source_settling
    }

    pub fn model_unavailable_reason(&self) -> Option<&'static str> {
        if self.source_settling {
            return Some("正在回收旧账号的任务并保存待办");
        }
        if self.operation.is_some() {
            return Some("正在更新 Codex 凭据");
        }
        match self.codex.as_ref().map(|source| &source.status) {
            Some(CodexStatus::Read { expired: false, .. }) => None,
            Some(CodexStatus::Read { expired: true, .. }) => Some("请在 Codex 中恢复授权"),
            Some(CodexStatus::Failed(_)) | None => Some("请选择有效的 Codex 凭据文件"),
        }
    }

    pub fn model_identity(&self) -> Option<(&str, &str)> {
        match &self.codex.as_ref()?.status {
            CodexStatus::Read {
                subject,
                workspace_id,
                expired: false,
                ..
            } => Some((subject, workspace_id)),
            _ => None,
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

#[derive(Component, Clone, Copy)]
enum ButtonAction {
    SelectCodex,
    DisconnectCodex,
}

impl ButtonAction {
    fn label(self, state: &AccountState) -> &'static str {
        match self {
            Self::SelectCodex if state.codex.is_some() => "更改 Codex 凭据文件",
            Self::SelectCodex => "选择 Codex 凭据文件",
            Self::DisconnectCodex => "断开 Codex 来源",
        }
    }

    fn visible(self, state: &AccountState) -> bool {
        matches!(self, Self::SelectCodex) || state.codex.is_some()
    }
}

#[derive(Component, Clone, Copy)]
enum AccountText {
    Status,
    Path,
    Identity,
    Error,
    Button(ButtonAction),
}

impl AccountText {
    fn content(self, state: &AccountState) -> String {
        let source = state.codex.as_ref();
        match self {
            Self::Status if state.source_settling => "正在回收旧账号的任务并保存待办…".into(),
            Self::Status if state.operation.is_some() => "正在更新 Codex 凭据…".into(),
            Self::Status => match source.map(|file| &file.status) {
                Some(CodexStatus::Read { expired: false, .. }) => "已读取 Codex 凭据",
                Some(CodexStatus::Read { expired: true, .. }) => {
                    "Codex 凭据已过期，请在 Codex 中恢复授权"
                }
                Some(CodexStatus::Failed(_)) => "Codex 凭据不可用",
                None => "未连接。请选择 Codex 的 auth.json 文件",
            }
            .into(),
            Self::Path => source
                .map(|file| display_path(&file.path))
                .unwrap_or_default(),
            Self::Identity => match source.map(|file| &file.status) {
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
            Self::Error => match (&state.error, source.map(|file| &file.status)) {
                (Some(error), _) | (None, Some(CodexStatus::Failed(error))) => error.clone(),
                _ => String::new(),
            },
            Self::Button(action) => action.label(state).into(),
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
    let text = AccountText::Button(action);
    let label = text.content(state);
    let visibility = display(action.visible(state));
    let disabled = state.runtime_failed || state.is_busy() || model_busy;
    (
        bsn! {
            @WidgetryButton {
                @children: {Some(Box::new(bsn_list! {
                    Text(label) WidgetryText template(move |_| Ok(text))
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
            @text(state, AccountText::Path, "AccountSourcePath")--
            @text(state, AccountText::Identity, "AccountIdentity")--
            @text(state, AccountText::Error, "AccountError")--
            Node { flex_wrap: FlexWrap::Wrap, column_gap: px(ACCOUNT_GAP), row_gap: px(ACCOUNT_GAP) }
            Children [
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
    if state.runtime_failed || state.is_busy() || model.model_busy || !action.visible(&state) {
        return Ok(());
    }
    match action {
        ButtonAction::SelectCodex => commands.queue(crate::directory_settings::pick_codex),
        ButtonAction::DisconnectCodex => commands.queue(|world: &mut World| {
            crate::authentication::change_source(
                world,
                agentelier_agent_host::SourceChange::Disconnect,
            )
        }),
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
        if state.runtime_failed || state.is_busy() || model.model_busy {
            commands.entity(entity).insert(InteractionDisabled);
        } else {
            commands.entity(entity).remove::<InteractionDisabled>();
        }
        WidgetryButtonLoading::set(&mut commands, entity, state.operation.is_some());
    }
}

fn display(visible: bool) -> Display {
    if visible {
        Display::Flex
    } else {
        Display::None
    }
}
