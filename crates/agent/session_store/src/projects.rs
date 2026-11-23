use agentelier_conversation::{
    ArchiveEntry, Project, ProjectCatalog, ProjectChange, ProjectId, ProjectView,
};
use agentelier_tools::WorkingDirectory;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::Path,
};
use tempfile::NamedTempFile;

const CATALOG_FILE: &str = "projects.json";
pub(super) const MEMBERSHIP_FILE: &str = "project.json";

fn read_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|failure| io::Error::new(io::ErrorKind::InvalidData, failure)),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(failure),
    }
}

fn save(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("项目记录缺少父目录"))?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value).map_err(io::Error::other)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}

fn load(root: &Path) -> io::Result<Vec<Project>> {
    let projects: Vec<Project> = read_optional(&root.join(CATALOG_FILE))?.unwrap_or_default();
    let mut ids = BTreeSet::new();
    for project in &projects {
        if project.id.is_nil()
            || !ids.insert(project.id)
            || !project.directory.is_absolute()
            || agentelier_conversation::session_title(&project.name).is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "项目目录包含无效记录",
            ));
        }
    }
    Ok(projects)
}

pub(super) fn project_directory(root: &Path, project: ProjectId) -> io::Result<std::path::PathBuf> {
    load(root)?
        .into_iter()
        .find(|candidate| candidate.id == project)
        .map(|candidate| candidate.directory)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "项目不存在"))
}

pub(super) fn save_membership(directory: &Path, project: ProjectId) -> io::Result<()> {
    save(&directory.join(MEMBERSHIP_FILE), &project)
}

pub(super) fn catalog(root: &Path, entries: &[ArchiveEntry]) -> io::Result<ProjectCatalog> {
    let projects = load(root)?;
    let mut catalog = ProjectCatalog::default();
    let known: BTreeSet<_> = projects.iter().map(|project| project.id).collect();
    for entry in entries {
        let session = entry.summary.session;
        let project = crate::attachments::session_directory(root, session)
            .and_then(|directory| read_optional::<ProjectId>(&directory.join(MEMBERSHIP_FILE)));
        match project {
            Ok(Some(project)) if known.contains(&project) => {
                catalog.membership.insert(session, project);
            }
            Ok(_) => {}
            Err(failure) => catalog
                .issues
                .push(format!("会话 {session} 的项目归属无法读取：{failure}")),
        }
    }
    catalog.projects = projects
        .into_iter()
        .map(|project| {
            let directory_error = fs::read_dir(&project.directory)
                .err()
                .map(|failure| format!("项目目录不可用：{failure}"));
            ProjectView {
                project,
                directory_error,
            }
        })
        .collect();
    Ok(catalog)
}

pub(super) fn apply(root: &Path, change: ProjectChange) -> io::Result<()> {
    let mut projects = load(root)?;
    match change {
        ProjectChange::Create { directory, name } => {
            let directory = WorkingDirectory::validate(&directory)
                .map_err(io::Error::other)?
                .path()
                .to_path_buf();
            let folder = directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("项目");
            let name = match name {
                Some(name) => {
                    agentelier_conversation::session_title(&name).map_err(io::Error::other)?
                }
                None => agentelier_conversation::session_title(folder).map_err(io::Error::other)?,
            };
            projects.push(Project {
                id: ProjectId::new_v4(),
                name,
                directory,
            });
        }
        ProjectChange::Rename { project, name } => {
            let target = projects
                .iter_mut()
                .find(|candidate| candidate.id == project)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "项目不存在"))?;
            target.name =
                agentelier_conversation::session_title(&name).map_err(io::Error::other)?;
        }
        ProjectChange::SetDirectory { project, directory } => {
            let target = projects
                .iter_mut()
                .find(|candidate| candidate.id == project)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "项目不存在"))?;
            target.directory = WorkingDirectory::validate(&directory)
                .map_err(io::Error::other)?
                .path()
                .to_path_buf();
        }
        ProjectChange::Delete { project } => {
            let count = projects.len();
            projects.retain(|candidate| candidate.id != project);
            if count == projects.len() {
                return Ok(());
            }
        }
        ProjectChange::Move { session, project } => {
            if project.is_some_and(|id| !projects.iter().any(|candidate| candidate.id == id)) {
                return Err(io::Error::new(io::ErrorKind::NotFound, "目标项目不存在"));
            }
            let directory = crate::attachments::session_directory(root, session)?;
            let archive = crate::reader::read(&directory, session)?;
            if archive.summary.deleted
                || archive
                    .initial
                    .as_ref()
                    .is_none_or(|initial| !initial.settings.kind.is_user())
            {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "会话已删除或缺少初始记录",
                ));
            }
            let path = directory.join(MEMBERSHIP_FILE);
            return match project {
                Some(project) => save_membership(&directory, project),
                None => match fs::remove_file(path) {
                    Err(failure) if failure.kind() == io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
            };
        }
    }
    save(&root.join(CATALOG_FILE), &projects)
}
