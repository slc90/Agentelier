use crate::{
    MemoryClient, attention, classified,
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
use std::sync::Arc;

const MAX_SOURCES: usize = 12;
const MAX_PROJECT_BYTES: usize = 160;
// Two batches can be compared in one request within the previous revision-input budget.
const REVISION_BATCH_BYTES: usize = 128 * 1024;
use agentelier_llm_model::MAX_MEMORY_SUMMARY_BYTES;

pub struct Consolidation {
    pub(crate) previous_id: Option<String>,
    pub(crate) epoch: i64,
    pub(crate) read_generation: i64,
    pub(crate) details: BTreeMap<String, Detail>,
    pub(crate) deleted: Vec<String>,
    pub(crate) classified_hash: String,
    observations: Vec<Observation>,
    ended_attention: BTreeSet<String>,
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
pub struct ConsolidationExclusions {
    excluded_sources: BTreeSet<String>,
    excluded_observations: BTreeSet<String>,
}

impl ConsolidationExclusions {
    pub fn merge(&mut self, output: &str) -> io::Result<()> {
        if output.len() > MAX_MEMORY_SUMMARY_BYTES * 2 {
            return Err(io::Error::other("单批记忆整合结果过大"));
        }
        let batch: Self = serde_json::from_str(output)
            .map_err(|_| io::Error::other("全局记忆模型返回无效结构"))?;
        self.excluded_sources.extend(batch.excluded_sources);
        self.excluded_observations
            .extend(batch.excluded_observations);
        Ok(())
    }
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
            let (observation_hash, observations) = classified::inputs(connection, &sessions)?;
            let ended_attention = attention::ended_topics(connection, &sessions)?;
            let attention_hash =
                serde_json::to_string(&ended_attention).map_err(io::Error::other)?;
            let classified_hash = sha256(&format!("{observation_hash}\n{attention_hash}"));
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
                ended_attention,
                revisions,
            }))
        })
        .await
    }

    pub async fn publish_consolidation(
        &self,
        work: Consolidation,
        output: Option<ConsolidationExclusions>,
        published_at_ms: u64,
    ) -> io::Result<bool> {
        let content = work.content(output)?;
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

    pub fn model_inputs(&self) -> io::Result<impl Iterator<Item = String> + '_> {
        let mut batches = Vec::new();
        let mut start = 0;
        let mut bytes = 0;
        for (index, note) in self.revisions.iter().enumerate() {
            let size = serde_json::to_vec(note).map_err(io::Error::other)?.len() + 1;
            if bytes + size > REVISION_BATCH_BYTES && start < index {
                batches.push(&self.revisions[start..index]);
                start = index;
                bytes = 0;
            }
            bytes += size;
        }
        if start < self.revisions.len() {
            batches.push(&self.revisions[start..]);
        }
        let batches = Arc::new(batches);
        Ok((0..batches.len()).flat_map(move |index| {
            let batches = batches.clone();
            (0..=index).filter_map(move |earlier| {
                let current = batches[index];
                if !current.iter().any(|note| note.action != Action::Remember) {
                    return None;
                }
                if earlier == index {
                    return Some(self.model_input(current));
                }
                let mut targets = batches[earlier]
                    .iter()
                    .filter(|target| {
                        target.action != Action::Forget
                            && current.iter().any(|note| {
                                classified::affected(note, &classified::revision_scope(target))
                            })
                    })
                    .collect::<Vec<_>>();
                if targets.is_empty() {
                    return None;
                }
                targets.extend(current);
                Some(
                    serde_json::json!({
                        "sources": [],
                        "observations": [],
                        "revision_notes": targets,
                    })
                    .to_string(),
                )
            })
        }))
    }

    fn model_input(&self, notes: &[Revision]) -> String {
        let sources = self
            .details
            .iter()
            .filter(|(_, detail)| {
                notes
                    .iter()
                    .any(|note| note.affects_details(&detail.directory))
            })
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
                .filter(|item| notes.iter().any(|note| classified::affected(note, &item.scope)))
                .collect::<Vec<_>>(),
            "revision_notes": notes,
        })
        .to_string()
    }

    fn content(&self, output: Option<ConsolidationExclusions>) -> io::Result<PublicationContent> {
        let result = match output {
            Some(output) => output,
            None if !self.needs_model() => ConsolidationExclusions::default(),
            None => return Err(io::Error::other("整合结果缺失")),
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
            &self.ended_attention,
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
