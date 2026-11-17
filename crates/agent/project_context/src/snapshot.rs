use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RuleSource {
    pub path: PathBuf,
    pub scope: PathBuf,
    pub text: String,
    pub hash: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectSnapshot {
    pub working_directory: PathBuf,
    pub project_root: Option<PathBuf>,
    pub sources: Vec<RuleSource>,
    pub source_bytes: usize,
    pub rendered: String,
    pub hash: String,
}

pub(crate) fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl ProjectSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if !self.working_directory.is_absolute()
            || self
                .project_root
                .as_ref()
                .is_some_and(|root| !self.working_directory.starts_with(root))
            || self.source_bytes > crate::MAX_PROJECT_SOURCE_BYTES
        {
            return Err("项目快照目录或预算无效".into());
        }
        let boundary = self
            .project_root
            .as_ref()
            .unwrap_or(&self.working_directory);
        let mut previous = boundary.as_path();
        for (index, source) in self.sources.iter().enumerate() {
            if source.path != source.scope.join("AGENTS.md")
                || !source.scope.starts_with(boundary)
                || !self.working_directory.starts_with(&source.scope)
                || !source.scope.starts_with(previous)
                || (index > 0 && source.scope == previous)
                || source.hash != hash(&source.text)
            {
                return Err("项目快照来源、范围、顺序或摘要无效".into());
            }
            previous = &source.scope;
        }
        let text_bytes: usize = self.sources.iter().map(|source| source.text.len()).sum();
        let bom_bytes = self.source_bytes.saturating_sub(text_bytes);
        if self.source_bytes < text_bytes
            || bom_bytes > self.sources.len() * 3
            || !bom_bytes.is_multiple_of(3)
        {
            return Err("项目快照原始字节数无效".into());
        }
        let mut expected = self.clone();
        expected.render();
        if expected.rendered != self.rendered || expected.hash != self.hash {
            return Err("项目快照渲染文本或摘要不一致".into());
        }
        Ok(())
    }
    pub fn has_rules(&self) -> bool {
        self.sources
            .iter()
            .any(|source| !source.text.trim().is_empty())
    }

    pub(crate) fn render(&mut self) {
        let mut rendered = String::new();
        if self.has_rules() {
            rendered.push_str(&format!(
                "Project instructions\nSession working directory: {}\nProject boundary: {}\n\nFiles are ordered from parent to child. Each file applies to its scope and descendants. Deeper rules apply only within their subtree. Direct user requests and higher-priority instructions still apply. Read deeper AGENTS.md files with local tools when needed; they are not automatically included here. File bodies are quoted verbatim, not imported recursively.\n",
                self.working_directory.display(),
                self.project_root.as_deref().unwrap_or(&self.working_directory).display(),
            ));
            for source in &self.sources {
                if source.text.trim().is_empty() {
                    continue;
                }
                rendered.push_str(&format!(
                    "\nSource: {}\nScope: {}\n<project-rule>\n{}\n</project-rule>\n",
                    source.path.display(),
                    source.scope.display(),
                    source.text,
                ));
            }
        }
        self.hash = hash(&rendered);
        self.rendered = rendered;
    }
}
