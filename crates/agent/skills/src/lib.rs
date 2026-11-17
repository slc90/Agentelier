#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod builtin;
mod context;
mod discovery;
mod parser;
mod selection;
pub use selection::{SelectedSkill, mentions, select};

pub use builtin::discover_standard;
pub use discovery::{discover, standard_roots};
pub use parser::parse;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_SKILL_BYTES: usize = 256 * 1024;
pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SCAN_ENTRIES: usize = 2048;
pub const MAX_SCAN_DEPTH: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Source {
    Project { scope: PathBuf },
    User,
    Builtin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    pub path: PathBuf,
    pub discovered_path: PathBuf,
    pub source: Source,
    pub metadata: Metadata,
    pub enabled: bool,
    pub allow_implicit_invocation: bool,
    pub interface: Option<serde_json::Value>,
    pub dependencies: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub roots: Vec<Root>,
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct Failure(pub String);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

pub(crate) fn check(cancel: &tokio_util::sync::CancellationToken) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        Err(Failure("Skills 文件工作已取消".into()))
    } else {
        Ok(())
    }
}
