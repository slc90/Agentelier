#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod consolidation;
mod extraction;
mod publication;
mod redaction;
mod retrieval;
mod revision;
mod store;
mod tools;

pub use consolidation::Consolidation;
pub use extraction::{SourceClaim, SourceCompletion, extraction_prompt, parse_extraction};
pub use store::{Memory, MemoryClient};
pub use tools::registrations;
