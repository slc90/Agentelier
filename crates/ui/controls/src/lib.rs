#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod avatar;
mod avatar_cropper;
mod button;
mod theme;

pub use avatar::{
    AgentelierAvatar, AgentelierAvatarProps, AvatarCrop, AvatarPresentation, AvatarSource,
    SyncAvatars, UserAvatar,
};
pub use avatar_cropper::{AvatarCropper, AvatarCropperProps, AvatarCropperState};
pub use bevy_widgetry::button::WidgetryButtonLoading as AgentelierButtonLoading;
pub use button::{AgentelierButton, AgentelierButtonProps, AgentelierButtonStyle};
pub use theme::AGENTELIER_THEME;

use bevy::prelude::*;
use bevy_widgetry::button::WidgetryButtonPlugin;

pub struct AgentelierControlsPlugin;

impl Plugin for AgentelierControlsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WidgetryButtonPlugin>() {
            app.add_plugins(WidgetryButtonPlugin);
        }
        button::install(app);
        avatar::install(app);
    }
}
