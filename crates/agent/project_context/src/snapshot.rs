use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Clone)]
pub struct RuleSource {
    pub path: PathBuf,
    pub scope: PathBuf,
    pub text: String,
    pub hash: String,
}

#[derive(Clone)]
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
