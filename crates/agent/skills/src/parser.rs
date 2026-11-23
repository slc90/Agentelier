use crate::{Failure, Metadata};
use serde::Deserialize;

const MAX_NAME_CHARACTERS: usize = 64;
const MAX_DESCRIPTION_CHARACTERS: usize = 1024;

#[derive(Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
}

pub(crate) fn frontmatter(text: &str) -> Result<(&str, &str), Failure> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    let Some(opening) = lines
        .next()
        .filter(|line| line.trim_end_matches(['\r', '\n']) == "---")
    else {
        return Err(Failure("SKILL.md 缺少 YAML frontmatter".into()));
    };
    let mut bytes = opening.len();
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((&text[opening.len()..bytes], &text[bytes + line.len()..]));
        }
        bytes += line.len();
    }
    Err(Failure("SKILL.md frontmatter 未封闭".into()))
}

pub fn parse(text: &str) -> Result<Metadata, Failure> {
    let (yaml, _) = frontmatter(text)?;
    let parsed: Frontmatter =
        serde_yaml::from_str(yaml).map_err(|error| Failure(format!("Skill YAML 无效：{error}")))?;
    let name = parsed.name.trim().to_owned();
    let description = parsed
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .chars()
            .all(|c| c.is_lowercase() || c.is_numeric() || c == '-')
    {
        return Err(Failure(
            "Skill name 必须是至多 64 字符的小写名称，以单个连字符分词".into(),
        ));
    }
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION_CHARACTERS {
        return Err(Failure("Skill description 必须包含 1–1024 字符".into()));
    }
    Ok(Metadata { name, description })
}
