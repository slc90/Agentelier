use crate::{Catalog, Diagnostic, Failure, Root, Skill, SkillResource, Source, check, parse};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use tokio_util::sync::CancellationToken;

const MAX_DIRECTORIES_PER_ROOT: usize = 2_000;
const MAX_ENTRIES_PER_ROOT: usize = 20_000;

pub(crate) fn read_file(path: &Path, limit: usize) -> Result<String, Failure> {
    let file = File::open(path).map_err(|e| Failure(format!("{}：{e}", path.display())))?;
    read_open_file(file, limit)
}

pub(crate) fn read_open_file(file: File, limit: usize) -> Result<String, Failure> {
    if !file
        .metadata()
        .map_err(|e| Failure(e.to_string()))?
        .is_file()
    {
        return Err(Failure("Skill 材料不是普通文件".into()));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure(e.to_string()))?;
    if bytes.len() > limit {
        return Err(Failure("Skill 材料超过字节预算".into()));
    }
    String::from_utf8(bytes).map_err(|e| Failure(format!("Skill 材料不是 UTF-8：{e}")))
}

fn read_frontmatter(path: &Path) -> Result<String, Failure> {
    let file = File::open(path).map_err(|e| Failure(e.to_string()))?;
    read_open_frontmatter(file)
}

pub(crate) fn read_open_frontmatter(file: File) -> Result<String, Failure> {
    let metadata = file.metadata().map_err(|e| Failure(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > crate::MAX_SKILL_BYTES as u64 {
        return Err(Failure("SKILL.md 不是普通文件或超过 256 KiB".into()));
    }
    let mut reader = BufReader::new(file.take((crate::MAX_METADATA_BYTES + 1) as u64));
    let mut yaml = String::new();
    let mut delimiters = 0;
    loop {
        let count = reader
            .read_line(&mut yaml)
            .map_err(|e| Failure(e.to_string()))?;
        if yaml.len() > crate::MAX_METADATA_BYTES {
            return Err(Failure("Skill frontmatter 超过 64 KiB".into()));
        }
        if count == 0 {
            break;
        }
        if yaml
            .lines()
            .next_back()
            .is_some_and(|line| line.trim_start_matches('\u{feff}') == "---")
        {
            delimiters += 1;
            if delimiters == 2 {
                break;
            }
        }
        if delimiters == 0 {
            break;
        }
    }
    Ok(yaml)
}

#[derive(Default, Deserialize)]
pub(crate) struct Extension {
    pub(crate) interface: Option<serde_json::Value>,
    pub(crate) dependencies: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) policy: Policy,
}
#[derive(Deserialize)]
pub(crate) struct Policy {
    #[serde(default = "implicit_default")]
    pub(crate) allow_implicit_invocation: bool,
}
pub(crate) fn extension(text: &str) -> Result<Extension, Failure> {
    serde_yaml::from_str(text).map_err(|error| Failure(error.to_string()))
}

fn implicit_default() -> bool {
    true
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            allow_implicit_invocation: true,
        }
    }
}

fn skill(path: &Path, source: &Source) -> Result<(Skill, Option<Diagnostic>), Failure> {
    let yaml = read_frontmatter(path)?;
    let metadata = parse(&yaml)?;
    if path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        != Some(&metadata.name)
    {
        return Err(Failure("Skill name 与目录名称不一致".into()));
    }
    let canonical = path.canonicalize().map_err(|e| Failure(e.to_string()))?;
    let extra_path = canonical
        .parent()
        .ok_or_else(|| Failure("Skill 缺少父目录".into()))?
        .join("agents/openai.yaml");
    let (extension, warning) = match read_extension(&extra_path) {
        Ok(extra) => (extra, None),
        Err(error) => (
            Extension {
                policy: Policy {
                    allow_implicit_invocation: false,
                },
                ..Extension::default()
            },
            Some(Diagnostic {
                path: extra_path,
                message: format!("可选元数据无效，仅允许显式选择：{error}"),
            }),
        ),
    };
    Ok((
        Skill {
            resource: SkillResource::File {
                path: canonical,
                discovered_path: path.to_owned(),
            },
            source: source.clone(),
            metadata,
            enabled: true,
            allow_implicit_invocation: extension.policy.allow_implicit_invocation,
            interface: extension.interface,
            dependencies: extension.dependencies,
        },
        warning,
    ))
}

fn read_extension(path: &Path) -> Result<Extension, Failure> {
    let parent = path
        .parent()
        .ok_or_else(|| Failure("Skill 扩展元数据缺少父目录".into()))?;
    for part in [parent, path] {
        match fs::symlink_metadata(part) {
            Ok(metadata) if metadata.file_attributes() & crate::native::REPARSE_POINT != 0 => {
                return Err(Failure("Skill 扩展元数据不能经过重解析点".into()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Extension::default());
            }
            Err(e) => return Err(e.into()),
        }
    }
    let text = read_file(path, crate::MAX_METADATA_BYTES)?;
    extension(&text)
}

pub(crate) fn discover(roots: Vec<Root>, cancel: &CancellationToken) -> Result<Catalog, Failure> {
    let mut catalog = Catalog {
        roots,
        ..Catalog::default()
    };
    let mut visited = BTreeSet::new();
    let mut skills = BTreeSet::new();
    for root in catalog.roots.clone() {
        let mut pending = vec![(root.path.clone(), 0)];
        let mut directories_scanned = 0;
        let mut entries_left = MAX_ENTRIES_PER_ROOT;
        let mut entries_truncated = false;
        while let Some((directory, depth)) = pending.pop() {
            check(cancel)?;
            let canonical = match fs::symlink_metadata(&directory).and_then(|metadata| {
                if metadata.file_attributes() & crate::native::REPARSE_POINT != 0 {
                    return Err(std::io::Error::other("Skill 目录不能是链接或重解析点"));
                }
                directory.canonicalize()
            }) {
                Ok(path) => path,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && depth == 0 => {
                    if fs::symlink_metadata(&directory)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    {
                        continue;
                    }
                    catalog.diagnostics.push(Diagnostic {
                        path: directory,
                        message: e.to_string(),
                    });
                    continue;
                }
                Err(e) => {
                    catalog.diagnostics.push(Diagnostic {
                        path: directory,
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            if visited.contains(&canonical) {
                continue;
            }
            if directories_scanned == MAX_DIRECTORIES_PER_ROOT {
                catalog.diagnostics.push(Diagnostic {
                    path: root.path.clone(),
                    message: format!(
                        "Skills 扫描达到 {MAX_DIRECTORIES_PER_ROOT} 个目录，已保留发现结果"
                    ),
                });
                break;
            }
            visited.insert(canonical);
            directories_scanned += 1;
            let candidate = directory.join("SKILL.md");
            match fs::symlink_metadata(&candidate) {
                Ok(metadata) => {
                    if metadata.file_attributes() & crate::native::REPARSE_POINT != 0 {
                        catalog.diagnostics.push(Diagnostic {
                            path: candidate,
                            message: "SKILL.md 不能是链接或重解析点".into(),
                        });
                        continue;
                    }
                    match skill(&candidate, &root.source) {
                        Ok((skill, warning)) => {
                            if let Some(warning) = warning {
                                catalog.diagnostics.push(warning);
                            }
                            if skill
                                .resource
                                .path()
                                .is_some_and(|path| skills.insert(path.to_owned()))
                            {
                                catalog.skills.push(skill);
                            }
                        }
                        Err(e) => catalog.diagnostics.push(Diagnostic {
                            path: candidate,
                            message: e.to_string(),
                        }),
                    }
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => {
                    catalog.diagnostics.push(Diagnostic {
                        path: candidate,
                        message: e.to_string(),
                    });
                    continue;
                }
            }
            if entries_truncated {
                continue;
            }
            let children = match fs::read_dir(&directory) {
                Ok(children) => children,
                Err(e) => {
                    catalog.diagnostics.push(Diagnostic {
                        path: directory,
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            let mut paths = Vec::new();
            for child in children {
                check(cancel)?;
                if entries_left == 0 {
                    entries_truncated = true;
                    catalog.diagnostics.push(Diagnostic {
                        path: root.path.clone(),
                        message: format!(
                            "Skills 扫描达到 {MAX_ENTRIES_PER_ROOT} 个目录项，已保留发现结果"
                        ),
                    });
                    break;
                }
                entries_left -= 1;
                match child {
                    Ok(child) => {
                        let path = child.path();
                        if child.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        match fs::symlink_metadata(&path) {
                            Ok(meta)
                                if meta.file_attributes() & crate::native::REPARSE_POINT != 0 =>
                            {
                                catalog.diagnostics.push(Diagnostic {
                                    path,
                                    message: "Skill 目录项不能是链接或重解析点".into(),
                                });
                            }
                            Ok(meta) if meta.is_dir() => paths.push(path),
                            Ok(_) => (),
                            Err(e) => catalog.diagnostics.push(Diagnostic {
                                path,
                                message: e.to_string(),
                            }),
                        }
                    }
                    Err(e) => catalog.diagnostics.push(Diagnostic {
                        path: directory.clone(),
                        message: e.to_string(),
                    }),
                }
            }
            paths.sort();
            if depth >= crate::MAX_SCAN_DEPTH {
                if !paths.is_empty() {
                    catalog.diagnostics.push(Diagnostic {
                        path: directory,
                        message: "Skills 扫描达到深度 6，未扫描更深目录".into(),
                    });
                }
            } else {
                pending.extend(paths.into_iter().rev().map(|path| (path, depth + 1)));
            }
        }
    }
    check(cancel)?;
    Ok(catalog)
}
