use crate::{Catalog, EmbeddedSkill, Failure, Root, Skill, SkillResource, Source, check, parse};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub const READ_RESOURCE_TOOL: &str = "read_skill_resource";
pub const BUILTIN_NAME: &str = "skill-creator";

const FILES: &[(&str, &str, &[u8])] = &[
    (
        "SKILL.md",
        "text/markdown",
        include_bytes!("../assets/skill-creator/SKILL.md"),
    ),
    (
        "agents/openai.yaml",
        "text/yaml",
        include_bytes!("../assets/skill-creator/agents/openai.yaml"),
    ),
    (
        "assets/skill-creator-small.svg",
        "image/svg+xml",
        include_bytes!("../assets/skill-creator/assets/skill-creator-small.svg"),
    ),
    (
        "assets/skill-creator.png",
        "image/png",
        include_bytes!("../assets/skill-creator/assets/skill-creator.png"),
    ),
    (
        "references/openai_yaml.md",
        "text/markdown",
        include_bytes!("../assets/skill-creator/references/openai_yaml.md"),
    ),
    (
        "license.txt",
        "text/plain",
        include_bytes!("../assets/skill-creator/license.txt"),
    ),
    (
        "source.md",
        "text/markdown",
        include_bytes!("../assets/skill-creator/source.md"),
    ),
];

pub struct EmbeddedResource {
    pub bytes: &'static [u8],
    pub media_type: &'static str,
}

pub fn read_resource(
    skill: EmbeddedSkill,
    path: &str,
    cancel: &CancellationToken,
) -> Result<EmbeddedResource, Failure> {
    check(cancel)?;
    if path.len() > 256
        || path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(Failure(
            "内置 Skill 资源必须使用其根目录下的相对路径".into(),
        ));
    }
    let files = match skill {
        EmbeddedSkill::SkillCreator => FILES,
    };
    files
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, media_type, bytes)| EmbeddedResource { bytes, media_type })
        .ok_or_else(|| Failure(format!("内置 Skill 资源不存在：{path}")))
}

pub(crate) fn text(
    skill: EmbeddedSkill,
    path: &str,
    cancel: &CancellationToken,
) -> Result<&'static str, Failure> {
    let resource = read_resource(skill, path, cancel)?;
    std::str::from_utf8(resource.bytes)
        .map_err(|_| Failure("内置 Skill 资源不是 UTF-8 文本".into()))
}

pub fn discover_standard(
    user_skills_root: &Path,
    include_builtin: bool,
    cancel: &CancellationToken,
) -> Result<Catalog, Failure> {
    check(cancel)?;
    if !user_skills_root.is_absolute() {
        return Err(Failure(
            "用户 Skills 根必须是应用数据目录下的绝对路径".into(),
        ));
    }
    let mut catalog = crate::discovery::discover(
        vec![Root {
            path: user_skills_root.to_owned(),
            source: Source::User,
        }],
        cancel,
    )?;
    if include_builtin {
        let embedded = EmbeddedSkill::SkillCreator;
        let metadata = parse(text(embedded, "SKILL.md", cancel)?)?;
        let extension = crate::discovery::extension(text(embedded, "agents/openai.yaml", cancel)?)?;
        catalog.skills.push(Skill {
            resource: SkillResource::Embedded { embedded },
            source: Source::Builtin,
            metadata,
            enabled: true,
            allow_implicit_invocation: extension.policy.allow_implicit_invocation,
            interface: extension.interface,
            dependencies: extension.dependencies,
        });
    }
    catalog.diagnose_duplicates();
    Ok(catalog)
}
