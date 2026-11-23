mod management;
mod metadata;
mod validation;

use crate::{Failure, MAX_SKILL_BYTES, Metadata, check, discovery::read_file, parse};
pub use management::{SkillEntry, SkillRequest, SkillSnapshot, UserSkillId};
pub use metadata::{InterfacePatch, MetadataUpdate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tempfile::Builder;
use tokio_util::sync::CancellationToken;

pub(crate) const REPARSE_POINT: u32 = 0x0000_0400;
const STARTER: &str = "<!-- agentelier:skill-starter -->";

#[derive(Clone)]
pub struct UserSkills {
    root: PathBuf,
    writes: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDirectory {
    References,
    Scripts,
    Assets,
}

impl ResourceDirectory {
    fn name(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Scripts => "scripts",
            Self::Assets => "assets",
        }
    }
}

#[derive(Serialize)]
pub struct Initialized {
    pub name: String,
    pub directory: PathBuf,
}

#[derive(Serialize)]
pub struct Validation {
    pub metadata: Metadata,
    pub references_checked: usize,
    pub metadata_sha256: Option<String>,
}

impl UserSkills {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            writes: Arc::new(Mutex::new(())),
        }
    }

    fn guard(&self) -> Result<MutexGuard<'_, ()>, Failure> {
        self.writes
            .lock()
            .map_err(|_| Failure("用户 Skill 写入状态无法使用".into()))
    }

    pub fn initialize(
        &self,
        name: &str,
        description: &str,
        resources: &[ResourceDirectory],
        cancel: &CancellationToken,
    ) -> Result<Initialized, Failure> {
        let name = normalize_name(name)?;
        if name == crate::BUILTIN_NAME {
            return Err(Failure("skill-creator 是应用内置的保留名称".into()));
        }
        let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
        let yaml = serde_yaml::to_string(&Metadata {
            name: name.clone(),
            description,
        })
        .map_err(|failure| Failure(failure.to_string()))?;
        let starter = format!("---\n{yaml}---\n\n# {name}\n\n{STARTER}\n");
        parse(&starter)?;
        let _write = self.guard()?;
        check(cancel)?;
        let root = self.root(true)?;
        let target = root.join(&name);
        require_absent(&target)?;
        let stage = Builder::new().prefix(".skill-init-").tempdir_in(&root)?;
        let mut instructions = File::create_new(stage.path().join("SKILL.md"))?;
        instructions.write_all(starter.as_bytes())?;
        instructions.sync_all()?;
        drop(instructions);
        for resource in resources
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
        {
            check(cancel)?;
            fs::create_dir(stage.path().join(resource.name()))?;
        }
        check(cancel)?;
        publish_directory(stage.path(), &target)?;
        Ok(Initialized {
            name,
            directory: target,
        })
    }

    pub fn validate(&self, name: &str, cancel: &CancellationToken) -> Result<Validation, Failure> {
        let _read = self.guard()?;
        let root = self.existing(name)?;
        validation::directory(&root, name, cancel)
    }

    pub fn write_metadata(
        &self,
        name: &str,
        update: MetadataUpdate,
        cancel: &CancellationToken,
    ) -> Result<String, Failure> {
        let _write = self.guard()?;
        let root = self.existing(name)?;
        let file = root.join("SKILL.md");
        require_file(&file)?;
        let parsed = parse(&read_file(&file, MAX_SKILL_BYTES)?)?;
        if parsed.name != name {
            return Err(Failure(format!(
                "目录 {name} 与 frontmatter 名称 {} 不一致",
                parsed.name
            )));
        }
        metadata::write(&root, name, update, cancel)
    }

    fn root(&self, create: bool) -> Result<PathBuf, Failure> {
        if !self.root.is_absolute() {
            return Err(Failure("用户 Skill 根必须是绝对路径".into()));
        }
        if create {
            match fs::create_dir(&self.root) {
                Ok(()) => {}
                Err(failure) if failure.kind() == io::ErrorKind::AlreadyExists => {}
                Err(failure) => return Err(failure.into()),
            }
        }
        require_directory(&self.root)?;
        Ok(self.root.canonicalize()?)
    }

    fn existing(&self, name: &str) -> Result<PathBuf, Failure> {
        if normalize_name(name)? != name {
            return Err(Failure("请输入现有 Skill 的规范目录名称".into()));
        }
        let root = self.root(false)?;
        let path = root.join(name);
        require_directory(&path)?;
        Ok(path)
    }
}

pub fn normalize_name(raw: &str) -> Result<String, Failure> {
    let mut name = String::new();
    let mut separator = false;
    for ch in raw.trim().chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            separator = !name.is_empty();
            continue;
        }
        if !ch.is_alphabetic() && !ch.is_numeric() {
            return Err(Failure("Skill 名称包含非法字符".into()));
        }
        if separator {
            name.push('-');
            separator = false;
        }
        name.extend(ch.to_lowercase());
    }
    if name.is_empty()
        || name.chars().count() > 64
        || !name
            .chars()
            .all(|ch| ch.is_lowercase() || ch.is_numeric() || ch == '-')
    {
        return Err(Failure(
            "Skill 名称需要 1–64 个小写字母、数字或连字符".into(),
        ));
    }
    let device = name
        .strip_prefix("com")
        .or_else(|| name.strip_prefix("lpt"));
    if matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
        || device.is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err(Failure("Skill 名称是 Windows 保留设备名".into()));
    }
    Ok(name)
}

fn path_text(path: &Path) -> Result<&str, Failure> {
    path.to_str()
        .ok_or_else(|| Failure("Skill 目录路径无法表示为 Unicode".into()))
}

fn publish_directory(stage: &Path, target: &Path) -> Result<(), Failure> {
    winsafe::MoveFile(path_text(stage)?, path_text(target)?).map_err(|failure| {
        Failure(format!(
            "无法发布 Skill 目录 {}：{failure}",
            target.display()
        ))
    })
}

fn require_absent(path: &Path) -> Result<(), Failure> {
    if optional_metadata(path)?.is_some() {
        return Err(Failure(format!("目标已存在：{}", path.display())));
    }
    Ok(())
}

fn optional_metadata(path: &Path) -> Result<Option<fs::Metadata>, Failure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_attributes() & REPARSE_POINT != 0 => Err(Failure(format!(
            "Skill 路径不允许使用链接或重解析点：{}",
            path.display()
        ))),
        Ok(metadata) => Ok(Some(metadata)),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(Failure(format!("{}：{failure}", path.display()))),
    }
}

fn require_directory(path: &Path) -> Result<(), Failure> {
    if !optional_metadata(path)?.is_some_and(|metadata| metadata.is_dir()) {
        return Err(Failure(format!(
            "Skill 目录不存在或类型错误：{}",
            path.display()
        )));
    }
    Ok(())
}

fn require_file(path: &Path) -> Result<(), Failure> {
    if !optional_metadata(path)?.is_some_and(|metadata| metadata.is_file()) {
        return Err(Failure(format!(
            "Skill 文件不存在或类型错误：{}",
            path.display()
        )));
    }
    Ok(())
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl From<io::Error> for Failure {
    fn from(failure: io::Error) -> Self {
        Self(failure.to_string())
    }
}
