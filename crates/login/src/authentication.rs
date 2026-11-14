use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error, fmt};

pub(crate) const ISSUER: &str = "https://auth.openai.com";
pub(crate) const SUBSCRIPTION_SCOPE: &str = "chatgpt.tokens.use.direct";
const MAX_AUTH_FILE_BYTES: u64 = 1024 * 1024;

pub struct AuthFile {
    pub(crate) path: PathBuf,
    pub(crate) record: Option<AccountRecord>,
    pub(crate) pending: Option<AccountRecord>,
    pub(crate) issued_client: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountRecord {
    pub(crate) ext_agent_host_id: String,
    pub(crate) registration: Option<Registration>,
    pub(crate) session: Option<Session>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Registration {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    pub(crate) subject: String,
    pub(crate) display_name: Option<String>,
    pub(crate) email: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Session {
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) id_token: String,
    pub(crate) token_type: Option<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) received_at: u64,
    pub(crate) expires_at: Option<u64>,
    pub(crate) refresh_not_before: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}

#[derive(Clone, Debug)]
pub enum CredentialState {
    NoRecord,
    HostOnly,
    SignedOut,
    StoredSession {
        expired: bool,
        subscription_enabled: bool,
    },
}

#[derive(Clone, Debug)]
pub struct RestoredAccount {
    pub account: Option<AccountSummary>,
    pub credentials: CredentialState,
}

#[derive(Debug)]
pub enum RestoreFailure {
    Read(io::Error),
    Parse { line: usize, column: usize },
    Invalid(&'static str),
    TooLarge,
    Clock,
}

impl fmt::Display for RestoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(f, "无法读取 auth.json：{error}"),
            Self::Parse { line, column } => {
                write!(f, "auth.json 格式无效（第 {line} 行，第 {column} 列）")
            }
            Self::Invalid(field) => write!(f, "auth.json 字段不一致或无效：{field}"),
            Self::TooLarge => f.write_str("auth.json 超过允许的 1 MiB 大小"),
            Self::Clock => f.write_str("系统时钟早于 Unix 时间起点，无法判断凭据状态"),
        }
    }
}

impl Error for RestoreFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            _ => None,
        }
    }
}

impl AuthFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            record: None,
            pending: None,
            issued_client: None,
        }
    }

    pub fn restore(&mut self) -> Result<RestoredAccount, RestoreFailure> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.record = None;
                return Ok(RestoredAccount {
                    account: None,
                    credentials: CredentialState::NoRecord,
                });
            }
            Err(error) => return Err(RestoreFailure::Read(error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_AUTH_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(RestoreFailure::Read)?;
        if bytes.len() as u64 > MAX_AUTH_FILE_BYTES {
            return Err(RestoreFailure::TooLarge);
        }
        // Serde diagnostics may contain values copied from the credential file.
        let record: AccountRecord =
            serde_json::from_slice(&bytes).map_err(|error| RestoreFailure::Parse {
                line: error.line(),
                column: error.column(),
            })?;
        record.validate()?;
        let projection = record.projection()?;
        self.record = Some(record);
        Ok(projection)
    }
}

impl AccountRecord {
    pub(crate) fn validate(&self) -> Result<(), RestoreFailure> {
        let host = self
            .ext_agent_host_id
            .strip_prefix("urn:uuid:")
            .and_then(|value| uuid::Uuid::parse_str(value).ok());
        if host.is_none_or(|id| id.is_nil()) {
            return Err(RestoreFailure::Invalid("ext_agent_host_id"));
        }
        if let Some(registration) = &self.registration {
            if registration.issuer != ISSUER {
                return Err(RestoreFailure::Invalid("registration.issuer"));
            }
            if registration.client_id.trim().is_empty() || registration.subject.trim().is_empty() {
                return Err(RestoreFailure::Invalid("registration.client_id / subject"));
            }
            if registration
                .display_name
                .as_ref()
                .is_some_and(|text| text.trim().is_empty())
                || registration
                    .email
                    .as_ref()
                    .is_some_and(|text| text.trim().is_empty())
            {
                return Err(RestoreFailure::Invalid("registration.display_name / email"));
            }
        }
        if let Some(session) = &self.session {
            if self.registration.is_none() {
                return Err(RestoreFailure::Invalid("session requires registration"));
            }
            if session
                .access_token
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
                || session
                    .refresh_token
                    .as_ref()
                    .is_some_and(|value| value.trim().is_empty())
                || session.id_token.trim().is_empty()
            {
                return Err(RestoreFailure::Invalid("session tokens"));
            }
            if session.access_token.is_some()
                && !session
                    .token_type
                    .as_ref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("Bearer"))
            {
                return Err(RestoreFailure::Invalid("session.token_type"));
            }
            let mut scopes = HashSet::new();
            if session.scopes.is_empty()
                || session.scopes.iter().any(|scope| {
                    scope.is_empty()
                        || scope.chars().any(char::is_whitespace)
                        || !scopes.insert(scope)
                })
            {
                return Err(RestoreFailure::Invalid("session.scopes"));
            }
            if session.access_token.is_some() != session.expires_at.is_some()
                || session.access_token.is_some() != session.token_type.is_some()
                || session
                    .expires_at
                    .is_some_and(|time| time <= session.received_at)
            {
                return Err(RestoreFailure::Invalid("session timestamps"));
            }
            if (session
                .scopes
                .iter()
                .any(|scope| scope == SUBSCRIPTION_SCOPE || scope == "offline_access")
                && session.access_token.is_none())
                || (session.scopes.iter().any(|scope| scope == "offline_access")
                    && session.refresh_token.is_none())
            {
                return Err(RestoreFailure::Invalid("session granted credentials"));
            }
        }
        Ok(())
    }

    pub(crate) fn projection(&self) -> Result<RestoredAccount, RestoreFailure> {
        let account = self
            .registration
            .as_ref()
            .map(|registration| AccountSummary {
                subject: registration.subject.clone(),
                display_name: registration.display_name.clone(),
                email: registration.email.clone(),
            });
        let credentials = if let Some(session) = &self.session {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| RestoreFailure::Clock)?
                .as_secs();
            CredentialState::StoredSession {
                expired: session.expires_at.is_some_and(|time| now >= time),
                subscription_enabled: session
                    .scopes
                    .iter()
                    .any(|scope| scope == SUBSCRIPTION_SCOPE),
            }
        } else if account.is_some() {
            CredentialState::SignedOut
        } else {
            CredentialState::HostOnly
        };
        Ok(RestoredAccount {
            account,
            credentials,
        })
    }
}
