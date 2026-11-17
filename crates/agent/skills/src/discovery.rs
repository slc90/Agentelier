use crate::{Catalog, Diagnostic, Failure, Root, Skill, Source, check, parse};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub fn standard_roots(directory: &Path, cancel: &CancellationToken) -> Result<Vec<Root>, Failure> {
    check(cancel)?;
    let directory = directory
        .canonicalize()
        .map_err(|e| Failure(e.to_string()))?;
    if !directory.is_dir() {
        return Err(Failure("Skills 工作目录不是目录".into()));
    }
    let mut chain = Vec::new();
    let mut found_boundary = false;
    for scope in directory.ancestors() {
        check(cancel)?;
        chain.push(scope.to_owned());
        match fs::metadata(scope.join(".git")) {
            Ok(marker) if marker.is_dir() || marker.is_file() => {
                found_boundary = true;
                break;
            }
            Ok(_) => return Err(Failure("Skills 项目 Git 边界无效".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(Failure(format!("Skills 项目边界读取失败：{e}"))),
        }
    }
    if !found_boundary {
        chain.truncate(1);
    }
    chain.reverse();
    let mut roots: Vec<_> = chain
        .into_iter()
        .map(|scope| Root {
            path: scope.join(".agents/skills"),
            source: Source::Project { scope },
        })
        .collect();
    let profile = std::env::var_os("USERPROFILE")
        .ok_or_else(|| Failure("缺少用户主目录 USERPROFILE".into()))?;
    roots.push(Root {
        path: PathBuf::from(profile).join(".agents/skills"),
        source: Source::User,
    });
    let executable = std::env::current_exe().map_err(|e| Failure(e.to_string()))?;
    let parent = executable
        .parent()
        .ok_or_else(|| Failure("应用路径缺少父目录".into()))?;
    roots.push(Root {
        path: parent.join("skills/.system"),
        source: Source::Builtin,
    });
    Ok(roots)
}

pub(crate) fn read_file(path: &Path, limit: usize) -> Result<String, Failure> {
    let file = File::open(path).map_err(|e| Failure(format!("{}：{e}", path.display())))?;
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

fn read_frontmatter(path: &Path, bytes_used: &mut usize) -> Result<String, Failure> {
    let file = File::open(path).map_err(|e| Failure(e.to_string()))?;
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
        *bytes_used += count;
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
struct Extension {
    interface: Option<serde_json::Value>,
    dependencies: Option<serde_json::Value>,
    #[serde(default)]
    policy: Policy,
}
#[derive(Deserialize)]
struct Policy {
    #[serde(default = "implicit_default")]
    allow_implicit_invocation: bool,
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

fn skill(
    path: &Path,
    source: &Source,
    bytes_used: &mut usize,
) -> Result<(Skill, Option<Diagnostic>), Failure> {
    let yaml = read_frontmatter(path, bytes_used)?;
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
    let extension = match fs::symlink_metadata(&extra_path) {
        Ok(_) => read_file(&extra_path, crate::MAX_METADATA_BYTES).and_then(|text| {
            serde_yaml::from_str::<Extension>(&text)
                .map(|extra| (extra, text.len()))
                .map_err(|e| Failure(e.to_string()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Extension::default(), 0)),
        Err(e) => Err(Failure(e.to_string())),
    };
    let (extension, warning, extension_bytes) = match extension {
        Ok((extra, bytes)) => (extra, None, bytes),
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
            crate::MAX_METADATA_BYTES,
        ),
    };
    *bytes_used += extension_bytes;
    Ok((
        Skill {
            path: canonical,
            discovered_path: path.to_owned(),
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

pub fn discover(roots: Vec<Root>, cancel: &CancellationToken) -> Result<Catalog, Failure> {
    let mut catalog = Catalog {
        roots,
        ..Catalog::default()
    };
    let mut visited = BTreeSet::new();
    let mut skills = BTreeSet::new();
    let mut entries_used = 0;
    let mut bytes_used = 0;
    for root in catalog.roots.clone() {
        let mut pending = vec![(root.path, 0)];
        while let Some((directory, depth)) = pending.pop() {
            check(cancel)?;
            let canonical = match directory.canonicalize() {
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
            if !visited.insert(canonical.clone()) {
                continue;
            }
            let candidate = directory.join("SKILL.md");
            match fs::symlink_metadata(&candidate) {
                Ok(_) => {
                    let result = skill(&candidate, &root.source, &mut bytes_used);
                    if bytes_used > crate::MAX_DISCOVERY_BYTES {
                        return Err(Failure("Skills 发现材料超过 4 MiB".into()));
                    }
                    match result {
                        Ok((skill, warning)) => {
                            if let Some(warning) = warning {
                                catalog.diagnostics.push(warning);
                            }
                            if skills.insert(skill.path.clone()) {
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
                entries_used += 1;
                if entries_used > crate::MAX_SCAN_ENTRIES {
                    return Err(Failure("Skills 扫描超过 2048 个目录项".into()));
                }
                match child {
                    Ok(child) => {
                        let path = child.path();
                        if child.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        match fs::metadata(&path) {
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
