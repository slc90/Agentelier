#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod connection;

pub use connection::{BridgeEvent, BridgeEventKind, BridgeFailure, Connection};
