use crate::{Catalog, Diagnostic, Failure, Source, check};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

const REPARSE_POINT: u32 = 0x0000_0400;
const FILES: &[(&str, &[u8])] = &[
    (
        "SKILL.md",
        include_bytes!("../assets/skill-creator/SKILL.md"),
    ),
    (
        "agents/openai.yaml",
        include_bytes!("../assets/skill-creator/agents/openai.yaml"),
    ),
    (
        "assets/skill-creator-small.svg",
        include_bytes!("../assets/skill-creator/assets/skill-creator-small.svg"),
    ),
    (
        "assets/skill-creator.png",
        include_bytes!("../assets/skill-creator/assets/skill-creator.png"),
    ),
    (
        "references/openai_yaml.md",
        include_bytes!("../assets/skill-creator/references/openai_yaml.md"),
    ),
    (
        "scripts/init_skill.py",
        include_bytes!("../assets/skill-creator/scripts/init_skill.py"),
    ),
    (
        "scripts/generate_openai_yaml.py",
        include_bytes!("../assets/skill-creator/scripts/generate_openai_yaml.py"),
    ),
    (
        "scripts/quick_validate.py",
        include_bytes!("../assets/skill-creator/scripts/quick_validate.py"),
    ),
    (
        "license.txt",
        include_bytes!("../assets/skill-creator/license.txt"),
    ),
    (
        "source.md",
        include_bytes!("../assets/skill-creator/source.md"),
    ),
];

pub(crate) fn root() -> Result<PathBuf, Failure> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| Failure("缺少系统技能缓存目录 LOCALAPPDATA".into()))?;
    let mut hash = Sha256::new();
    for (path, bytes) in FILES {
        hash.update(path.as_bytes());
        hash.update(bytes);
    }
    Ok(PathBuf::from(local)
        .join("Agentelier/skills/.system")
        .join(format!("{:x}", hash.finalize())))
}

fn directory(path: &Path) -> Result<(), Failure> {
    fs::create_dir_all(path).map_err(|e| Failure(format!("{}：{e}", path.display())))?;
    let metadata = fs::symlink_metadata(path).map_err(|e| Failure(e.to_string()))?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(Failure(format!(
            "内置 Skills 缓存目录不是普通目录：{}",
            path.display()
        )));
    }
    Ok(())
}

fn verify(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let metadata = fs::symlink_metadata(path).map_err(|e| Failure(e.to_string()))?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(Failure(format!(
            "内置 Skill 材料不是普通文件：{}",
            path.display()
        )));
    }
    let mut actual = Vec::new();
    File::open(path)
        .map_err(|e| Failure(e.to_string()))?
        .take((bytes.len() + 1) as u64)
        .read_to_end(&mut actual)
        .map_err(|e| Failure(e.to_string()))?;
    if actual != bytes {
        return Err(Failure(format!(
            "内置 Skill 缓存内容已变化，原件未覆盖：{}",
            path.display()
        )));
    }
    let mut permissions = metadata.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).map_err(|e| Failure(e.to_string()))?;
    }
    Ok(())
}

fn install(root: &Path, cancel: &CancellationToken) -> Result<(), Failure> {
    let system = root
        .parent()
        .ok_or_else(|| Failure("内置 Skills 缓存没有父目录".into()))?;
    directory(system)?;
    directory(root)?;
    let skill = root.join("skill-creator");
    directory(&skill)?;
    for (relative, bytes) in FILES {
        check(cancel)?;
        let path = skill.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| Failure("内置 Skill 材料没有父目录".into()))?;
        directory(parent)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => verify(&path, bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut temporary =
                    NamedTempFile::new_in(parent).map_err(|e| Failure(e.to_string()))?;
                temporary
                    .write_all(bytes)
                    .map_err(|e| Failure(e.to_string()))?;
                temporary
                    .as_file()
                    .sync_all()
                    .map_err(|e| Failure(e.to_string()))?;
                match temporary.persist_noclobber(&path) {
                    Ok(_) => (),
                    Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(Failure(e.to_string())),
                }
                verify(&path, bytes)?;
            }
            Err(e) => return Err(Failure(e.to_string())),
        }
    }
    check(cancel)
}

pub fn discover_standard(
    directory: &Path,
    user_skills_root: &Path,
    cancel: &CancellationToken,
) -> Result<Catalog, Failure> {
    let roots = crate::standard_roots(directory, user_skills_root, cancel)?;
    let mut active = Vec::new();
    let mut diagnostics = Vec::new();
    for root in &roots {
        if matches!(root.source, Source::Builtin)
            && let Err(failure) = install(&root.path, cancel)
        {
            check(cancel)?;
            diagnostics.push(Diagnostic {
                path: root.path.clone(),
                message: format!("内置 Skills 部署失败：{failure}"),
            });
            continue;
        }
        active.push(root.clone());
    }
    let mut catalog = crate::discover(active, cancel)?;
    catalog.roots = roots;
    catalog.diagnostics.extend(diagnostics);
    Ok(catalog)
}
