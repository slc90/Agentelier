#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::prelude::*;

pub const PROMPT_LIBRARY_JSON: &str = include_str!("assets/prompts/library.json");

#[derive(Resource)]
pub struct AvatarAssets {
    pub default_user: Handle<Image>,
    pub assistant: Handle<Image>,
}

pub struct AppAssetPlugin;

impl Plugin for AppAssetPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "assets/avatars/user.png");
        embedded_asset!(app, "assets/avatars/assistant.png");
        let server = app.world().resource::<AssetServer>();
        let avatars = AvatarAssets {
            default_user: load_embedded_asset!(server, "assets/avatars/user.png"),
            assistant: load_embedded_asset!(server, "assets/avatars/assistant.png"),
        };
        app.insert_resource(avatars);
    }
}
