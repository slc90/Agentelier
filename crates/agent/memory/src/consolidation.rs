use crate::{MemoryClient, publication, redaction::Redactor};
use agentelier_conversation::SessionId;
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::io;

const MAX_SOURCES: usize = 12;
const MAX_SOURCE_EXCERPT_BYTES: usize = 2 * 1024;
const MAX_PREVIOUS_EXCERPT_BYTES: usize = 4 * 1024;
const MAX_PROJECT_BYTES: usize = 160;
pub(crate) const MAX_SUMMARY_BYTES: usize = 10 * 1024;

pub struct Consolidation {
    pub(crate) previous_id: Option<String>,
    pub(crate) epoch: i64,
    pub(crate) details: BTreeMap<String, Detail>,
    pub(crate) deleted: Vec<String>,
    previous_summary: String,
    changes: Changes,
}

pub(crate) struct Detail {
    pub(crate) revision: String,
    pub(crate) hash: String,
    pub(crate) name: String,
    pub(crate) content: String,
    project: String,
    date: time::Date,
    excerpt: String,
}

#[derive(Default, serde::Serialize)]
struct Changes {
    added: Vec<String>,
    updated: Vec<String>,
    removed: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsolidatedText {
    summary: String,
}

impl MemoryClient {
    pub async fn prepare_consolidation(
        &self,
        sessions: &[SessionId],
    ) -> io::Result<Option<Consolidation>> {
        let sessions = sessions
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        self.with_store(move |connection, workspace| {
            let baseline = publication::restore(connection, workspace)?;
            let details = select(connection, &sessions)?;
            let mut changes = Changes::default();
            for (session_id, detail) in &details {
                match baseline.sources.get(session_id) {
                    None => changes.added.push(session_id.clone()),
                    Some((revision, hash))
                        if *revision != detail.revision || *hash != detail.hash =>
                    {
                        changes.updated.push(session_id.clone())
                    }
                    Some(_) => {}
                }
            }
            changes.removed.extend(
                baseline
                    .sources
                    .keys()
                    .filter(|id| !details.contains_key(*id))
                    .cloned(),
            );
            if changes.added.is_empty() && changes.updated.is_empty() && changes.removed.is_empty()
            {
                return Ok(None);
            }
            let epoch = connection
                .query_row(
                    "SELECT epoch FROM memory_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            let deleted = changes
                .removed
                .iter()
                .filter(|id| !sessions.contains(*id))
                .cloned()
                .collect();
            Ok(Some(Consolidation {
                previous_id: baseline.id,
                epoch,
                details,
                deleted,
                previous_summary: baseline.summary,
                changes,
            }))
        })
        .await
    }

    pub async fn publish_consolidation(
        &self,
        work: Consolidation,
        output: Option<String>,
        published_at_ms: u64,
    ) -> io::Result<bool> {
        let summary = work.summary(output.as_deref())?;
        let published_at_ms = i64::try_from(published_at_ms).map_err(io::Error::other)?;
        self.with_store(move |connection, workspace| {
            publication::publish(connection, workspace, work, summary, published_at_ms)
        })
        .await
    }
}

impl Consolidation {
    pub fn source_count(&self) -> usize {
        self.details.len()
    }

    pub fn model_input(&self) -> String {
        let sources = self
            .details
            .iter()
            .map(|(id, detail)| {
                serde_json::json!({
                    "session_id": id,
                    "project": detail.project,
                    "date": detail.date.to_string(),
                    "record": detail.name,
                    "evidence": detail.excerpt,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "previous_summary": &self.previous_summary[..self.previous_summary.floor_char_boundary(MAX_PREVIOUS_EXCERPT_BYTES)],
            "changes": self.changes,
            "sources": sources,
            "revision_notes": [],
        }).to_string()
    }

    fn summary(&self, output: Option<&str>) -> io::Result<Option<String>> {
        if self.details.is_empty() && output.is_none() {
            return Ok(None);
        }
        let output = output.ok_or_else(|| io::Error::other("整合结果缺少模型正文"))?;
        if output.len() > MAX_SUMMARY_BYTES * 2 {
            return Err(io::Error::other("全局记忆模型响应超出大小限制"));
        }
        let result: ConsolidatedText = serde_json::from_str(output)
            .map_err(|_| io::Error::other("全局记忆模型返回无效结构"))?;
        let body = result.summary.trim();
        if body.is_empty() || body.len() > MAX_SUMMARY_BYTES {
            return Err(io::Error::other("全局记忆摘要为空或超出大小限制"));
        }
        let mut index = String::from("\n\n## 来源会话\n\n");
        for (id, detail) in &self.details {
            writeln!(
                index,
                "- [{}](rollout_summaries/{}) · {} · {}",
                id, detail.name, detail.date, detail.project
            )
            .map_err(io::Error::other)?;
        }
        let body = Redactor::new()?.clean(body, MAX_SUMMARY_BYTES);
        let summary = format!("{body}{index}");
        if summary.len() > MAX_SUMMARY_BYTES {
            return Err(io::Error::other("全局记忆摘要与来源导航合计超过 10 KiB"));
        }
        Ok(Some(summary))
    }
}

fn select(
    connection: &Connection,
    sessions: &BTreeSet<String>,
) -> io::Result<BTreeMap<String, Detail>> {
    let redactor = Redactor::new()?;
    let mut statement = connection.prepare(
        "SELECT session_id, sequence, revision, work_directory, activity_ms, summary FROM sources
         WHERE status = 'summary' ORDER BY max(activity_ms, COALESCE(last_usage_ms, activity_ms)) DESC,
         usage_count DESC, activity_ms DESC, session_id",
    ).map_err(io::Error::other)?;
    let mut selected = BTreeMap::new();
    let mut rows = statement.query([]).map_err(io::Error::other)?;
    while selected.len() < MAX_SOURCES
        && let Some(row) = rows.next().map_err(io::Error::other)?
    {
        let id: String = row.get(0).map_err(io::Error::other)?;
        if !sessions.contains(&id) {
            continue;
        }
        let sequence: i64 = row.get(1).map_err(io::Error::other)?;
        let revision: String = row.get(2).map_err(io::Error::other)?;
        let project: String = row.get(3).map_err(io::Error::other)?;
        let timestamp_ms: i64 = row.get(4).map_err(io::Error::other)?;
        let summary: String = row.get(5).map_err(io::Error::other)?;
        let date = time::OffsetDateTime::from_unix_timestamp(timestamp_ms / 1_000)
            .map_err(io::Error::other)?
            .date();
        let project = redactor.clean(&project.replace(['\n', '\r'], " "), MAX_PROJECT_BYTES);
        let content = format!(
            "# 会话 {id}\n\n- 项目目录 `{project}`\n- 日期 {date}\n- 已处理日志序号 {sequence}\n- 来源修订 `{revision}`\n\n{summary}\n"
        );
        let hash = sha256(&content);
        let name = format!("{id}-{hash}.md");
        selected.insert(
            id,
            Detail {
                revision,
                hash,
                name,
                content,
                project,
                date,
                excerpt: redactor.clean(&summary, MAX_SOURCE_EXCERPT_BYTES),
            },
        );
    }
    Ok(selected)
}

pub(crate) fn sha256(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
