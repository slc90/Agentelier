#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod extraction;
mod redaction;
mod store;

pub use extraction::{SourceClaim, SourceCompletion, extraction_prompt, parse_extraction};
pub use store::{Memory, MemoryClient};
