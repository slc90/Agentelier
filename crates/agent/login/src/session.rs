use crate::{CodexCredentials, CodexIdentity};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};

#[derive(Clone, Debug)]
pub struct SessionFailure(String);

impl SessionFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for SessionFailure {}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialVersion {
    identity: CodexIdentity,
    fingerprint: [u8; 32],
}

impl CredentialVersion {
    pub fn identity(&self) -> &CodexIdentity {
        &self.identity
    }
}

#[derive(Clone)]
pub struct UsableCredentials {
    access_token: String,
    version: CredentialVersion,
}

impl UsableCredentials {
    pub fn from_codex(file: CodexCredentials) -> Result<Self, SessionFailure> {
        if file.summary.expired {
            return Err(SessionFailure::new(
                "Codex 外部凭据已过期，请在 Codex 中恢复授权",
            ));
        }
        Ok(Self {
            version: CredentialVersion {
                identity: file.summary.identity,
                fingerprint: Sha256::digest(file.access_token.as_bytes()).into(),
            },
            access_token: file.access_token,
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn version(&self) -> &CredentialVersion {
        &self.version
    }
}

pub struct AuthenticatedRequest {
    version: CredentialVersion,
    recovery_used: bool,
    output_started: bool,
}
impl AuthenticatedRequest {
    pub fn new(credentials: &UsableCredentials) -> Self {
        Self {
            version: credentials.version.clone(),
            recovery_used: false,
            output_started: false,
        }
    }
    pub fn mark_output_started(&mut self) {
        self.output_started = true;
    }
    pub fn begin_recovery(&mut self, status: u16) -> Option<CredentialVersion> {
        if status != 401 || self.recovery_used || self.output_started {
            return None;
        }
        self.recovery_used = true;
        Some(self.version.clone())
    }
}
