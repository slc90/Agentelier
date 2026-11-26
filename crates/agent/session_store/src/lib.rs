#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod attachments;
mod audio;
mod deletion;
mod journal;
mod projects;
mod reader;
mod store;

pub use audio::{AudioExtrema, summarize_audio};
pub use store::{
    Commit, FORMAT, FORMAT_VERSION, MetadataCommit, Record, RecordKind, SessionStore,
    SessionStoreClient,
};

pub use reader::{MemoryCutoff, MemorySourceSnapshot};
pub use store::{ArchiveCatalog, ArchivedTasks, DeleteCommit};
