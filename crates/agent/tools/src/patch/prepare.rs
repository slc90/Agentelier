use super::{
    MAX_FILE_BYTES, MAX_PREPARED_BYTES, invalid,
    parse::{Action, Chunk},
};
use crate::{ToolFailure, WorkingDirectory, blocking::Control};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) struct Prepared {
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
    pub(super) old: Option<Vec<u8>>,
    pub(super) new: Option<Vec<u8>>,
    pub(super) directories: Vec<PathBuf>,
}

pub(super) fn unchanged(path: &Path, expected: &[u8]) -> Result<bool, ToolFailure> {
    const COMPARE_CHUNK_BYTES: usize = 8192;
    let mut file = fs::File::open(path).map_err(|error| invalid(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| invalid(error.to_string()))?;
    if !metadata.is_file() || metadata.len() != expected.len() as u64 {
        return Ok(false);
    }
    let mut buffer = [0; COMPARE_CHUNK_BYTES];
    let mut offset = 0;
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|error| invalid(error.to_string()))?;
        if length == 0 {
            return Ok(offset == expected.len());
        }
        if expected.get(offset..offset + length) != Some(&buffer[..length]) {
            return Ok(false);
        }
        offset += length;
    }
}

fn read_limited(path: &Path, available: usize) -> Result<Vec<u8>, ToolFailure> {
    let limit = available.min(MAX_FILE_BYTES);
    let file =
        fs::File::open(path).map_err(|error| invalid(format!("{}: {error}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|error| invalid(error.to_string()))?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(invalid("目标必须是最多 8 MiB 的普通文本文件"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(error.to_string()))?;
    if bytes.len() > limit {
        return Err(invalid("实际文件读取超过 8 MiB"));
    }
    Ok(bytes)
}

fn path(base: &WorkingDirectory, value: &str) -> Result<PathBuf, ToolFailure> {
    if value.trim().is_empty() {
        return Err(invalid("文件路径不能为空"));
    }
    let absolute = base
        .resolve(Path::new(value))
        .and_then(std::path::absolute)
        .map_err(|error| invalid(error.to_string()))?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor
        .try_exists()
        .map_err(|error| invalid(error.to_string()))?
    {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| invalid("路径没有文件名"))?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| invalid("路径没有有效父目录"))?;
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|error| invalid(error.to_string()))?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn text(bytes: &[u8]) -> Result<(&str, bool, bool), ToolFailure> {
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let text = std::str::from_utf8(if bom { &bytes[3..] } else { bytes })
        .map_err(|_| invalid("只支持 UTF-8 文本"))?;
    if text.contains('\0') {
        return Err(invalid("文件包含 NUL，不作为 UTF-8 文本修改"));
    }
    let crlf = text.contains("\r\n");
    if text.as_bytes().iter().enumerate().any(|(i, byte)| {
        (*byte == b'\r' && text.as_bytes().get(i + 1) != Some(&b'\n'))
            || (*byte == b'\n' && crlf && (i == 0 || text.as_bytes()[i - 1] != b'\r'))
    }) {
        return Err(invalid("不支持混合行尾或孤立 CR"));
    }
    Ok((text, bom, crlf))
}

fn update(bytes: &[u8], chunks: Vec<Chunk>, control: &Control) -> Result<Vec<u8>, ToolFailure> {
    let (original, bom, crlf) = text(bytes)?;
    let mut lines: Vec<&str> = original
        .split_terminator('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut cursor = 0;
    let mut output = Vec::<String>::new();
    for chunk in chunks {
        control.check()?;
        if let Some(anchor) = &chunk.anchor {
            let matches: Vec<_> = lines
                .iter()
                .enumerate()
                .skip(cursor)
                .filter(|(_, line)| **line == anchor)
                .map(|(i, _)| i)
                .take(2)
                .collect();
            if matches.len() != 1 {
                return Err(invalid("@@ 定位行缺失或不唯一，请增加上下文"));
            }
            output.extend(
                lines[cursor..=matches[0]]
                    .iter()
                    .map(|line| (*line).to_owned()),
            );
            cursor = matches[0] + 1;
        }
        let length = chunk.old.len();
        let mut found = None;
        for start in cursor..=lines.len().saturating_sub(length) {
            if start + length > lines.len() {
                continue;
            }
            if chunk.eof && start + length != lines.len() {
                continue;
            }
            if lines[start..start + length]
                .iter()
                .zip(&chunk.old)
                .all(|(a, b)| a == b)
            {
                if found.is_some() {
                    return Err(invalid("上下文有多个匹配位置，请增加上下文"));
                }
                found = Some(start);
            }
        }
        let start = found.ok_or_else(|| invalid("上下文不匹配，请重新读取文件"))?;
        output.extend(lines[cursor..start].iter().map(|line| (*line).to_owned()));
        output.extend(chunk.new);
        cursor = start + length;
    }
    output.extend(lines.drain(cursor..).map(str::to_owned));
    let ending = if crlf { "\r\n" } else { "\n" };
    let size = output
        .iter()
        .map(|line| line.len() + ending.len())
        .sum::<usize>()
        + if bom { 3 } else { 0 };
    if size > MAX_FILE_BYTES {
        return Err(invalid("更新内容超过 8 MiB"));
    }
    let mut result = Vec::with_capacity(size);
    if bom {
        result.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    let last = output.len().saturating_sub(1);
    for (i, line) in output.into_iter().enumerate() {
        result.extend_from_slice(line.as_bytes());
        if i < last || original.ends_with('\n') {
            result.extend_from_slice(ending.as_bytes());
        }
    }
    Ok(result)
}

pub(super) fn prepare(
    actions: Vec<Action>,
    base: &WorkingDirectory,
    control: &Control,
) -> Result<Vec<Prepared>, ToolFailure> {
    let mut prepared = Vec::new();
    let mut paths = HashSet::new();
    let mut budget = 0;
    for action in actions {
        control.check()?;
        let (source, target, old, new) = match action {
            Action::Add {
                path: name,
                content,
            } => {
                let source = path(base, &name)?;
                if source
                    .try_exists()
                    .map_err(|error| invalid(error.to_string()))?
                {
                    return Err(invalid("新增目标已存在"));
                }
                (source.clone(), source, None, Some(content.into_bytes()))
            }
            Action::Delete { path: name } => {
                let source = path(base, &name)?;
                let bytes = read_limited(&source, MAX_PREPARED_BYTES.saturating_sub(budget))?;
                text(&bytes)?;
                (source.clone(), source, Some(bytes), None)
            }
            Action::Update {
                path: name,
                target,
                chunks,
            } => {
                let source = path(base, &name)?;
                let target = target
                    .map(|name| path(base, &name))
                    .transpose()?
                    .unwrap_or_else(|| source.clone());
                if target != source
                    && target
                        .try_exists()
                        .map_err(|error| invalid(error.to_string()))?
                {
                    return Err(invalid("移动目标已存在"));
                }
                let bytes = read_limited(
                    &source,
                    MAX_PREPARED_BYTES.saturating_sub(budget + crate::MAX_TOOL_BODY_BYTES) / 2,
                )?;
                budget += bytes.len();
                if budget + bytes.len() + crate::MAX_TOOL_BODY_BYTES > MAX_PREPARED_BYTES {
                    return Err(invalid("准备文本超过 32 MiB"));
                }
                let new = update(&bytes, chunks, control)?;
                budget -= bytes.len();
                (source, target, Some(bytes), Some(new))
            }
        };
        for candidate in if source == target {
            vec![&source]
        } else {
            vec![&source, &target]
        } {
            let key = candidate.to_string_lossy().to_lowercase();
            if paths.iter().any(|existing: &String| {
                key == *existing
                    || key.starts_with(&format!("{existing}\\"))
                    || existing.starts_with(&format!("{key}\\"))
            }) {
                return Err(invalid("补丁含重复、别名或相互冲突的路径"));
            }
            paths.insert(key);
        }
        budget += old.as_ref().map_or(0, Vec::len) + new.as_ref().map_or(0, Vec::len);
        if budget > MAX_PREPARED_BYTES {
            return Err(invalid("准备文本超过 32 MiB"));
        }
        let mut directories = Vec::new();
        if new.is_some() {
            let mut parent = target.parent().ok_or_else(|| invalid("目标没有父目录"))?;
            while !parent
                .try_exists()
                .map_err(|error| invalid(error.to_string()))?
            {
                directories.push(parent.to_owned());
                parent = parent
                    .parent()
                    .ok_or_else(|| invalid("目标没有有效父目录"))?;
            }
            if !parent.is_dir() {
                return Err(invalid("目标父路径不是目录"));
            }
            directories.reverse();
        }
        prepared.push(Prepared {
            source,
            target,
            old,
            new,
            directories,
        });
    }
    super::commit::check_report_budget(&prepared)?;
    Ok(prepared)
}
