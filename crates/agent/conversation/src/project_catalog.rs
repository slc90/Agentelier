use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

pub type ProjectId = uuid::Uuid;

// The Max UUID is reserved and cannot be generated as a UUID v4.
pub const STANDALONE_PROJECT_ID: ProjectId = ProjectId::from_u128(u128::MAX);

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub directory: PathBuf,
}

#[derive(Clone)]
pub struct ProjectView {
    pub project: Project,
    pub directory_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct ProjectCatalog {
    pub projects: Vec<ProjectView>,
    pub membership: BTreeMap<SessionId, ProjectId>,
    pub issues: Vec<String>,
}

pub enum ProjectChange {
    Create {
        directory: PathBuf,
        name: Option<String>,
    },
    Rename {
        project: ProjectId,
        name: String,
    },
    SetDirectory {
        project: ProjectId,
        directory: PathBuf,
    },
    Delete {
        project: ProjectId,
    },
    Move {
        session: SessionId,
        project: Option<ProjectId>,
    },
}
