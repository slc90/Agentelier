use crate::{CodexIdentity, CodexOAuthCredentials};
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

impl CodexOAuthCredentials {
    pub fn version(&self) -> CredentialVersion {
        CredentialVersion {
            identity: self.summary().identity.clone(),
            fingerprint: Sha256::digest(self.access_token().as_bytes()).into(),
        }
    }
}

#[derive(Clone)]
pub struct UsableCredentials {
    access_token: String,
    version: CredentialVersion,
}

impl UsableCredentials {
    pub fn from_oauth(credentials: &CodexOAuthCredentials) -> Result<Self, SessionFailure> {
        if credentials
            .is_expired()
            .map_err(|failure| SessionFailure::new(failure.to_string()))?
        {
            return Err(SessionFailure::new("Codex OAuth 凭据已过期"));
        }
        Ok(Self {
            version: credentials.version(),
            access_token: credentials.access_token().to_owned(),
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
    pub fn output_started(&self) -> bool {
        self.output_started
    }
    pub fn reset_output_for_retry(&mut self) {
        self.output_started = false;
    }
    pub fn begin_recovery(&mut self, status: u16) -> Option<CredentialVersion> {
        if status != 401 || self.recovery_used || self.output_started {
            return None;
        }
        self.recovery_used = true;
        Some(self.version.clone())
    }
}
