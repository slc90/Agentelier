use crate::conversation::ChatState;
use crate::workspace_layout::{self, Split, WorkspaceRatios};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_login_ui::{AccountState, QuotaWindow};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_app_assets::NavigationIcon;
use bevy_widgetry::icon::WidgetryIcon;
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
pub(crate) const SESSION_INITIAL: f32 = 280.0;
pub(crate) const SESSION_MIN: f32 = 220.0;
pub(crate) const CHAT_MIN: f32 = 640.0;
pub(crate) const RIGHT_INITIAL: f32 = 360.0;
pub(crate) const RIGHT_MIN: f32 = 320.0;
const PANE_PADDING: f32 = 12.0;
pub(crate) const HANDLE_WIDTH: f32 = 8.0;
const SEPARATOR_WIDTH: f32 = 1.0;
pub(crate) const WORKSPACE_CONTENT_WIDTH: f32 =
    MAIN_WINDOW_WIDTH - FRAME_BORDERS - NAV_WIDTH - 2.0 * HANDLE_WIDTH;
const SIDE_BACKGROUND: Color = Color::srgb_u8(0x0D, 0x26, 0x1B);
const PANEL_BACKGROUND: Color = Color::srgb_u8(0x14, 0x35, 0x26);

#[derive(Component, Clone, Default)]
struct ContextStatus;

#[derive(Component, Clone, Default)]
struct QuotaStatusLabel;

#[derive(Event)]
pub struct OpenSettings;

#[derive(Component, Clone, Copy)]
enum Entry {
    Settings,
    Skills,
    Mcp,
    Memory,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Self::Settings => "WorkspaceSettings",
            Self::Skills => "WorkspaceSkills",
            Self::Mcp => "WorkspaceMcp",
            Self::Memory => "WorkspaceMemory",
        }
    }

    fn label(self) -> &'static str {
        match self {
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
    app.add_plugins(WidgetrySplitterPlugin).add_systems(
        PostUpdate,
        (sync_context, sync_quota).before(UiSystems::Content),
    );
    workspace_layout::install(app);
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
    tasks: impl Scene + 'static,
    layout: WorkspaceRatios,
) -> impl Scene {
    let session_ratio = workspace_layout::initial_ratio(layout, Split::Session);
    let chat_ratio = workspace_layout::initial_ratio(layout, Split::Chat);
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
                @split_ratio: chat_ratio,
                @handle_thickness: HANDLE_WIDTH,
                @line_thickness: SEPARATOR_WIDTH,
                @pane_a: {Some(conversation_pane)},
                @pane_b: {Some(Box::new(right_column(tasks)) as Box<dyn Scene>)},
            }
            Name("WorkspaceChatSplit")
            Node { width: percent(100), height: percent(100) }
            template(|_| Ok(Split::Chat))
            on(workspace_layout::changed)
        ]
    }) as Box<dyn Scene>;
    bsn! {
        Name("Workspace")
        Node { width: percent(100), height: percent(100), min_height: px(0), padding: UiRect::top(px(TITLE_BOTTOM_PADDING)) }
        Children [
            @navigation()--
            @WidgetrySplitter {
                @split_ratio: session_ratio,
                @handle_thickness: HANDLE_WIDTH,
                @line_thickness: SEPARATOR_WIDTH,
                @pane_a: {Some(session_pane)},
                @pane_b: {Some(remaining)},
            }
            Name("WorkspaceSessionSplit")
            Node { flex_grow: 1.0, flex_basis: px(0), height: percent(100), min_width: px(0) }
            template(|_| Ok(Split::Session))
            on(workspace_layout::changed)
        ]
    }
}

fn navigation() -> impl Scene {
    bsn! {
        Name("WorkspaceB")
        BackgroundColor(SIDE_BACKGROUND)
        Node { width: px(NAV_WIDTH), height: percent(100), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(NAV_GAP), padding: UiRect::top(px(NAV_GAP)) }
        Children [
            @agentelier_login_ui::login_entry(NAV_AVATAR_SIZE)--
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
    bsn! {
            @AgentelierButton { @style: AgentelierButtonStyle::Icon, @children: {Some(Box::new(children) as Box<dyn SceneList>)} }
            @WidgetryTooltip { @content: {TooltipContentFactory::new(move || bsn_list! { Text(label) WidgetryText })} }
            Name(name) template(move |_| Ok(entry))
            on(open)
    }
}

fn open(event: On<Activate>, entries: Query<&Entry>, mut commands: Commands) -> Result {
    match entries.get(event.entity)? {
        Entry::Settings => commands.trigger(OpenSettings),
        Entry::Skills => commands.queue(crate::skills::open),
        Entry::Mcp => commands.queue(crate::prompts::open_mcp),
        Entry::Memory => commands.queue(crate::prompts::open_memory),
    }
    Ok(())
}

fn right_column(tasks: impl Scene + 'static) -> impl Scene {
    bsn! {
        Name("WorkspaceRight")
        Node { min_width: px(RIGHT_MIN), flex_direction: FlexDirection::Column }
        Children [
            Name("WorkspaceE") BackgroundColor(PANEL_BACKGROUND)
            Node { width: percent(100), flex_basis: px(0), flex_grow: 1.0, min_height: px(0) }
            Children [
                Node { width: percent(100), height: percent(100), min_height: px(0), flex_direction: FlexDirection::Column, padding: UiRect::all(px(PANE_PADDING)), row_gap: px(PANE_PADDING) }
                Children [ Text("后台任务") WidgetryText-- @tasks ]
            ]--
            Name("WorkspaceF") BackgroundColor(SIDE_BACKGROUND)
            Node { width: percent(100), flex_basis: px(0), flex_grow: 1.0, min_height: px(0) }
        ]
    }
}
