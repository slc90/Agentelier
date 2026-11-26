#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod builtin;
mod context;
mod discovery;
mod native;
mod parser;
mod selection;
pub use selection::{SelectedSkill, mentions, select};

pub use builtin::{
    BUILTIN_NAME, EmbeddedResource, READ_RESOURCE_TOOL, discover_standard, read_resource,
};

pub use native::{
    Initialized, InterfacePatch, MetadataUpdate, ResourceDirectory, SkillEntry, SkillRequest,
    SkillScript, SkillSnapshot, UserSkillId, UserSkills, Validation, normalize_name,
};
pub use parser::parse;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_SKILL_BYTES: usize = 256 * 1024;
pub const MAX_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SCAN_ENTRIES: usize = 2048;
pub const MAX_SCAN_DEPTH: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Source {
    #[serde(rename = "Project")]
    Archived {
        scope: PathBuf,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmbeddedSkill {
    #[serde(rename = "skill-creator")]
    SkillCreator,
}

impl EmbeddedSkill {
    pub fn name(self) -> &'static str {
        match self {
            Self::SkillCreator => BUILTIN_NAME,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkillResource {
    File {
        path: PathBuf,
        discovered_path: PathBuf,
    },
    Embedded {
        embedded: EmbeddedSkill,
    },
}

impl SkillResource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } => Some(path),
            Self::Embedded { .. } => None,
        }
    }

    pub fn embedded(&self) -> Option<EmbeddedSkill> {
        match self {
            Self::File { .. } => None,
            Self::Embedded { embedded } => Some(*embedded),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::File { path, .. } => path.display().to_string(),
            Self::Embedded { embedded } => format!("内置 {}", embedded.name()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    #[serde(flatten)]
    pub resource: SkillResource,
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

impl Catalog {
    pub fn exclude_archived(&mut self) {
        let archived: Vec<_> = self
            .roots
            .iter()
            .filter(|root| root.source != Source::User)
            .map(|root| root.path.clone())
            .collect();
        self.roots.retain(|root| root.source == Source::User);
        self.skills.retain(Skill::available);
        self.diagnostics.retain(|diagnostic| {
            !archived
                .iter()
                .any(|root| diagnostic.path.starts_with(root))
        });
    }

    pub(crate) fn diagnose_duplicates(&mut self) {
        let mut names = BTreeMap::<&str, Vec<&Skill>>::new();
        for skill in &self.skills {
            names.entry(&skill.metadata.name).or_default().push(skill);
        }
        for (name, skills) in names {
            if skills.len() > 1
                && let Some(path) = skills.iter().find_map(|skill| skill.resource.path())
            {
                self.diagnostics.push(Diagnostic {
                    path: path.to_owned(),
                    message: format!(
                        "Skill ${name} 名称重复：{}",
                        skills
                            .iter()
                            .map(|skill| skill.resource.label())
                            .collect::<Vec<_>>()
                            .join("；")
                    ),
                });
            }
        }
    }
}

impl Skill {
    pub fn available(&self) -> bool {
        matches!(
            (&self.source, &self.resource),
            (Source::User, SkillResource::File { .. })
                | (Source::Builtin, SkillResource::Embedded { .. })
        )
    }
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
