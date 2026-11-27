use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs, io,
    os::windows::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

const REPARSE_POINT: u32 = 0x0400;
const MAX_FILES: usize = 12_000;
const MAX_ENTRIES: usize = 36_000;
const MAX_DEPTH: usize = 14;

#[derive(Clone, PartialEq, Eq)]
pub struct FileScope {
    pub working_directory: PathBuf,
    pub project: Option<PathBuf>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReference {
    pub directory: PathBuf,
    pub path: PathBuf,
    created: SystemTime,
}

impl FileReference {
    pub fn display(&self) -> String {
        let path = self.path.to_string_lossy();
        if path.contains(char::is_whitespace) {
            format!("\"{path}\"")
        } else {
            path.into_owned()
        }
    }

    pub fn valid_shape(&self) -> bool {
        self.directory.is_absolute()
            && !self.path.as_os_str().is_empty()
            && self
                .path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
    }

    pub fn verify(&self, scope: &FileScope) -> io::Result<PathBuf> {
        let working_directory = fs::canonicalize(&scope.working_directory)?;
        let scan_root = scope.root(&working_directory)?;
        if !self.valid_shape() || self.directory != working_directory {
            return Err(io::Error::other(format!(
                "文件引用 {} 的工作目录已变化",
                self.display()
            )));
        }
        let path = working_directory.join(&self.path);
        let metadata = fs::symlink_metadata(&path).and_then(|metadata| {
            let resolved = fs::canonicalize(&path)?;
            if resolved == path
                && resolved.starts_with(&scan_root)
                && ordinary(&metadata)
                && metadata.is_file()
                && metadata.created()? == self.created
            {
                Ok(resolved)
            } else {
                Err(io::Error::other("文件身份或工作范围已变化"))
            }
        });
        metadata.map_err(|failure| {
            io::Error::other(format!("文件引用 {} 已失效：{failure}", self.display()))
        })
    }
}

impl FileScope {
    fn root(&self, cwd: &Path) -> io::Result<PathBuf> {
        let Some(project) = &self.project else {
            return Ok(cwd.to_owned());
        };
        let project = fs::canonicalize(project)?;
        if cwd.starts_with(&project) {
            Ok(cwd.to_owned())
        } else if project.starts_with(cwd) {
            Ok(project)
        } else {
            Err(io::Error::other("会话工作目录与所属项目没有重叠"))
        }
    }

    pub fn list(&self) -> io::Result<Vec<FileReference>> {
        let cwd = fs::canonicalize(&self.working_directory)?;
        let root = self.root(&cwd)?;
        if root.parent().is_none() {
            return Err(io::Error::other("项目文件补全需要卷根目录内的工作文件夹"));
        }
        let mut pending = VecDeque::from([(root.clone(), 0)]);
        let mut files = Vec::new();
        let mut visited = 0;
        'scan: while let Some((directory, depth)) = pending.pop_front() {
            match fs::canonicalize(&directory) {
                Ok(resolved) if resolved == directory && resolved.starts_with(&root) => {}
                Ok(_) => continue,
                Err(failure)
                    if depth > 0
                        && matches!(
                            failure.kind(),
                            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                        ) =>
                {
                    continue;
                }
                Err(failure) => return Err(failure),
            }
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(failure)
                    if depth > 0
                        && matches!(
                            failure.kind(),
                            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                        ) =>
                {
                    continue;
                }
                Err(failure) => return Err(failure),
            };
            for entry in entries {
                let entry = entry?;
                visited += 1;
                if visited > MAX_ENTRIES || files.len() >= MAX_FILES {
                    break 'scan;
                }
                if entry.file_name() == ".git" {
                    continue;
                }
                let full = entry.path();
                let metadata = match fs::symlink_metadata(&full) {
                    Ok(metadata) if ordinary(&metadata) => metadata,
                    Ok(_) => continue,
                    Err(failure)
                        if matches!(
                            failure.kind(),
                            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
                        ) =>
                    {
                        continue;
                    }
                    Err(failure) => return Err(failure),
                };
                if metadata.is_dir() && depth < MAX_DEPTH {
                    pending.push_back((full, depth + 1));
                } else if metadata.is_file()
                    && let Ok(path) = full.strip_prefix(&cwd)
                {
                    files.push(FileReference {
                        directory: cwd.clone(),
                        path: path.to_owned(),
                        created: metadata.created()?,
                    });
                }
            }
        }
        files.sort_by_cached_key(|reference| reference.path.to_string_lossy().to_lowercase());
        Ok(files)
    }
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & REPARSE_POINT == 0
}
