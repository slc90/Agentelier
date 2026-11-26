use crate::{
    MemoryClient, classified,
    observation::Observation,
    publication,
    redaction::Redactor,
    revision::{self, Action, Revision},
};
use agentelier_conversation::SessionId;
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::io;

const MAX_SOURCES: usize = 12;
const MAX_PROJECT_BYTES: usize = 160;
use agentelier_llm_model::MAX_MEMORY_SUMMARY_BYTES;

pub struct Consolidation {
    pub(crate) previous_id: Option<String>,
    pub(crate) epoch: i64,
    pub(crate) read_generation: i64,
    pub(crate) details: BTreeMap<String, Detail>,
    pub(crate) deleted: Vec<String>,
    pub(crate) classified_hash: String,
    observations: Vec<Observation>,
    pub(crate) revisions: Vec<Revision>,
}

pub(crate) struct Detail {
    pub(crate) revision: String,
    pub(crate) hash: String,
    pub(crate) name: String,
    pub(crate) content: String,
    pub(crate) directory: String,
    pub(crate) sequence: i64,
    pub(crate) activity_ms: i64,
    pub(crate) directory_verified: bool,
    date: time::Date,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsolidatedText {
    excluded_sources: BTreeSet<String>,
    excluded_observations: BTreeSet<String>,
}

pub(crate) struct PublicationContent {
    pub(crate) summary: Option<String>,
    pub(crate) excluded: BTreeSet<String>,
    pub(crate) classified: Vec<classified::Entry>,
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
            let (read_generation, published_generation) =
                crate::store::generations(connection).map_err(io::Error::other)?;
            let details = select(connection, &sessions)?;
            let (classified_hash, observations) = classified::inputs(connection, &sessions)?;
            let epoch = connection
                .query_row(
                    "SELECT epoch FROM memory_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            let revisions = revision::revisions(workspace, epoch)?;
            let published = revision::published_ids(connection)?;
            let all_revisions_published = revisions.len() == published.len()
                && revisions.iter().all(|note| published.contains(&note.id));
            let unchanged = details.len() == baseline.sources.len()
                && details.iter().all(|(id, detail)| {
                    baseline.sources.get(id).is_some_and(|(revision, hash)| {
                        *revision == detail.revision && *hash == detail.hash
                    })
                });
            let mut statement = connection
                .prepare("SELECT session_id FROM sources")
                .map_err(io::Error::other)?;
            let deleted = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?
                .filter(|id| id.as_ref().map_or(true, |id| !sessions.contains(id)))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(io::Error::other)?;
            if read_generation == published_generation
                && all_revisions_published
                && unchanged
                && baseline.classified_hash.as_deref() == Some(&classified_hash)
                && deleted.is_empty()
            {
                return Ok(None);
            }
            Ok(Some(Consolidation {
                previous_id: baseline.id,
                epoch,
                read_generation,
                details,
                deleted,
                classified_hash,
                observations,
                revisions,
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
        let content = work.content(output.as_deref())?;
        let published_at_ms = i64::try_from(published_at_ms).map_err(io::Error::other)?;
        self.with_store(move |connection, workspace| {
            publication::publish(connection, workspace, work, content, published_at_ms)
        })
        .await
    }
}

impl Consolidation {
    pub fn source_count(&self) -> usize {
        self.details.len()
    }

    pub fn needs_model(&self) -> bool {
        self.revisions
            .iter()
            .any(|note| note.action != Action::Remember)
            && (!self.details.is_empty()
                || !self.observations.is_empty()
                || self.revisions.len() > 1)
    }

    fn corrective_revision_applies(&self, detail: &Detail) -> bool {
        self.revisions
            .iter()
            .any(|note| note.affects_details(&detail.directory))
    }

    pub fn model_input(&self) -> String {
        let sources = self
            .details
            .iter()
            .filter(|(_, detail)| self.corrective_revision_applies(detail))
            .map(|(id, detail)| {
                serde_json::json!({
                    "session_id": id,
                    "evidence": detail.content,
                    "work_directory": detail.directory,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "sources": sources,
            "observations": self.observations.iter()
                .filter(|item| self.revisions.iter().any(|note| classified::affected(note, &item.scope)))
                .collect::<Vec<_>>(),
            "revision_notes": self.revisions,
        }).to_string()
    }

    fn content(&self, output: Option<&str>) -> io::Result<PublicationContent> {
        let result: ConsolidatedText = match output {
            Some(output) if output.len() <= MAX_MEMORY_SUMMARY_BYTES * 2 => {
                serde_json::from_str(output)
                    .map_err(|_| io::Error::other("全局记忆模型返回无效结构"))?
            }
            None if !self.needs_model() => ConsolidatedText::default(),
            _ => return Err(io::Error::other("整合结果缺失或超出大小限制")),
        };
        let mut excluded = result.excluded_sources;
        if excluded.iter().any(|id| {
            self.details
                .get(id)
                .is_none_or(|detail| !self.corrective_revision_applies(detail))
        }) {
            return Err(io::Error::other("整合结果排除了修订范围外的来源"));
        }
        if result.excluded_observations.iter().any(|id| {
            let observation = self.observations.iter().any(|item| {
                &item.id == id
                    && self
                        .revisions
                        .iter()
                        .any(|note| classified::affected(note, &item.scope))
            });
            let revision = self.revisions.iter().enumerate().any(|(index, note)| {
                &note.id == id
                    && note.action != Action::Forget
                    && self.revisions[index + 1..]
                        .iter()
                        .any(|later| classified::affected(later, &classified::revision_scope(note)))
            });
            !observation && !revision
        }) {
            return Err(io::Error::other("整合结果排除了修订范围外的观察"));
        }
        for (id, detail) in &self.details {
            let contents = detail.content.to_lowercase();
            if self.revisions.iter().any(|note| {
                note.affects_details(&detail.directory)
                    && contents.contains(&note.target.to_lowercase())
            }) {
                excluded.insert(id.clone());
            }
        }
        let classified = classified::classify(
            &self.observations,
            &self.revisions,
            &result.excluded_observations,
        );
        let mut summary = classified::personal_navigation(&classified)?;
        if self.details.len() > excluded.len() {
            summary.push_str("\n\n## 项目工作记忆\n\n");
        }
        for (id, detail) in &self.details {
            if excluded.contains(id) {
                continue;
            }
            writeln!(
                summary,
                "- [项目来源 {}](rollout_summaries/{}) · {}",
                id, detail.name, detail.date
            )
            .map_err(io::Error::other)?;
        }
        if summary.trim().is_empty() && !classified.is_empty() {
            summary.push_str("## 来源记忆\n\n已发布当前可按范围检索的分类记录。");
        }
        if summary.len() > MAX_MEMORY_SUMMARY_BYTES {
            return Err(io::Error::other("全局记忆摘要与来源导航合计超过 10 KiB"));
        }
        Ok(PublicationContent {
            summary: (!summary.trim().is_empty()).then_some(summary),
            excluded,
            classified,
        })
    }
}

fn select(
    connection: &Connection,
    sessions: &BTreeSet<String>,
) -> io::Result<BTreeMap<String, Detail>> {
    let redactor = Redactor::new()?;
    let mut statement = connection.prepare(
        "SELECT session_id, sequence, revision, work_directory, activity_ms, summary, directory_verified FROM sources
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
        let directory: String = row.get(3).map_err(io::Error::other)?;
        let timestamp_ms: i64 = row.get(4).map_err(io::Error::other)?;
        let summary: String = row.get(5).map_err(io::Error::other)?;
        let directory_verified = row.get(6).map_err(io::Error::other)?;
        let date = time::OffsetDateTime::from_unix_timestamp(timestamp_ms / 1_000)
            .map_err(io::Error::other)?
            .date();
        let project = redactor.clean(&directory.replace(['\n', '\r'], " "), MAX_PROJECT_BYTES);
        let scope = if directory_verified {
            "已核对为单一目录"
        } else {
            "仅供管理检查"
        };
        let content = format!(
            "# 会话 {id}\n\n- 项目目录 `{project}`\n- 目录范围 {scope}\n- 日期 {date}\n- 已处理日志序号 {sequence}\n- 来源修订 `{revision}`\n\n{summary}\n"
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
                directory,
                sequence,
                activity_ms: timestamp_ms,
                directory_verified,
                date,
            },
        );
    }
    Ok(selected)
}

pub(crate) fn sha256(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
