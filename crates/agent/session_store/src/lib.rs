#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod attachments;
mod audio;
mod composer;
mod deletion;
mod delta;
mod journal;
mod projects;
mod reader;
mod store;

pub use audio::{AudioExtrema, summarize_audio};
pub use store::{Commit, MetadataCommit, Publication, SessionStore, SessionStoreClient};
use store::{FORMAT, FORMAT_VERSION, Record, RecordKind};

pub use reader::{MemoryCutoff, MemorySourceSnapshot};
pub use store::{ArchiveCatalog, ArchivedTasks, DeleteCommit};
