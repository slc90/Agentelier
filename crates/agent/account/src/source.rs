use agentelier_login::{CodexAccountSummary, CodexReadFailure, read_codex_account};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type SaveSource = dyn Fn(Option<&Path>) -> io::Result<()> + Send + Sync;

#[derive(Clone)]
pub struct SourceSettings {
    selected_file: Option<PathBuf>,
    save: Arc<SaveSource>,
}

impl SourceSettings {
    pub fn new(
        selected_file: Option<PathBuf>,
        save: impl Fn(Option<&Path>) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            selected_file,
            save: Arc::new(save),
        }
    }

    pub fn codex_selected(&self) -> bool {
        self.selected_file.is_some()
    }

    pub(crate) fn read(&self) -> AccountSource {
        self.selected_file
            .as_ref()
            .map_or(AccountSource::Siwc, |path| AccountSource::Codex {
                path: path.clone(),
                status: read_codex_account(path),
            })
    }

    pub(crate) fn change(&mut self, change: SourceChange) -> io::Result<()> {
        let selected = match change {
            SourceChange::Select(path) if path.is_absolute() => Some(path),
            SourceChange::Select(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Codex 凭据文件必须使用绝对路径",
                ));
            }
            SourceChange::Disconnect => None,
        };
        if self.selected_file != selected {
            (self.save)(selected.as_deref())?;
            self.selected_file = selected;
        }
        Ok(())
    }
}

pub enum SourceChange {
    Select(PathBuf),
    Disconnect,
}

#[derive(Clone, PartialEq, Eq)]
pub enum AccountSource {
    Siwc,
    Codex {
        path: PathBuf,
        status: Result<CodexAccountSummary, CodexReadFailure>,
    },
}
