use super::{STARTER, Validation, metadata, optional_metadata, require_directory, require_file};
use crate::{Failure, MAX_SKILL_BYTES, check, discovery::read_file, parse, parser::frontmatter};
use percent_encoding::percent_decode_str;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(super) fn directory(
    root: &Path,
    name: &str,
    cancel: &CancellationToken,
) -> Result<Validation, Failure> {
    check(cancel)?;
    let path = root.join("SKILL.md");
    require_file(&path)?;
    let text = read_file(&path, MAX_SKILL_BYTES)?;
    let parsed = parse(&text)?;
    if parsed.name != name {
        return Err(Failure(format!(
            "目录 {name} 与 frontmatter 名称 {} 不一致",
            parsed.name
        )));
    }
    let (yaml, body) = frontmatter(&text)?;
    let yaml: serde_yaml::Mapping = serde_yaml::from_str(yaml)
        .map_err(|failure| Failure(format!("Skill YAML 无效：{failure}")))?;
    for key in yaml.keys() {
        if !key.as_str().is_some_and(|key| {
            matches!(
                key,
                "name" | "description" | "license" | "allowed-tools" | "metadata" | "compatibility"
            )
        }) {
            return Err(Failure(format!(
                "SKILL.md 不支持该 frontmatter 字段：{key:?}"
            )));
        }
    }
    if parsed.description.starts_with("[TODO:") || body.trim().is_empty() {
        return Err(Failure("SKILL.md 仍含空白正文或未完成的说明占位".into()));
    }
    for folder in ["agents", "references", "scripts", "assets"] {
        let path = root.join(folder);
        if optional_metadata(&path)?.is_some() {
            require_directory(&path)?;
        }
    }
    let references_checked = links(root, body, cancel)?;
    Ok(Validation {
        metadata: parsed,
        references_checked,
        metadata_sha256: metadata::inspect(root, name)?,
    })
}

fn links(root: &Path, body: &str, cancel: &CancellationToken) -> Result<usize, Failure> {
    let mut references = BTreeSet::new();
    let mut fenced = Vec::new();
    let mut code = None;
    for (event, span) in Parser::new(body).into_offset_iter() {
        check(cancel)?;
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = Some(span.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = code.take() {
                    fenced.push(start..span.end);
                }
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                if !dest_url.is_empty()
                    && !dest_url.starts_with('#')
                    && !["http://", "https://", "mailto:"]
                        .iter()
                        .any(|prefix| dest_url.starts_with(prefix)) =>
            {
                references.insert(resource_path(root, &dest_url)?);
            }
            _ => {}
        }
    }
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        let marked =
            trimmed == STARTER || (trimmed.starts_with("[TODO:") && trimmed.ends_with(']'));
        if marked && !fenced.iter().any(|span| span.contains(&offset)) {
            return Err(Failure("SKILL.md 仍含未完成的模板占位".into()));
        }
        offset += line.len();
    }
    Ok(references.len())
}

pub(super) fn resource_path(root: &Path, value: &str) -> Result<PathBuf, Failure> {
    let value = value.split_once('#').map_or(value, |(path, _)| path);
    let decoded = percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| Failure("Skill 资源链接必须是 UTF-8 路径".into()))?;
    let relative = decoded.strip_prefix("./").unwrap_or(&decoded);
    let mut target = root.to_owned();
    for part in relative.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with([' ', '.'])
            || part.chars().any(|ch| {
                ch.is_control() || matches!(ch, '\\' | ':' | '?' | '*' | '"' | '<' | '>' | '|')
            })
        {
            return Err(Failure(format!("Skill 资源须位于当前目录内：{value}")));
        }
        target.push(part);
        if optional_metadata(&target)?.is_none() {
            return Err(Failure(format!("Skill 引用的资源不存在：{value}")));
        }
    }
    Ok(target)
}
