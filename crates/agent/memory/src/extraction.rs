use crate::{
    MemoryClient,
    observation::{
        Anchor, Candidate, Evidence, Location, Observation, Patterns, merge, patterns, validated,
    },
    redaction::Redactor,
};
use agentelier_conversation::{MemorySourceTurn, SteerState, TurnStatus};
use agentelier_session_store::MemorySourceSnapshot;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
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
    cutoff_turn: u64,
    attempts: u32,
}

impl SourceClaim {
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

pub enum SourceCompletion {
    Summary {
        text: String,
        slug: Option<String>,
        observations: Vec<Observation>,
    },
    Empty,
    Retry {
        at_ms: u64,
    },
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
        let directory_verified = source
            .turns
            .iter()
            .all(|turn| turn.work_directory == source.work_directory);
        let now_ms = sql_integer(now_ms)?;
        self.with_connection(move |connection| {
            let tx = connection.transaction()?;
            let (epoch, cutoff): (i64, i64) = tx.query_row(
                "SELECT epoch, COALESCE((SELECT sequence FROM reset_cutoffs WHERE session_id = ?1), 0) FROM memory_state WHERE singleton = 1",
                [&session], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if sequence <= cutoff { return Ok(None); }
            let cutoff_turn: i64 = tx.query_row(
                "SELECT COALESCE((SELECT last_turn FROM reset_turn_cutoffs WHERE session_id = ?1), 0)",
                [&session], |row| row.get(0),
            )?;
            let attempts = tx.query_row(
                "INSERT INTO sources (session_id, sequence, revision, work_directory, activity_ms, status, attempts, directory_verified)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'processing', 1, ?7)
                 ON CONFLICT(session_id) DO UPDATE SET sequence = excluded.sequence, revision = excluded.revision,
                   work_directory = excluded.work_directory, activity_ms = excluded.activity_ms, status = 'processing',
                   directory_verified = excluded.directory_verified,
                   summary = NULL, slug = NULL, retry_at_ms = NULL,
                   attempts = CASE WHEN sources.revision = excluded.revision THEN MIN(sources.attempts + 1, 32) ELSE 1 END
                 WHERE excluded.sequence >= sources.sequence AND (sources.revision <> excluded.revision OR sources.status = 'pending'
                   OR (sources.status = 'failed' AND sources.retry_at_ms <= ?6)) RETURNING attempts",
                params![session, sequence, revision, directory, activity, now_ms, directory_verified], |row| row.get(0),
            ).optional()?;
            if attempts.is_none() {
                tx.execute(
                    "UPDATE sources SET directory_verified = ?3
                     WHERE session_id = ?1 AND revision = ?2 AND status = 'summary' AND directory_verified <> ?3",
                    params![session, revision, directory_verified],
                )?;
            }
            tx.commit()?;
            Ok(attempts.map(|attempts| SourceClaim { session, sequence, revision, epoch, cutoff: cutoff.cast_unsigned(), cutoff_turn: cutoff_turn.cast_unsigned(), attempts }))
        }).await
    }

    pub async fn complete_source(
        &self,
        claim: SourceClaim,
        completion: SourceCompletion,
    ) -> io::Result<bool> {
        let (status, summary, slug, retry, observations) = match completion {
            SourceCompletion::Summary {
                text,
                slug,
                observations,
            } => ("summary", Some(text), slug, None, observations),
            SourceCompletion::Empty => ("empty", None, None, None, Vec::new()),
            SourceCompletion::Retry { at_ms } => {
                ("failed", None, None, Some(sql_integer(at_ms)?), Vec::new())
            }
            SourceCompletion::Release => ("pending", None, None, None, Vec::new()),
        };
        self.with_connection(move |connection| {
            let tx = connection.transaction()?;
            let count = tx.execute(
                "UPDATE sources SET status = ?1, summary = ?2, slug = ?3, retry_at_ms = ?4,
                   attempts = CASE WHEN ?1 = 'pending' THEN attempts - 1 ELSE attempts END
                 WHERE session_id = ?5 AND sequence = ?6 AND revision = ?7 AND attempts = ?8 AND status = 'processing'
                   AND ?9 = (SELECT epoch FROM memory_state WHERE singleton = 1)",
                params![status, summary, slug, retry, claim.session, claim.sequence, claim.revision, claim.attempts, claim.epoch],
            )?;
            if count == 1 && !observations.is_empty() {
                crate::attention::remember_boundaries(&tx, &observations)?;
                crate::experience::remember_objections(&tx, &observations)?;
                merge(&tx, &claim.session, observations)?;
            }
            tx.commit()?;
            Ok(count == 1)
        }).await
    }
}

fn sql_integer(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(io::Error::other)
}

pub struct Extraction {
    prompt: String,
    evidence: Vec<Evidence>,
}

impl Extraction {
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

pub fn extraction_prompt(
    source: &MemorySourceSnapshot,
    claim: &SourceClaim,
) -> io::Result<Option<Extraction>> {
    let redactor = Redactor::new()?;
    let patterns = patterns()?;
    let mut evidence = Vec::new();
    let mut used = 0;
    'turns: for source_turn in source.turns.iter().rev().filter(|turn| {
        turn.input_sequence > claim.cutoff && turn.turn.input_id.0 > claim.cutoff_turn
    }) {
        let mut items = turn_evidence(source_turn, &redactor, patterns, claim.cutoff == 0);
        while let Some(item) = items.pop() {
            let size = serde_json::to_vec(&item).map_err(io::Error::other)?.len() + 24;
            if used + size > EVIDENCE_BYTES {
                break 'turns;
            }
            used += size;
            evidence.push(item);
        }
        if used + EVIDENCE_ITEM_BYTES > EVIDENCE_BYTES {
            break;
        }
    }
    if evidence.is_empty() {
        return Ok(None);
    }
    evidence.reverse();
    let messages = evidence
        .iter()
        .enumerate()
        .map(|(id, item)| {
            serde_json::json!({
                "id": id,
                "turn": item.anchor.as_ref().map(|anchor| anchor.turn),
                "project": item.project,
                "fictional_context": item.fictional_context,
                "role": item.role,
                "text": item.text,
            })
        })
        .collect::<Vec<_>>();
    let prompt = serde_json::json!({
        "session_id": source.session.to_string(),
        "project": redactor.clean(&source.work_directory.to_string_lossy(), EVIDENCE_ITEM_BYTES),
        "origin": source.origin,
        "sequence": source.sequence,
        "messages": messages,
    })
    .to_string();
    Ok(Some(Extraction { prompt, evidence }))
}

fn turn_evidence(
    source: &MemorySourceTurn,
    redactor: &Redactor,
    patterns: &Patterns,
    include_assistant: bool,
) -> Vec<Evidence> {
    let turn = &source.turn;
    let project = redactor.clean(
        &source.work_directory.to_string_lossy(),
        EVIDENCE_ITEM_BYTES,
    );
    let item = |role, text: &str, anchor| Evidence {
        role,
        text: redactor.clean(text, EVIDENCE_ITEM_BYTES),
        anchor,
        fictional_context: false,
        project: project.clone(),
        work_directory: source.work_directory.clone(),
    };
    let anchor = |sequence, location| {
        Some(Anchor {
            turn: turn.id,
            input: Some(turn.input_id),
            sequence,
            at_ms: source.activity_ms,
            location,
        })
    };
    let mut items = Vec::new();
    if !turn.result_continuation {
        items.push(item(
            "user",
            &turn.input,
            anchor(source.input_sequence, Location::Input),
        ));
    }
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
            item(
                role,
                &steer.text,
                steer.applied_request.and_then(|_| {
                    anchor(
                        source.finished_sequence,
                        Location::Followup { id: steer.id },
                    )
                }),
            ),
        ));
    }
    for tool in turn.tools.iter().filter(|tool| {
        tool.retained && !tool.collaboration && !crate::tools::is_memory_tool(&tool.name)
    }) {
        let text = format!(
            "{} [{:?}] {}",
            tool.name,
            tool.status,
            tool.output.as_deref().unwrap_or("")
        );
        ordered.push((
            tool.request_index,
            tool.output_index.saturating_add(1),
            item(
                "tool_observation",
                &text,
                source
                    .tool_sequences
                    .get(&tool.call_id)
                    .and_then(|sequence| {
                        anchor(
                            *sequence,
                            Location::Tool {
                                call: tool.call_id.clone(),
                                status: tool.status,
                            },
                        )
                    }),
            ),
        ));
    }
    if include_assistant && matches!(turn.status, TurnStatus::Completed) {
        for message in turn
            .messages
            .iter()
            .filter(|message| message.phase.as_deref() != Some("commentary"))
        {
            for part in &message.content {
                ordered.push((
                    message.request_index,
                    message.output_index.saturating_add(1),
                    item(
                        "assistant",
                        &part.text,
                        anchor(
                            source.finished_sequence,
                            Location::Reply {
                                message: message.id.clone(),
                                content: part.content_index,
                            },
                        ),
                    ),
                ));
            }
        }
    }
    ordered.sort_by_key(|(request, output, _)| (*request, *output));
    items.extend(ordered.into_iter().map(|(_, _, evidence)| evidence));
    items.extend(turn.process_cleanup.iter().map(|process| {
        let text = format!(
            "进程 {} 回收已确认={} {}",
            process.session_id, process.cleanup_confirmed, process.output
        );
        item("process_observation", &text, None)
    }));
    items.push(match &turn.status {
        TurnStatus::Completed => item("status", "completed", None),
        TurnStatus::Cancelled => item("status", "cancelled", None),
        TurnStatus::Limited => item("status", "budget limited", None),
        TurnStatus::Failed(message) => item("failed", message, None),
        TurnStatus::Running => item("status", "running", None),
    });
    let fictional_context = items.iter().any(|item| {
        matches!(item.role, "user" | "user_applied_followup")
            && patterns.fictional.is_match(&item.text)
    });
    if fictional_context {
        items
            .iter_mut()
            .for_each(|item| item.fictional_context = true);
    }
    items
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extracted {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    rollout_summary: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    observations: Vec<Candidate>,
}

impl Extraction {
    pub fn complete(
        &self,
        source: &MemorySourceSnapshot,
        text: &str,
    ) -> io::Result<SourceCompletion> {
        if text.len() > SUMMARY_BYTES * 3 {
            return Err(io::Error::other("来源记忆响应超出大小限制"));
        }
        let result: Extracted = serde_json::from_str(text)
            .map_err(|_| io::Error::other("来源记忆响应缺少有效 JSON 结构"))?;
        let redactor = Redactor::new()?;
        let observations = validated(
            source,
            &self.evidence,
            result.observations,
            &redactor,
            patterns()?,
        );
        let Some(summary) = result.rollout_summary else {
            if result.slug.is_some() {
                return Err(io::Error::other("空来源记忆携带了标识"));
            }
            return Ok(if observations.is_empty() {
                SourceCompletion::Empty
            } else {
                SourceCompletion::Summary {
                    text: "此来源保存了有证据的结构化观察。".to_owned(),
                    slug: None,
                    observations,
                }
            });
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
            text: redactor.clean(summary, SUMMARY_BYTES),
            slug,
            observations,
        })
    }
}
