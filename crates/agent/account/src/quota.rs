use crate::{Account, CODEX_ORIGINATOR, CodexIdentity, CredentialClient, CredentialVersion};
use agentelier_login::UsableCredentials;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const FIVE_HOUR_SECONDS: u32 = 5 * 60 * 60;
const WEEK_SECONDS: u32 = 7 * 24 * 60 * 60;
const SECONDS_PER_MINUTE: u32 = 60;

#[derive(Clone)]
pub struct QuotaWindow {
    pub duration_minutes: u32,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone)]
pub struct QuotaSnapshot {
    pub identity: CodexIdentity,
    pub fetched_at: u64,
    pub five_hours: Option<QuotaWindow>,
    pub week: Option<QuotaWindow>,
}

pub struct QuotaUpdate {
    pub identity: CodexIdentity,
    pub snapshot: Option<QuotaSnapshot>,
    pub stale: bool,
}

pub struct QuotaAttempt {
    credentials: CredentialClient,
    started_with: CredentialVersion,
}

pub struct QuotaCompletion {
    version: CredentialVersion,
    result: Result<QuotaSnapshot, String>,
}

impl QuotaAttempt {
    pub(super) fn new(credentials: CredentialClient, started_with: CredentialVersion) -> Self {
        Self {
            credentials,
            started_with,
        }
    }

    pub async fn run(self) -> QuotaCompletion {
        let acquire = async {
            let credentials = self
                .credentials
                .request()
                .map_err(|_| ())?
                .wait()
                .await
                .map_err(|_| ())?;
            (credentials.version().identity() == self.started_with.identity())
                .then_some(credentials)
                .ok_or(())
        };
        match acquire.await {
            Ok(credentials) => QuotaCompletion {
                version: credentials.version().clone(),
                result: fetch(&credentials).await,
            },
            Err(()) => QuotaCompletion {
                version: self.started_with,
                result: Err("无法取得当前账号的额度查询凭据".into()),
            },
        }
    }
}

impl Account {
    pub fn accept_quota(&mut self, result: QuotaCompletion) -> Option<QuotaUpdate> {
        let QuotaCompletion { version, result } = result;
        if self.quota_version().as_ref() != Some(&version) {
            return None;
        }
        let identity = version.identity();
        let stale = match result {
            Ok(snapshot) => {
                self.quota = Some(snapshot);
                false
            }
            Err(failure) => {
                tracing::error!(reason = %failure, "Codex 账号额度刷新失败");
                true
            }
        };
        Some(QuotaUpdate {
            identity: identity.clone(),
            snapshot: self
                .quota
                .as_ref()
                .filter(|snapshot| &snapshot.identity == identity)
                .cloned(),
            stale,
        })
    }
}

#[derive(Deserialize)]
struct UsageResponse {
    rate_limit: Option<UsageWindows>,
}

#[derive(Deserialize)]
struct UsageWindows {
    primary_window: Option<UsageWindow>,
    secondary_window: Option<UsageWindow>,
}

#[derive(Deserialize)]
struct UsageWindow {
    limit_window_seconds: Option<u32>,
    used_percent: Option<f64>,
    reset_at: Option<i64>,
}

impl UsageWindow {
    fn project(self, duration: u32) -> Option<QuotaWindow> {
        let used_percent = self
            .used_percent
            .filter(|value| (0.0..=100.0).contains(value))?;
        Some(QuotaWindow {
            duration_minutes: duration / SECONDS_PER_MINUTE,
            used_percent,
            resets_at: self.reset_at.filter(|timestamp| *timestamp > 0),
        })
    }
}

async fn fetch(credentials: &UsableCredentials) -> Result<QuotaSnapshot, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| "无法创建安全的账号额度客户端".to_owned())?;
    let identity = credentials.version().identity();
    let mut response = client
        .get(USAGE_URL)
        .bearer_auth(credentials.access_token())
        .header("ChatGPT-Account-Id", &identity.workspace_id)
        .header("originator", CODEX_ORIGINATOR)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            concat!("Agentelier/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(|_| "无法连接 Codex 账号额度服务".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("额度服务返回 HTTP {}", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "无法接收账号额度响应".to_owned())?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("账号额度响应超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let data: UsageResponse =
        serde_json::from_slice(&bytes).map_err(|_| "账号额度响应格式无效".to_owned())?;
    let fetched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "无法取得额度获取时间".to_owned())?
        .as_secs();
    let mut snapshot = QuotaSnapshot {
        identity: identity.clone(),
        fetched_at,
        five_hours: None,
        week: None,
    };
    if let Some(windows) = data.rate_limit {
        for window in [windows.primary_window, windows.secondary_window]
            .into_iter()
            .flatten()
        {
            match window.limit_window_seconds {
                Some(FIVE_HOUR_SECONDS) if snapshot.five_hours.is_none() => {
                    snapshot.five_hours = window.project(FIVE_HOUR_SECONDS)
                }
                Some(WEEK_SECONDS) if snapshot.week.is_none() => {
                    snapshot.week = window.project(WEEK_SECONDS)
                }
                _ => {}
            }
        }
    }
    Ok(snapshot)
}
