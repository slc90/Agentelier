use crate::snapshot::{ProjectSnapshot, RuleSource, hash};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub const MAX_PROJECT_SOURCE_BYTES: usize = 32 * 1024;

#[derive(Debug)]
pub enum ProjectContextFailure {
    Cancelled,
    Io {
        path: PathBuf,
        source: io::Error,
    },
    NotDirectory(PathBuf),
    NotFile(PathBuf),
    InvalidGitMarker(PathBuf),
    InvalidUtf8 {
        path: PathBuf,
        source: std::str::Utf8Error,
    },
    Budget {
        path: PathBuf,
        used: usize,
    },
}

impl std::fmt::Display for ProjectContextFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("项目规则采集已取消"),
            Self::Io { path, source } => {
                write!(f, "项目规则路径 {} 读取失败：{source}", path.display())
            }
            Self::NotDirectory(path) => write!(f, "会话工作目录不可用：{}", path.display()),
            Self::NotFile(path) => write!(f, "项目规则不是普通文件：{}", path.display()),
            Self::InvalidGitMarker(path) => {
                write!(f, "Git 边界标记不是文件或目录：{}", path.display())
            }
            Self::InvalidUtf8 { path, .. } => {
                write!(f, "项目规则不是有效 UTF-8：{}", path.display())
            }
            Self::Budget { path, used } => write!(
                f,
                "项目规则超过字节预算：{}；已读取至少 {used} 字节，预算 {MAX_PROJECT_SOURCE_BYTES} 字节",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ProjectContextFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidUtf8 { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_failure(path: &Path, source: io::Error) -> ProjectContextFailure {
    ProjectContextFailure::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn check(cancel: &CancellationToken) -> Result<(), ProjectContextFailure> {
    if cancel.is_cancelled() {
        Err(ProjectContextFailure::Cancelled)
    } else {
        Ok(())
    }
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>, ProjectContextFailure> {
    match fs::symlink_metadata(path) {
        Ok(entry) if entry.is_symlink() => fs::metadata(path)
            .map(Some)
            .map_err(|error| io_failure(path, error)),
        Ok(entry) => Ok(Some(entry)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(path, error)),
    }
}

pub fn collect(
    directory: &Path,
    cancel: &CancellationToken,
) -> Result<ProjectSnapshot, ProjectContextFailure> {
    check(cancel)?;
    if !directory.is_absolute() {
        return Err(ProjectContextFailure::NotDirectory(directory.to_path_buf()));
    }
    let working_directory = directory
        .canonicalize()
        .map_err(|error| io_failure(directory, error))?;
    if !fs::metadata(&working_directory)
        .map_err(|error| io_failure(&working_directory, error))?
        .is_dir()
    {
        return Err(ProjectContextFailure::NotDirectory(working_directory));
    }
    let mut project_root = None;
    let mut chain = Vec::new();
    for ancestor in working_directory.ancestors() {
        check(cancel)?;
        chain.push(ancestor.to_path_buf());
        let marker = ancestor.join(".git");
        if let Some(entry) = metadata(&marker)? {
            if !entry.is_file() && !entry.is_dir() {
                return Err(ProjectContextFailure::InvalidGitMarker(marker));
            }
            project_root = Some(ancestor.to_path_buf());
            break;
        }
    }
    if project_root.is_none() {
        chain.truncate(1);
    }
    chain.reverse();
    let mut snapshot = ProjectSnapshot {
        working_directory,
        project_root,
        sources: Vec::new(),
        source_bytes: 0,
        rendered: String::new(),
        hash: String::new(),
    };
    for scope in chain {
        check(cancel)?;
        let path = scope.join("AGENTS.md");
        let Some(entry) = metadata(&path)? else {
            continue;
        };
        if !entry.is_file() {
            return Err(ProjectContextFailure::NotFile(path));
        }
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A disappeared regular file is absent; a dangling link is a read error.
                if metadata(&path)?.is_none() {
                    continue;
                }
                return Err(io_failure(&path, error));
            }
            Err(error) => return Err(io_failure(&path, error)),
        };
        if !file
            .metadata()
            .map_err(|error| io_failure(&path, error))?
            .is_file()
        {
            return Err(ProjectContextFailure::NotFile(path));
        }
        let remaining = MAX_PROJECT_SOURCE_BYTES - snapshot.source_bytes;
        let mut bytes = Vec::new();
        file.take((remaining + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| io_failure(&path, error))?;
        check(cancel)?;
        snapshot.source_bytes += bytes.len();
        if snapshot.source_bytes > MAX_PROJECT_SOURCE_BYTES {
            return Err(ProjectContextFailure::Budget {
                path,
                used: snapshot.source_bytes,
            });
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|source| ProjectContextFailure::InvalidUtf8 {
                path: path.clone(),
                source,
            })?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
        snapshot.sources.push(RuleSource {
            hash: hash(&text),
            path,
            scope,
            text,
        });
    }
    check(cancel)?;
    snapshot.render();
    Ok(snapshot)
}
