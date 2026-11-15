use crate::snapshot::{InstructionConfig, content_hash};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRef {
    pub id: String,
    pub version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptKind {
    Complete,
    Excerpt,
    Slice,
    Custom,
    Derived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRevision {
    pub reference: PromptRef,
    pub name: String,
    pub language: String,
    pub pair: Option<String>,
    pub source: String,
    pub range: String,
    pub dependencies: String,
    pub kind: PromptKind,
    pub readonly: bool,
    pub derived_from: Option<PromptRef>,
    pub text: String,
    pub hash: String,
}

impl PromptRevision {
    pub fn custom(name: String, text: String, language: String, source: String) -> Self {
        Self {
            reference: PromptRef {
                id: Uuid::new_v4().to_string(),
                version: 1,
            },
            name,
            language,
            source,
            pair: None,
            range: "全文".into(),
            dependencies: String::new(),
            kind: PromptKind::Custom,
            readonly: false,
            derived_from: None,
            hash: content_hash(&text),
            text,
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.reference.id.trim().is_empty()
            || self.reference.version == 0
            || self.name.trim().is_empty()
            || self.language.trim().is_empty()
            || self.source.trim().is_empty()
            || self.text.trim().is_empty()
            || self.hash != content_hash(&self.text)
        {
            return Err(io::Error::other(
                "提示词版本的标识、名称、语言、来源、正文或哈希无效",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PromptPreset {
    pub name: String,
    pub config: InstructionConfig,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Library {
    pub entries: Vec<PromptRevision>,
    pub presets: Vec<PromptPreset>,
}

impl Library {
    pub fn find(&self, reference: &PromptRef) -> io::Result<&PromptRevision> {
        self.entries
            .iter()
            .find(|entry| entry.reference == *reference)
            .ok_or_else(|| io::Error::other("找不到所选提示词版本"))
    }

    pub fn validate(&self) -> io::Result<()> {
        for (index, entry) in self.entries.iter().enumerate() {
            entry.validate()?;
            if self.entries[..index]
                .iter()
                .any(|other| other.reference == entry.reference)
            {
                return Err(io::Error::other("提示词库包含重复的版本标识"));
            }
        }
        for (index, preset) in self.presets.iter().enumerate() {
            if preset.name.trim().is_empty()
                || self.presets[..index]
                    .iter()
                    .any(|other| other.name == preset.name)
            {
                return Err(io::Error::other("组合预设名称为空或重复"));
            }
            self.resolve(&preset.config)?;
        }
        Ok(())
    }

    pub fn revise(
        &mut self,
        original: Option<&PromptRef>,
        name: String,
        text: String,
        language: String,
    ) -> io::Result<PromptRef> {
        let mut entry = match original {
            Some(reference) => {
                let source = self.find(reference)?;
                let mut entry = source.clone();
                entry.derived_from = Some(source.reference.clone());
                entry.kind = PromptKind::Derived;
                if source.readonly {
                    entry.reference = PromptRef {
                        id: Uuid::new_v4().to_string(),
                        version: 1,
                    };
                    entry.readonly = false;
                } else {
                    entry.reference.version = self
                        .entries
                        .iter()
                        .filter(|other| other.reference.id == source.reference.id)
                        .map(|other| other.reference.version)
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("提示词版本已耗尽"))?;
                }
                entry
            }
            None => PromptRevision::custom(
                name.clone(),
                text.clone(),
                language.clone(),
                "自定义".into(),
            ),
        };
        entry.name = name;
        entry.language = language;
        entry.text = text;
        entry.hash = content_hash(&entry.text);
        entry.validate()?;
        let reference = entry.reference.clone();
        self.entries.push(entry);
        Ok(reference)
    }

    pub fn load(path: &Path, sources: &Self) -> io::Result<Self> {
        let mut library = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Self>(&text).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        for source in &sources.entries {
            if let Some(existing) = library
                .entries
                .iter()
                .find(|entry| entry.reference == source.reference)
            {
                if existing != source {
                    return Err(io::Error::other(
                        "提示词库中的内置来源版本已被修改，未覆盖原文件",
                    ));
                }
            } else {
                library.entries.push(source.clone());
            }
        }
        library.validate()?;
        Ok(library)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("提示词库缺少父目录"))?;
        let mut file = NamedTempFile::with_prefix_in(".agentelier-prompts-", parent)?;
        file.write_all(
            serde_json::to_string_pretty(self)
                .map_err(io::Error::other)?
                .as_bytes(),
        )?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}
