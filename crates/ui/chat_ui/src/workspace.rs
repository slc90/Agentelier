use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_app_assets::NavigationIcon;
use bevy_widgetry::icon::WidgetryIcon;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{TooltipContentFactory, WidgetryTooltip};

pub const MAIN_WINDOW_WIDTH: f32 = 1920.0;

const NAV_GAP: f32 = 8.0;
const NAV_ICON_SIZE: u32 = 20;
const NAV_AVATAR_SIZE: f32 = 24.0;
const PANE_PADDING: f32 = 12.0;
const SESSION_SHARE: f32 = 3.0;
const CHAT_SHARE: f32 = 7.0;
const SIDE_BACKGROUND: Color = Color::srgb_u8(0xED, 0xF6, 0xEE);

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

pub(crate) fn scene(chat: impl SceneList, archives: impl SceneList) -> impl Scene {
    bsn! {
        Name("Workspace")
        Node {
            width: percent(100), height: percent(100), min_height: px(0), display: Display::Grid,
            grid_template_columns: {vec![GridTrack::flex(SESSION_SHARE), GridTrack::px(1.0), GridTrack::flex(CHAT_SHARE)]},
            grid_template_rows: {vec![GridTrack::flex(1.0)]},
        }
        Children [
            Name("WorkspaceC")
            BackgroundColor(SIDE_BACKGROUND)
            Node {
                min_width: px(0), min_height: px(0),
                padding: UiRect::all(px(PANE_PADDING)), overflow: Overflow::clip(),
                border_radius: BorderRadius::bottom_left(px(8)),
            }
            Children [{archives}]--
            Name("WorkspaceSessionDivider") Pickable::IGNORE
            BackgroundColor(Color::srgb_u8(0xB7, 0xCA, 0xBC))
            Node { width: px(1), flex_shrink: 0.0 }--
            Name("WorkspaceD")
            Node {
                min_width: px(0), min_height: px(0),
                padding: UiRect::all(px(PANE_PADDING)), overflow: Overflow::clip(),
            }
            Children [{chat}]
        ]
    }
}

pub fn workspace_navigation() -> impl Scene {
    bsn! {
        Name("WorkspaceNavigation") Pickable::IGNORE
        Node { flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(NAV_GAP), padding: UiRect::horizontal(px(PANE_PADDING)) }
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
        Entry::Mcp => commands.queue(crate::mcp::open),
        Entry::Memory => commands.queue(crate::memory::open),
    }
    Ok(())
}
