use crate::{MemoryClient, redaction::Redactor};
use agentelier_conversation::{SteerState, TurnStatus, TurnView};
use agentelier_session_store::MemorySourceSnapshot;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::io;

const EVIDENCE_BYTES: usize = 36 * 1024;
const EVIDENCE_ITEM_BYTES: usize = 2 * 1024;
const SUMMARY_BYTES: usize = 8 * 1024;
const SLUG_BYTES: usize = 48;

pub struct SourceClaim {
    session: String,
    sequence: i64,
    revision: String,
    epoch: i64,
    cutoff: u64,
    attempts: u32,
}

impl SourceClaim {
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

pub enum SourceCompletion {
    Summary { text: String, slug: Option<String> },
    Empty,
    Retry { at_ms: u64 },
    Release,
}

impl MemoryClient {
    pub async fn claim_source(
        &self,
        source: &MemorySourceSnapshot,
        now_ms: u64,
    ) -> io::Result<Option<SourceClaim>> {
        let session = source.session.to_string();
        let sequence = sql_integer(source.sequence)?;
        let revision = source
            .revision
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let directory = source
            .work_directory
            .to_str()
            .ok_or_else(|| io::Error::other("记忆来源路径不是 UTF-8"))?
            .to_owned();
        let activity = sql_integer(source.activity_ms)?;
        let now_ms = sql_integer(now_ms)?;
        self.with_connection(move |connection| {
            let tx = connection.transaction()?;
            let (epoch, cutoff): (i64, i64) = tx.query_row(
                "SELECT epoch, COALESCE((SELECT sequence FROM reset_cutoffs WHERE session_id = ?1), 0) FROM memory_state WHERE singleton = 1",
                [&session], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if sequence <= cutoff { return Ok(None); }
            let attempts = tx.query_row(
                "INSERT INTO sources (session_id, sequence, revision, work_directory, activity_ms, status, attempts)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'processing', 1)
                 ON CONFLICT(session_id) DO UPDATE SET sequence = excluded.sequence, revision = excluded.revision,
                   work_directory = excluded.work_directory, activity_ms = excluded.activity_ms, status = 'processing',
                   summary = NULL, slug = NULL, retry_at_ms = NULL,
                   attempts = CASE WHEN sources.revision = excluded.revision THEN MIN(sources.attempts + 1, 32) ELSE 1 END
                 WHERE excluded.sequence >= sources.sequence AND (sources.revision <> excluded.revision OR sources.status = 'pending'
                   OR (sources.status = 'failed' AND sources.retry_at_ms <= ?6)) RETURNING attempts",
                params![session, sequence, revision, directory, activity, now_ms], |row| row.get(0),
            ).optional()?;
            tx.commit()?;
            Ok(attempts.map(|attempts| SourceClaim { session, sequence, revision, epoch, cutoff: cutoff.cast_unsigned(), attempts }))
        }).await
    }

    pub async fn complete_source(
        &self,
        claim: SourceClaim,
        completion: SourceCompletion,
    ) -> io::Result<bool> {
        let (status, summary, slug, retry) = match completion {
            SourceCompletion::Summary { text, slug } => ("summary", Some(text), slug, None),
            SourceCompletion::Empty => ("empty", None, None, None),
            SourceCompletion::Retry { at_ms } => ("failed", None, None, Some(sql_integer(at_ms)?)),
            SourceCompletion::Release => ("pending", None, None, None),
        };
        self.with_connection(move |connection| {
            let count = connection.execute(
                "UPDATE sources SET status = ?1, summary = ?2, slug = ?3, retry_at_ms = ?4,
                   attempts = CASE WHEN ?1 = 'pending' THEN attempts - 1 ELSE attempts END
                 WHERE session_id = ?5 AND sequence = ?6 AND revision = ?7 AND attempts = ?8 AND status = 'processing'
                   AND ?9 = (SELECT epoch FROM memory_state WHERE singleton = 1)",
                params![status, summary, slug, retry, claim.session, claim.sequence, claim.revision, claim.attempts, claim.epoch],
            )?;
            Ok(count == 1)
        }).await
    }
}

fn sql_integer(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(io::Error::other)
}

#[derive(Serialize)]
struct Evidence {
    role: &'static str,
    text: String,
}

pub fn extraction_prompt(
    source: &MemorySourceSnapshot,
    claim: &SourceClaim,
) -> io::Result<Option<String>> {
    let redactor = Redactor::new()?;
    let mut evidence = Vec::new();
    let mut used = 0;
    for (_, turn) in source
        .turns
        .iter()
        .rev()
        .take_while(|(sequence, _)| *sequence > claim.cutoff)
    {
        let mut items = turn_evidence(turn, &redactor);
        while let Some(item) = items.pop() {
            let size = serde_json::to_vec(&item).map_err(io::Error::other)?.len() + 1;
            if used + size > EVIDENCE_BYTES {
                break;
            }
            used += size;
            evidence.push(item);
        }
        if !items.is_empty() || used + EVIDENCE_ITEM_BYTES > EVIDENCE_BYTES {
            break;
        }
    }
    if evidence.is_empty() {
        return Ok(None);
    }
    evidence.reverse();
    Ok(Some(serde_json::json!({
        "session_id": source.session.to_string(),
        "project": redactor.clean(&source.work_directory.to_string_lossy(), EVIDENCE_ITEM_BYTES),
        "sequence": source.sequence,
        "messages": evidence,
    }).to_string()))
}

fn turn_evidence(turn: &TurnView, redactor: &Redactor) -> Vec<Evidence> {
    let item = |role, text: &str| Evidence {
        role,
        text: redactor.clean(text, EVIDENCE_ITEM_BYTES),
    };
    let mut items = vec![item("user", &turn.input)];
    let mut ordered = Vec::new();
    for steer in &turn.steers {
        let role = if steer.applied_request.is_some() {
            "user_applied_followup"
        } else if matches!(steer.state, SteerState::NotApplied) {
            "user_followup_not_applied"
        } else {
            "user_followup"
        };
        ordered.push((
            steer.applied_request.unwrap_or(u32::MAX),
            0,
            item(role, &steer.text),
        ));
    }
    for tool in turn
        .tools
        .iter()
        .filter(|tool| tool.retained && !crate::tools::is_memory_tool(&tool.name))
    {
        let text = format!("{} {}", tool.name, tool.output.as_deref().unwrap_or(""));
        ordered.push((
            tool.request_index,
            tool.output_index.saturating_add(1),
            item("tool_observation", &text),
        ));
    }
    if matches!(turn.status, TurnStatus::Completed) {
        for message in &turn.messages {
            for part in &message.content {
                ordered.push((
                    message.request_index,
                    message.output_index.saturating_add(1),
                    item("assistant", &part.text),
                ));
            }
        }
    }
    ordered.sort_by_key(|(request, output, _)| (*request, *output));
    items.extend(ordered.into_iter().map(|(_, _, evidence)| evidence));
    items.push(match &turn.status {
        TurnStatus::Completed => item("status", "completed"),
        TurnStatus::Cancelled => item("status", "cancelled"),
        TurnStatus::Limited => item("status", "budget limited"),
        TurnStatus::Failed(message) => item("failed", message),
        TurnStatus::Running => item("status", "running"),
    });
    items
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extracted {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    rollout_summary: Option<String>,
    slug: Option<String>,
}

pub fn parse_extraction(text: &str) -> io::Result<SourceCompletion> {
    if text.len() > SUMMARY_BYTES * 2 {
        return Err(io::Error::other("来源记忆响应超出大小限制"));
    }
    let result: Extracted = serde_json::from_str(text)
        .map_err(|_| io::Error::other("来源记忆响应缺少有效 JSON 结构"))?;
    let Some(summary) = result.rollout_summary else {
        if result.slug.is_some() {
            return Err(io::Error::other("空来源记忆携带了标识"));
        }
        return Ok(SourceCompletion::Empty);
    };
    let summary = summary.trim();
    if summary.is_empty() || summary.len() > SUMMARY_BYTES {
        return Err(io::Error::other("来源记忆正文为空或超出大小限制"));
    }
    let slug = result.slug.filter(|slug| {
        !slug.is_empty()
            && slug.len() <= SLUG_BYTES
            && slug
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    });
    Ok(SourceCompletion::Summary {
        text: Redactor::new()?.clean(summary, SUMMARY_BYTES),
        slug,
    })
}
