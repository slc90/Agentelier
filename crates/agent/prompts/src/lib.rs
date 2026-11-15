#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod library;
mod snapshot;

pub use library::{Library, PromptKind, PromptPreset, PromptRef, PromptRevision};
pub use snapshot::{
    BaseInstruction, InstructionConfig, InstructionMessage, InstructionSlot, InstructionSnapshot,
    ResolvedInstruction, content_hash,
};
