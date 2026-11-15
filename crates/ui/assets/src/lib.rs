#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

use bevy::prelude::*;

pub const PROMPT_LIBRARY_JSON: &str = include_str!("assets/prompts/library.json");

pub struct AppAssetPlugin;

impl Plugin for AppAssetPlugin {
    fn build(&self, _app: &mut App) {}
}
