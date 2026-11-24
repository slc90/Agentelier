use bevy_widgetry_app_icon_build::{IconBuildFailure, set_exe_icon};

fn main() -> Result<(), IconBuildFailure> {
    set_exe_icon("../crates/ui/assets/src/assets/system-tray/application-icon.ico")
}
