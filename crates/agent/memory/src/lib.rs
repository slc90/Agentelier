#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod classified;
mod consolidation;
mod extraction;
mod observation;
mod publication;
mod redaction;
mod reset;
mod retrieval;
mod revision;
mod store;
mod tools;

pub use consolidation::Consolidation;
pub use extraction::{Extraction, SourceClaim, SourceCompletion, extraction_prompt};
pub use retrieval::{MemoryDetail, MemoryInspection, MemorySource, PublishedSummary};
pub use store::{Memory, MemoryClient};
pub use tools::{management_registrations, registrations};
