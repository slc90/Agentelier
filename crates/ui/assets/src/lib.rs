#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

use bevy::asset::{AssetPath, embedded_asset, embedded_path, load_embedded_asset};
use bevy::prelude::*;
use bevy::shader::Shader;

pub mod user_avatar;

pub const PROMPT_LIBRARY_JSON: &str = include_str!("assets/prompts/library.json");

#[derive(Resource)]
pub struct AvatarAssets {
    pub default_user: Handle<Image>,
    pub assistant: Handle<Image>,
}

#[derive(Resource)]
pub struct MainWindowBackground(pub Handle<Image>);

#[derive(Clone, Copy)]
pub enum NavigationIcon {
    Settings,
    Skills,
    Mcp,
    Memory,
}

impl NavigationIcon {
    pub fn path(self) -> AssetPath<'static> {
        let path = match self {
            Self::Settings => embedded_path!("assets/navigation/settings.svg"),
            Self::Skills => embedded_path!("assets/navigation/skill.svg"),
            Self::Mcp => embedded_path!("assets/navigation/mcp.svg"),
            Self::Memory => embedded_path!("assets/navigation/memory.svg"),
        };
        AssetPath::from_path_buf(path).with_source("embedded")
    }
}

#[derive(Clone, Copy)]
pub enum SidebarIcon {
    Add,
    More,
    SessionError,
}

impl SidebarIcon {
    pub fn path(self) -> AssetPath<'static> {
        let path = match self {
            Self::Add => embedded_path!("assets/sidebar/add.svg"),
            Self::More => embedded_path!("assets/sidebar/more.svg"),
            Self::SessionError => embedded_path!("assets/sidebar/session-error.svg"),
        };
        AssetPath::from_path_buf(path).with_source("embedded")
    }
}

pub fn window_alpha_shader(server: &AssetServer) -> Handle<Shader> {
    load_embedded_asset!(server, "assets/shaders/window_alpha.wesl")
}

pub struct AppAssetPlugin;

impl Plugin for AppAssetPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "assets/avatars/user.png");
        embedded_asset!(app, "assets/avatars/assistant.png");
        embedded_asset!(app, "assets/backgrounds/main-window-background.png");
        embedded_asset!(app, "assets/navigation/settings.svg");
        embedded_asset!(app, "assets/navigation/skill.svg");
        embedded_asset!(app, "assets/navigation/mcp.svg");
        embedded_asset!(app, "assets/navigation/memory.svg");
        embedded_asset!(app, "assets/sidebar/add.svg");
        embedded_asset!(app, "assets/sidebar/more.svg");
        embedded_asset!(app, "assets/sidebar/session-error.svg");
        embedded_asset!(app, "assets/shaders/window_alpha.wesl");
        let server = app.world().resource::<AssetServer>();
        let avatars = AvatarAssets {
            default_user: load_embedded_asset!(server, "assets/avatars/user.png"),
            assistant: load_embedded_asset!(server, "assets/avatars/assistant.png"),
        };
        let background = MainWindowBackground(load_embedded_asset!(
            server,
            "assets/backgrounds/main-window-background.png"
        ));
        app.insert_resource(avatars).insert_resource(background);
    }
}
