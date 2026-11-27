#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod avatar;
mod avatar_cropper;
mod button;
mod scroll;
mod switch;
mod theme;
mod window;

pub use avatar::{
    AgentelierAvatar, AgentelierAvatarProps, AvatarCrop, AvatarPresentation, AvatarSource,
    SyncAvatars, UserAvatar,
};
pub use avatar_cropper::{AvatarCropperState, avatar_cropper};
pub use bevy_widgetry::button::WidgetryButtonLoading as AgentelierButtonLoading;
pub use button::{AgentelierButton, AgentelierButtonProps, AgentelierButtonStyle};
pub use scroll::forward_vertical_scroll;
pub use switch::agentelier_switch;
pub use theme::AGENTELIER_THEME;
pub use window::{
    FILE_DIALOG_WINDOW_CONTROLS, MANAGEMENT_WINDOW_CONTROLS, install_owned_window_close,
    padded_window_title,
};

use bevy::prelude::*;
use bevy_widgetry::button::WidgetryButtonPlugin;

pub struct AgentelierControlsPlugin;

impl Plugin for AgentelierControlsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WidgetryButtonPlugin>() {
            app.add_plugins(WidgetryButtonPlugin);
        }
        button::install(app);
        switch::install(app);
        avatar::install(app);
    }
}
