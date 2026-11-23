use super::{
    digest, optional_metadata, require_directory, require_file, validation::resource_path,
};
use crate::{Failure, MAX_METADATA_BYTES, check, discovery::read_file};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InterfacePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon_small: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon_large: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    brand_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_prompt: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataUpdate {
    pub expected_sha256: Option<String>,
    pub interface: InterfacePatch,
    pub allow_implicit_invocation: Option<bool>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Document {
    #[serde(skip_serializing_if = "Option::is_none")]
    interface: Option<InterfacePatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<Policy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependencies: Option<Dependencies>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    allow_implicit_invocation: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dependencies {
    tools: Vec<Dependency>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dependency {
    #[serde(rename = "type")]
    kind: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl InterfacePatch {
    fn has_changes(&self) -> bool {
        self.display_name.is_some()
            || self.short_description.is_some()
            || self.icon_small.is_some()
            || self.icon_large.is_some()
            || self.brand_color.is_some()
            || self.default_prompt.is_some()
    }

    fn apply(&mut self, patch: Self) {
        self.display_name = patch.display_name.or(self.display_name.take());
        self.short_description = patch.short_description.or(self.short_description.take());
        self.icon_small = patch.icon_small.or(self.icon_small.take());
        self.icon_large = patch.icon_large.or(self.icon_large.take());
        self.brand_color = patch.brand_color.or(self.brand_color.take());
        self.default_prompt = patch.default_prompt.or(self.default_prompt.take());
    }

    fn validate(&self, root: &Path, name: &str) -> Result<(), Failure> {
        if self
            .display_name
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 128)
        {
            return Err(Failure("interface.display_name 必须为 1–128 字符".into()));
        }
        if self
            .short_description
            .as_ref()
            .is_some_and(|value| !(25..=64).contains(&value.trim().chars().count()))
        {
            return Err(Failure(
                "interface.short_description 必须为 25–64 字符".into(),
            ));
        }
        if self.brand_color.as_ref().is_some_and(|value| {
            value.len() != 7
                || !value.starts_with('#')
                || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        }) {
            return Err(Failure("interface.brand_color 必须使用 #RRGGBB".into()));
        }
        if self.default_prompt.as_ref().is_some_and(|value| {
            value.len() > 2048
                || !crate::mentions(value)
                    .iter()
                    .any(|candidate| candidate == name)
        }) {
            return Err(Failure(format!(
                "interface.default_prompt 必须提及 ${name}，并且不超过 2048 字节"
            )));
        }
        for icon in [self.icon_small.as_deref(), self.icon_large.as_deref()]
            .into_iter()
            .flatten()
        {
            require_file(&resource_path(root, icon)?)?;
        }
        Ok(())
    }
}

impl Document {
    fn parse(yaml: &str) -> Result<Self, Failure> {
        serde_yaml::from_str(yaml)
            .map_err(|failure| Failure(format!("agents/openai.yaml 无效：{failure}")))
    }

    fn validate(&self, root: &Path, name: &str) -> Result<(), Failure> {
        if let Some(interface) = &self.interface {
            interface.validate(root, name)?;
        }
        if self.dependencies.as_ref().is_some_and(|dependencies| {
            dependencies
                .tools
                .iter()
                .any(|tool| tool.kind != "mcp" || tool.value.trim().is_empty())
        }) {
            return Err(Failure(
                "dependencies.tools 需要 mcp 类型及非空 value".into(),
            ));
        }
        Ok(())
    }
}

fn read(path: &Path) -> Result<Option<String>, Failure> {
    if optional_metadata(path)?.is_none() {
        return Ok(None);
    }
    require_file(path)?;
    read_file(path, MAX_METADATA_BYTES).map(Some)
}

pub(super) fn inspect(root: &Path, name: &str) -> Result<Option<String>, Failure> {
    let Some(text) = read(&root.join("agents/openai.yaml"))? else {
        return Ok(None);
    };
    let parsed = Document::parse(&text)?;
    parsed.validate(root, name)?;
    Ok(Some(digest(&text)))
}

pub(super) fn write(
    root: &Path,
    name: &str,
    update: MetadataUpdate,
    cancel: &CancellationToken,
) -> Result<String, Failure> {
    check(cancel)?;
    let folder = root.join("agents");
    let folder_exists = optional_metadata(&folder)?.is_some();
    if folder_exists {
        require_directory(&folder)?;
    }
    let file = folder.join("openai.yaml");
    let original = read(&file)?;
    if original.as_deref().map(digest) != update.expected_sha256 {
        return Err(Failure(
            "agents/openai.yaml 已变化，请重新验证并使用最新的 SHA-256".into(),
        ));
    }
    let mut candidate = original
        .as_deref()
        .map(Document::parse)
        .transpose()?
        .unwrap_or_default();
    if update.interface.has_changes() {
        candidate
            .interface
            .get_or_insert_default()
            .apply(update.interface);
    } else if update.allow_implicit_invocation.is_none() {
        return Err(Failure("至少指定一项界面或调用策略字段".into()));
    }
    if let Some(allow_implicit_invocation) = update.allow_implicit_invocation {
        candidate.policy = Some(Policy {
            allow_implicit_invocation,
        });
    }
    candidate.validate(root, name)?;
    let yaml = serde_yaml::to_string(&candidate)
        .map_err(|failure| Failure(format!("无法生成 agents/openai.yaml：{failure}")))?;
    if yaml.len() > MAX_METADATA_BYTES {
        return Err(Failure("agents/openai.yaml 超过 64 KiB".into()));
    }
    if !folder_exists {
        fs::create_dir(&folder)?;
    }
    let mut temporary = NamedTempFile::with_prefix_in(".skill-metadata-", &folder)?;
    temporary.write_all(yaml.as_bytes())?;
    temporary.as_file().sync_all()?;
    check(cancel)?;
    require_directory(&folder)?;
    if read(&file)? != original {
        return Err(Failure(
            "agents/openai.yaml 在写入期间变化，未提交更新".into(),
        ));
    }
    if original.is_some() {
        temporary.persist(&file).map_err(|failure| failure.error)?;
    } else {
        temporary
            .persist_noclobber(&file)
            .map_err(|failure| failure.error)?;
    }
    Ok(digest(&yaml))
}
