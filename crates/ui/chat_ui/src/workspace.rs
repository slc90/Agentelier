use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_app_assets::NavigationIcon;
use bevy_widgetry::icon::WidgetryIcon;

pub const MAIN_WINDOW_WIDTH: f32 = 1920.0;

const NAV_GAP: f32 = 8.0;
const NAV_ICON_SIZE: u32 = 20;
const NAV_AVATAR_SIZE: f32 = 24.0;
const PANE_PADDING: f32 = 12.0;
const SESSION_SHARE: f32 = 1.0;
const CHAT_SHARE: f32 = 3.0;
const SESSION_DIVIDER_WIDTH: f32 = 1.0;
const SESSION_DIVIDER_COLOR: Color = Color::srgb_u8(0xB7, 0xCA, 0xBC);
const WORKSPACE_SURFACE: Color = Color::srgba_u8(0xF6, 0xFB, 0xF7, 0x66);

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
}

pub(crate) fn scene(chat: impl SceneList, archives: impl SceneList) -> impl Scene {
    bsn! {
        Name("Workspace") BackgroundColor(WORKSPACE_SURFACE)
        Node {
            width: percent(100), height: percent(100), min_height: px(0), display: Display::Grid,
            grid_template_columns: {vec![GridTrack::flex(SESSION_SHARE), GridTrack::px(SESSION_DIVIDER_WIDTH), GridTrack::flex(CHAT_SHARE)]},
            grid_template_rows: {vec![GridTrack::flex(1.0)]},
            border_radius: BorderRadius::bottom(px(8)),
        }
        Children [
            Name("WorkspaceC")
            Node {
                min_width: px(0), min_height: px(0),
                padding: UiRect::new(px(PANE_PADDING), px(PANE_PADDING), px(0), px(PANE_PADDING)),
                overflow: Overflow::clip(),
            }
            Children [{archives}]--
            Name("WorkspaceSessionDivider") Pickable::IGNORE BackgroundColor(SESSION_DIVIDER_COLOR)
            Node { width: px(SESSION_DIVIDER_WIDTH) }--
            Name("WorkspaceD")
            Node {
                min_width: px(0), min_height: px(0),
                overflow: Overflow::clip(),
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
    bsn! {
            @AgentelierButton { @style: AgentelierButtonStyle::Icon, @children: {Some(Box::new(children) as Box<dyn SceneList>)} }
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
