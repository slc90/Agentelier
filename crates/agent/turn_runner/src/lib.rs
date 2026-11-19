#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod access;
mod control;
mod runner;

pub use access::{RunnerEvent, TurnImage, TurnRecord};
pub use runner::run;
