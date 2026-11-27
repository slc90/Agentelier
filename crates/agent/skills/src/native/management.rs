use super::{
    UserSkills, normalize_name, optional_metadata, publish_directory, require_absent, validation,
};
use crate::{BUILTIN_NAME, Failure, Metadata, SkillResource, check, discover_standard};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use tokio_util::sync::CancellationToken;

const IMPORT_BYTES: u64 = 64 * 1024 * 1024;
const DIRECTORY_FLAGS: u32 = 0x0200_0000 | 0x0020_0000;
const SHARE_READ_WRITE: u32 = 0x0000_0003;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSkillId(PathBuf);

#[derive(Clone, Debug)]
pub enum SkillRequest {
    List,
    Import(PathBuf),
    Delete(UserSkillId),
}

pub struct SkillSnapshot {
    pub entries: Vec<SkillEntry>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone)]
pub struct SkillEntry {
    pub identity: crate::SkillIdentity,
    pub enabled: bool,
    pub metadata: Metadata,
    pub user: Option<UserSkillId>,
}

impl UserSkills {
    pub fn manage(
        &self,
        request: SkillRequest,
        cancel: &CancellationToken,
    ) -> Result<SkillSnapshot, Failure> {
        let _operation = self.guard()?;
        check(cancel)?;
        match request {
            SkillRequest::List => {}
            SkillRequest::Import(source) => self.import(&source, cancel)?,
            SkillRequest::Delete(id) => self.remove(&id, cancel)?,
        }
        self.snapshot(cancel)
    }

    fn snapshot(&self, cancel: &CancellationToken) -> Result<SkillSnapshot, Failure> {
        let catalog = discover_standard(&self.root, true, cancel)?;
        let entries = catalog
            .skills
            .into_iter()
            .filter_map(|skill| {
                let identity = crate::SkillIdentity::from(&skill.resource);
                let enabled = skill.available() && skill.enabled;
                let user = match skill.resource {
                    SkillResource::Embedded { .. } => None,
                    SkillResource::File {
                        discovered_path, ..
                    } => {
                        let relative = discovered_path.parent()?.strip_prefix(&self.root).ok()?;
                        Some(UserSkillId(relative.to_owned()))
                    }
                };
                Some(SkillEntry {
                    metadata: skill.metadata,
                    user,
                    identity,
                    enabled,
                })
            })
            .collect();
        let diagnostics = catalog
            .diagnostics
            .into_iter()
            .map(|item| {
                let path = item.path.strip_prefix(&self.root).unwrap_or(&item.path);
                format!("{}：{}", path.display(), item.message)
            })
            .collect();
        Ok(SkillSnapshot {
            entries,
            diagnostics,
        })
    }

    fn import(&self, source: &Path, cancel: &CancellationToken) -> Result<(), Failure> {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Failure("请选择一个有规范名称的 Skill 文件夹".into()))?;
        if normalize_name(name)? != name || name == BUILTIN_NAME {
            return Err(Failure("Skill 目录名称不规范或是内置保留名称".into()));
        }
        let _source_parents = pin_ancestors(source)?;
        let _source = pin(source)?;
        let _root_parents = pin_ancestors(&self.root)?;
        let root = self.root(true)?;
        let _root = pin(&root)?;
        let target = root.join(name);
        require_absent(&target)?;
        let current = self.snapshot(cancel)?;
        if current
            .entries
            .iter()
            .any(|skill| skill.metadata.name == name)
        {
            return Err(Failure(format!("已存在同名 Skill：{name}")));
        }
        validation::directory(source, name, cancel)?;
        let stage = tempfile::Builder::new()
            .prefix(".skill-import-")
            .tempdir_in(&root)?;
        let mut budget = CopyBudget {
            entries: crate::MAX_SCAN_ENTRIES,
            bytes: IMPORT_BYTES,
        };
        copy_tree(source, stage.path(), 0, &mut budget, cancel)?;
        validation::directory(stage.path(), name, cancel)?;
        check(cancel)?;
        publish_directory(stage.path(), &target)
    }

    fn remove(&self, id: &UserSkillId, cancel: &CancellationToken) -> Result<(), Failure> {
        let _root_parents = pin_ancestors(&self.root)?;
        let root = self.root(false)?;
        let mut target = root.clone();
        let parts: Vec<_> = id.0.components().collect();
        let Some((last, parents)) = parts.split_last() else {
            return Err(Failure("不能删除用户 Skill 根目录".into()));
        };
        let mut pins = vec![pin(&root)?];
        for part in parents {
            target.push(normal_component(*part)?);
            pins.push(pin(&target)?);
        }
        target.push(normal_component(*last)?);
        let selected = self
            .snapshot(cancel)?
            .entries
            .into_iter()
            .any(|item| item.user.as_ref() == Some(id));
        if !selected {
            return Err(Failure("Skill 已变更，请重新打开目录列表".into()));
        }
        let temporary = tempfile::Builder::new()
            .prefix(".skill-delete-")
            .tempdir_in(&root)?;
        let quarantined = temporary.path().join("entry");
        {
            let _target = pin(&target)?;
            check(cancel)?;
        }
        publish_directory(&target, &quarantined)?;
        if let Err(failure) = fs::remove_dir_all(&quarantined) {
            if publish_directory(&quarantined, &target).is_err() {
                let preserved = temporary.keep();
                return Err(Failure(format!(
                    "Skill 删除中断，剩余文件保留在 {}：{failure}",
                    preserved.display()
                )));
            }
            return Err(Failure(format!(
                "Skill 删除中断，剩余文件已移回：{failure}"
            )));
        }
        Ok(())
    }
}

fn normal_component(part: Component<'_>) -> Result<&std::ffi::OsStr, Failure> {
    match part {
        Component::Normal(part) if !part.to_string_lossy().starts_with('.') => Ok(part),
        _ => Err(Failure("Skill 目录超出用户 Skill 根目录".into())),
    }
}

pub(super) fn pin(path: &Path) -> Result<File, Failure> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .custom_flags(DIRECTORY_FLAGS)
        .open(path)
        .map_err(|failure| Failure(format!("无法打开 Skill 目录 {}：{failure}", path.display())))?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & super::REPARSE_POINT != 0 {
        return Err(Failure(format!(
            "Skill 目录必须是普通目录：{}",
            path.display()
        )));
    }
    Ok(file)
}

fn pin_ancestors(path: &Path) -> Result<Vec<File>, Failure> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(Failure("请选择正常的绝对目录".into()));
    }
    let mut ancestors = path
        .ancestors()
        .skip(1)
        .filter(|path| path.has_root())
        .collect::<Vec<_>>();
    ancestors.reverse();
    ancestors.into_iter().map(pin).collect()
}

struct CopyBudget {
    entries: usize,
    bytes: u64,
}

fn copy_tree(
    source: &Path,
    target: &Path,
    depth: usize,
    budget: &mut CopyBudget,
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    if depth > crate::MAX_SCAN_DEPTH {
        return Err(Failure("Skill 资源目录超过最大深度".into()));
    }
    for child in fs::read_dir(source)? {
        check(cancel)?;
        budget.entries = budget
            .entries
            .checked_sub(1)
            .ok_or_else(|| Failure("Skill 导入超过 2048 个目录项".into()))?;
        let child = child?;
        let source = child.path();
        let target = target.join(child.file_name());
        let metadata = optional_metadata(&source)?
            .ok_or_else(|| Failure("导入期间 Skill 资源已变更".into()))?;
        if metadata.is_dir() {
            let _directory = pin(&source)?;
            fs::create_dir(&target)?;
            copy_tree(&source, &target, depth + 1, budget, cancel)?;
        } else if metadata.is_file() {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(0x0020_0000)
                .open(&source)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.file_attributes() & super::REPARSE_POINT != 0 {
                return Err(Failure("Skill 资源必须是普通文件".into()));
            }
            let mut output = File::create_new(&target)?;
            let copied = io::copy(&mut file.take(budget.bytes + 1), &mut output)?;
            budget.bytes = budget
                .bytes
                .checked_sub(copied)
                .ok_or_else(|| Failure("Skill 导入超过 64 MiB".into()))?;
            output.sync_all()?;
        } else {
            return Err(Failure("Skill 资源必须是文件或目录".into()));
        }
    }
    Ok(())
}
