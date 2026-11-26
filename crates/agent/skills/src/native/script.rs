use super::{REPARSE_POINT, UserSkills, management, optional_metadata};
use crate::{
    Failure, MAX_SCAN_DEPTH, check,
    discovery::{read_open_file, read_open_frontmatter},
    parse,
};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillScript {
    pub directory: String,
    #[serde(default = "default_entry")]
    pub entry: String,
}

fn default_entry() -> String {
    "scripts/widget.rhai".into()
}

impl std::fmt::Display for SkillScript {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "{}/{}", self.directory, self.entry)
    }
}

impl UserSkills {
    pub fn read_script(
        &self,
        source: &SkillScript,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, Failure> {
        let directory = relative_parts(&source.directory)?;
        let entry = relative_parts(&source.entry)?;
        if directory.len() > MAX_SCAN_DEPTH
            || directory.iter().any(|part| part.starts_with('.'))
            || !Path::new(&source.entry)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rhai"))
        {
            return Err(Failure("Skill 来源目录或 Rhai 入口无效".into()));
        }
        let _guard = self.guard()?;
        check(cancel)?;
        if !self.root.is_absolute() {
            return Err(Failure("用户 Skill 根必须是绝对路径".into()));
        }
        let mut pins = Vec::new();
        let mut path = self.root.clone();
        if !pin_directory(&mut pins, &path)? || !descend(&mut pins, &mut path, &directory)? {
            return Ok(None);
        }
        let Some(instructions) = open_file(&path.join("SKILL.md"))? else {
            return Ok(None);
        };
        let mut bytes_used = 0;
        let metadata = parse(&read_open_frontmatter(instructions, &mut bytes_used)?)?;
        if Some(metadata.name.as_str()) != directory.last().copied() {
            return Err(Failure("Skill name 与目录名称不一致".into()));
        }
        let (filename, parents) = entry
            .split_last()
            .ok_or_else(|| Failure("Skill 缺少 Rhai 入口文件".into()))?;
        if !descend(&mut pins, &mut path, parents)? {
            return Ok(None);
        }
        let script = open_file(&path.join(filename))?
            .map(|file| read_open_file(file, limit))
            .transpose()?;
        check(cancel)?;
        Ok(script)
    }
}

fn relative_parts(path: &str) -> Result<Vec<&str>, Failure> {
    let parts = path.split('/').collect::<Vec<_>>();
    if path.len() > 256
        || path.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
        || path.chars().any(char::is_control)
        || parts
            .iter()
            .any(|part| part.is_empty() || part.ends_with(['.', ' ']))
    {
        return Err(Failure("Skill 目录与脚本入口必须使用普通相对路径".into()));
    }
    Ok(parts)
}

fn descend(pins: &mut Vec<File>, path: &mut PathBuf, parts: &[&str]) -> Result<bool, Failure> {
    for part in parts {
        path.push(part);
        if !pin_directory(pins, path)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn pin_directory(pins: &mut Vec<File>, path: &Path) -> Result<bool, Failure> {
    if optional_metadata(path)?.is_none() {
        return Ok(false);
    }
    pins.push(management::pin(path)?);
    Ok(true)
}

fn open_file(path: &Path) -> Result<Option<File>, Failure> {
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Failure(format!("{}：{error}", path.display()))),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(Failure(format!(
            "Skill 资源必须是普通文件：{}",
            path.display()
        )));
    }
    Ok(Some(file))
}
