use crate::conversation::ChatState;
use agentelier_controls::{AgentelierAvatar, AgentelierButton, AgentelierButtonStyle};
use agentelier_login_ui::{AccountState, QuotaWindow};
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_app_assets::NavigationIcon;
use bevy_widgetry::icon::WidgetryIcon;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::splitter::{WidgetrySplitter, WidgetrySplitterPlugin};
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{TooltipContentFactory, WidgetryTooltip};
use bevy_widgetry::window::WidgetryWindowTitle;

pub const MAIN_WINDOW_WIDTH: f32 = 1920.0;
pub const MIN_MAIN_WINDOW_WIDTH: f32 =
    NAV_WIDTH + SESSION_MIN + CHAT_MIN + RIGHT_MIN + 2.0 * HANDLE_WIDTH + FRAME_BORDERS;

const TITLE_BOTTOM_PADDING: f32 = 4.0;
const FRAME_BORDERS: f32 = 2.0;
const NAV_WIDTH: f32 = 64.0;
const NAV_GAP: f32 = 12.0;
const NAV_ICON_SIZE: u32 = 20;
const NAV_AVATAR_SIZE: f32 = 24.0;
const SESSION_INITIAL: f32 = 280.0;
const SESSION_MIN: f32 = 220.0;
const CHAT_MIN: f32 = 640.0;
const RIGHT_INITIAL: f32 = 360.0;
const RIGHT_MIN: f32 = 320.0;
const PANE_PADDING: f32 = 12.0;
const HANDLE_WIDTH: f32 = 8.0;
const SEPARATOR_WIDTH: f32 = 1.0;
const WORKSPACE_SPLIT_LENGTH: f32 = MAIN_WINDOW_WIDTH - FRAME_BORDERS - NAV_WIDTH - HANDLE_WIDTH;
const CHAT_SPLIT_LENGTH: f32 = WORKSPACE_SPLIT_LENGTH - SESSION_INITIAL - HANDLE_WIDTH;
const SESSION_RATIO: f32 = SESSION_INITIAL / WORKSPACE_SPLIT_LENGTH;
const CHAT_RATIO: f32 = (CHAT_SPLIT_LENGTH - RIGHT_INITIAL) / CHAT_SPLIT_LENGTH;
const SIDE_BACKGROUND: Color = Color::srgb_u8(0x0D, 0x26, 0x1B);
const PANEL_BACKGROUND: Color = Color::srgb_u8(0x14, 0x35, 0x26);

#[derive(Component, Clone, Default)]
struct ContextStatus;

#[derive(Component, Clone, Default)]
struct QuotaStatusLabel;

#[derive(Component, Clone, Copy)]
struct InitialSplit {
    ratio: f32,
    width: f32,
}

#[derive(Component, Clone, Copy)]
enum Entry {
    Login,
    Settings,
    Skills,
    Mcp,
    Memory,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Self::Login => "WorkspaceLogin",
            Self::Settings => "WorkspaceSettings",
            Self::Skills => "WorkspaceSkills",
            Self::Mcp => "WorkspaceMcp",
            Self::Memory => "WorkspaceMemory",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Login => "登录与账号",
            Self::Settings => "设置",
            Self::Skills => "Skill 管理",
            Self::Mcp => "MCP 管理",
            Self::Memory => "记忆管理",
        }
    }
}

pub fn workspace_title() -> impl Scene {
    bsn! {
        Name("WorkspaceTitle")
        Pickable::IGNORE
        Node { min_width: px(0), align_items: AlignItems::Center, column_gap: px(NAV_GAP), padding: UiRect::horizontal(px(PANE_PADDING)) }
        Children [
            WidgetryWindowTitle WidgetryText Pickable::IGNORE--
            Text::default() WidgetryText Pickable::IGNORE QuotaStatusLabel Name("AccountQuotaStatus")
            TextLayout::no_wrap()--
            Text::default() WidgetryText Pickable::IGNORE ContextStatus Name("WorkspaceContextStatus")
            TextLayout::no_wrap()
        ]
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_plugins(WidgetrySplitterPlugin)
        .add_systems(
            PostUpdate,
            (sync_context, sync_quota).before(UiSystems::Content),
        )
        .add_systems(PostUpdate, initialize_splits.after(UiSystems::Layout));
}

fn initialize_splits(
    mut commands: Commands,
    splits: Query<(Entity, &InitialSplit, &ComputedNode)>,
) {
    for (root, initial, geometry) in &splits {
        let width = geometry.size().x * geometry.inverse_scale_factor();
        if width + 1.0 >= initial.width {
            WidgetrySplitter::set_split_ratio(&mut commands, root, initial.ratio);
            commands.entity(root).remove::<InitialSplit>();
        }
    }
}

fn split_changed(event: On<ValueChange<f32>>, mut commands: Commands) {
    commands.entity(event.source).remove::<InitialSplit>();
}

fn sync_context(state: Res<ChatState>, mut labels: Query<&mut Text, With<ContextStatus>>) {
    let label = state
        .session
        .as_ref()
        .map_or("", |session| session.context.as_str());
    for mut text in &mut labels {
        if text.0 != label {
            text.0 = label.to_owned();
        }
    }
}

fn sync_quota(state: Res<AccountState>, mut labels: Query<&mut Text, With<QuotaStatusLabel>>) {
    for mut text in &mut labels {
        if !state.is_changed() && !text.is_added() {
            continue;
        }
        let quota = state.quota();
        let hours = quota.and_then(|quota| quota.five_hours.as_ref());
        let week = quota.and_then(|quota| quota.week.as_ref());
        let stale = quota.is_some_and(|quota| quota.stale && quota.fetched_at.is_some());
        let mark = if stale { "（已过期）" } else { "" };
        text.set_if_neq(Text(format!(
            "5 小时 {}  ·  周额度 {}{mark}",
            format_quota_window(hours),
            format_quota_window(week),
        )));
    }
}

fn format_quota_window(window: Option<&QuotaWindow>) -> String {
    let Some(window) = window else {
        return "不可用".into();
    };
    let timestamp = window
        .resets_at
        .and_then(|time| time::OffsetDateTime::from_unix_timestamp(time).ok())
        .map_or_else(
            || "未知".to_owned(),
            |time| {
                format!(
                    "{:02}-{:02} {:02}:{:02} UTC",
                    time.month() as u8,
                    time.day(),
                    time.hour(),
                    time.minute()
                )
            },
        );
    format!("剩余 {:.0}% · 重置 {timestamp}", window.remaining_percent)
}

pub(crate) fn scene(
    chat: impl SceneList + 'static,
    archives: impl SceneList + 'static,
) -> impl Scene {
    let session_pane = Box::new(bsn! {
        Name("WorkspaceC")
        BackgroundColor(SIDE_BACKGROUND)
        Node { min_width: px(SESSION_MIN), padding: UiRect::all(px(PANE_PADDING)), overflow: Overflow::clip() }
        Children [{archives}]
    }) as Box<dyn Scene>;
    let conversation_pane = Box::new(bsn! {
        Name("WorkspaceD")
        Node { min_width: px(CHAT_MIN), padding: UiRect::all(px(PANE_PADDING)), overflow: Overflow::clip() }
        Children [{chat}]
    }) as Box<dyn Scene>;
    let remaining = Box::new(bsn! {
        Node { min_width: px(CHAT_MIN + RIGHT_MIN + HANDLE_WIDTH) }
        Children [
            @WidgetrySplitter {
                @split_ratio: CHAT_RATIO,
                @handle_thickness: HANDLE_WIDTH,
                @line_thickness: SEPARATOR_WIDTH,
                @pane_a: {Some(conversation_pane)},
                @pane_b: {Some(Box::new(right_column()) as Box<dyn Scene>)},
            }
            Name("WorkspaceChatSplit")
            Node { width: percent(100), height: percent(100) }
            template(|_| Ok(InitialSplit { ratio: CHAT_RATIO, width: CHAT_SPLIT_LENGTH + HANDLE_WIDTH }))
            on(split_changed)
        ]
    }) as Box<dyn Scene>;
    bsn! {
        Name("Workspace")
        Node { width: percent(100), height: percent(100), min_height: px(0), padding: UiRect::top(px(TITLE_BOTTOM_PADDING)) }
        Children [
            @navigation()--
            @WidgetrySplitter {
                @split_ratio: SESSION_RATIO,
                @handle_thickness: HANDLE_WIDTH,
                @line_thickness: SEPARATOR_WIDTH,
                @pane_a: {Some(session_pane)},
                @pane_b: {Some(remaining)},
            }
            Name("WorkspaceSessionSplit")
            Node { flex_grow: 1.0, flex_basis: px(0), height: percent(100), min_width: px(0) }
            template(|_| Ok(InitialSplit { ratio: SESSION_RATIO, width: WORKSPACE_SPLIT_LENGTH + HANDLE_WIDTH }))
            on(split_changed)
        ]
    }
}

fn navigation() -> impl Scene {
    bsn! {
        Name("WorkspaceB")
        BackgroundColor(SIDE_BACKGROUND)
        Node { width: px(NAV_WIDTH), height: percent(100), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(NAV_GAP), padding: UiRect::top(px(NAV_GAP)) }
        Children [
            @entry(Entry::Login, bsn_list! { @AgentelierAvatar { @size: NAV_AVATAR_SIZE } })--
            @entry(Entry::Settings, icon(NavigationIcon::Settings))--
            @entry(Entry::Skills, icon(NavigationIcon::Skills))--
            @entry(Entry::Mcp, icon(NavigationIcon::Mcp))--
            @entry(Entry::Memory, icon(NavigationIcon::Memory))
        ]
    }
}

fn icon(icon: NavigationIcon) -> impl SceneList {
    bsn_list! {
        @WidgetryIcon { @path: {icon.path()}, @max_size: {Some(UVec2::splat(NAV_ICON_SIZE))} }
        Pickable::IGNORE
    }
}

fn entry(entry: Entry, children: impl SceneList + 'static) -> impl Scene {
    let name = entry.name();
    let label = entry.label();
    (
        bsn! {
            @AgentelierButton { @style: AgentelierButtonStyle::Icon, @children: {Some(Box::new(children) as Box<dyn SceneList>)} }
            @WidgetryTooltip { @content: {TooltipContentFactory::new(move || bsn_list! { Text(label) WidgetryText })} }
            Name(name) template(move |_| Ok(entry))
            on(open)
        },
        matches!(entry, Entry::Skills).then_some(bsn! { InteractionDisabled }),
    )
}

fn open(event: On<Activate>, entries: Query<&Entry>, mut commands: Commands) -> Result {
    match entries.get(event.entity)? {
        Entry::Login => commands.queue(agentelier_login_ui::open_account_window),
        Entry::Settings => commands.queue(agentelier_login_ui::open_directory_settings),
        Entry::Skills => {}
        Entry::Mcp => commands.queue(crate::prompts::open_mcp),
        Entry::Memory => commands.queue(crate::prompts::open_memory),
    }
    Ok(())
}

fn right_column() -> impl Scene {
    bsn! {
        Name("WorkspaceRight")
        Node { min_width: px(RIGHT_MIN), flex_direction: FlexDirection::Column }
        Children [
            @WidgetryScrollArea {
                @children: {Some(Box::new(bsn_list! {
                    Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(PANE_PADDING), padding: UiRect::all(px(PANE_PADDING)) }
                    Children [
                        Text("后台任务") WidgetryText--
                        @crate::task_panel::panel()
                    ]
                }) as Box<dyn SceneList>)}
            }
            Name("WorkspaceE") BackgroundColor(PANEL_BACKGROUND)
            Node { width: percent(100), flex_basis: px(0), flex_grow: 1.0, min_height: px(0) }--
            Name("WorkspaceF") BackgroundColor(SIDE_BACKGROUND)
            Node { width: percent(100), flex_basis: px(0), flex_grow: 1.0, min_height: px(0) }
        ]
    }
}
