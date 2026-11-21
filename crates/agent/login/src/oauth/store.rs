use super::{CodexOAuthCredentials, OAuthTokens};
use crate::{CodexAccountSummary, CodexReadFailure, DataEnvironment};
use serde::Deserialize;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use tempfile::NamedTempFile;

const FILE_NAME: &str = "codex-oauth.json";
const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub enum CredentialStoreFailure {
    Read(io::Error),
    Format,
    Invalid(CodexReadFailure),
    Write(io::Error),
}

impl fmt::Display for CredentialStoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(f, "无法读取本地 Codex OAuth 凭据：{error}"),
            Self::Format => f.write_str("本地 Codex OAuth 凭据文件格式无效"),
            Self::Invalid(error) => error.fmt(f),
            Self::Write(error) => write!(f, "无法保存本地 Codex OAuth 凭据：{error}"),
        }
    }
}

impl std::error::Error for CredentialStoreFailure {}

#[derive(Deserialize)]
struct StoredCredentials {
    tokens: OAuthTokens,
    summary: CodexAccountSummary,
}

#[derive(Clone)]
pub struct CredentialStore {
    environment: DataEnvironment,
    writer: Arc<Mutex<()>>,
}

impl CredentialStore {
    pub fn new(environment: DataEnvironment) -> Self {
        Self {
            environment,
            writer: Arc::new(Mutex::new(())),
        }
    }

    pub fn load(&self) -> Result<Option<CodexOAuthCredentials>, CredentialStoreFailure> {
        let file = match File::open(self.environment.root().join(FILE_NAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CredentialStoreFailure::Read(error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CREDENTIAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(CredentialStoreFailure::Read)?;
        if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
            return Err(CredentialStoreFailure::Format);
        }
        let stored: StoredCredentials =
            serde_json::from_slice(&bytes).map_err(|_| CredentialStoreFailure::Format)?;
        let credentials = CodexOAuthCredentials::from_tokens(stored.tokens)
            .map_err(CredentialStoreFailure::Invalid)?;
        if stored.summary.identity != credentials.summary.identity
            || stored.summary.email != credentials.summary.email
        {
            return Err(CredentialStoreFailure::Invalid(
                CodexReadFailure::IdentityMismatch,
            ));
        }
        Ok(Some(credentials))
    }

    pub fn save(&self, credentials: &CodexOAuthCredentials) -> Result<(), CredentialStoreFailure> {
        let _writer = self.lock_writer()?;
        self.write(credentials)
    }

    pub fn replace_if_current(
        &self,
        expected: &CodexOAuthCredentials,
        replacement: &CodexOAuthCredentials,
    ) -> Result<bool, CredentialStoreFailure> {
        let _writer = self.lock_writer()?;
        if self.load()?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.write(replacement)?;
        Ok(true)
    }

    fn lock_writer(&self) -> Result<MutexGuard<'_, ()>, CredentialStoreFailure> {
        self.writer
            .lock()
            .map_err(|_| CredentialStoreFailure::Write(io::Error::other("本地凭据写入状态不可用")))
    }

    fn write(&self, credentials: &CodexOAuthCredentials) -> Result<(), CredentialStoreFailure> {
        let bytes = serde_json::to_vec(credentials).map_err(|_| CredentialStoreFailure::Format)?;
        if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
            return Err(CredentialStoreFailure::Format);
        }
        let root = self.environment.root();
        let mut temporary = NamedTempFile::with_prefix_in(".codex-oauth-", root)
            .map_err(CredentialStoreFailure::Write)?;
        temporary
            .write_all(&bytes)
            .map_err(CredentialStoreFailure::Write)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(CredentialStoreFailure::Write)?;
        temporary
            .persist(root.join(FILE_NAME))
            .map_err(|failure| CredentialStoreFailure::Write(failure.error))?;
        Ok(())
    }
}
