use crate::{
    MemoryClient,
    attention::Attention,
    classified,
    consolidation::sha256,
    experience::{Experience, ExperienceState},
    observation::{Category, EvidenceKind, Scope},
    publication::{DETAILS, detail_name, read_checked},
    redaction::Redactor,
    store::check_path,
};
use agentelier_conversation::{MemoryWithdrawal, SessionId};
use agentelier_llm_model::{
    MAX_MEMORY_SUMMARY_BYTES, MemoryReference, MemoryScope, MemorySnapshot,
};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_QUERY_BYTES: usize = 256;
const MAX_MATCHES: usize = 5;
const MAX_EXCERPT_BYTES: usize = 640;
const MAX_SCOPE_DISPLAY_BYTES: usize = 512;
const MAX_INSPECTION_PREVIEW_BYTES: usize = 210;
const MAX_DETAIL_BYTES: usize = 9 * 1024;
const INSPECTION_CATEGORIES: [(Category, &str); 7] = [
    (Category::UserPreference, "用户偏好"),
    (Category::SharedExperience, "共同经历"),
    (Category::OriginSelfObservation, "Origin 自我观察"),
    (Category::AttentionClue, "当前关注"),
    (Category::ExperienceClue, "经验"),
    (Category::WorkFact, "项目工作记忆"),
    (Category::UserRevision, "用户修订"),
];
const SOURCE_QUERY: &str = "SELECT p.session_id, p.revision, p.content_hash, p.file_name,
        COALESCE(m.work_directory, s.work_directory), COALESCE(m.sequence, s.sequence),
        COALESCE(m.activity_ms, s.activity_ms), a.content, COALESCE(m.directory_verified, 0),
        (SELECT publication_id FROM publication)
    FROM published_sources p
    LEFT JOIN published_source_metadata m ON m.session_id = p.session_id
    LEFT JOIN sources s ON s.session_id = p.session_id AND s.revision = p.revision
    LEFT JOIN published_artifacts a ON a.file_name = p.file_name
    WHERE (?1 IS NULL OR p.file_name = ?1)
    ORDER BY COALESCE(m.activity_ms, s.activity_ms) DESC, p.session_id";

#[derive(Clone, Serialize)]
pub struct MemorySource {
    pub session_id: SessionId,
    pub record: String,
    pub revision: String,
    pub directory: Option<String>,
    pub sequence: Option<i64>,
    pub activity_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<Scope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<classified::Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<Box<classified::Provenance>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attention: Option<Box<Attention>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    experience: Option<Box<Experience>>,
}

#[derive(Serialize)]
pub struct MemoryMatch {
    source: MemorySource,
    excerpt: String,
}

#[derive(Clone, Serialize)]
pub struct MemoryDetail {
    pub source: MemorySource,
    pub text: String,
}

#[derive(Clone)]
pub struct PublishedSummary {
    pub id: String,
    pub text: String,
    pub published_at_ms: i64,
}

#[derive(Clone)]
pub struct MemoryInspection {
    pub summary: Option<PublishedSummary>,
    pub groups: Vec<MemoryInspectionGroup>,
    pub processing: bool,
    pub pending_sources: i64,
    pub failed_sources: i64,
    pub pending_revisions: usize,
    pub notice: Option<&'static str>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MemoryInspectionGroup {
    pub title: &'static str,
    pub records: Vec<MemoryInspectionRecord>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MemoryInspectionRecord {
    pub record: String,
    pub session_id: SessionId,
    pub activity_ms: Option<i64>,
    pub scope: &'static str,
    pub directory: Option<String>,
    pub status: &'static str,
    pub provenance: String,
    pub preview: String,
}

struct IndexedSource {
    source: MemorySource,
    scope: Option<PathBuf>,
    hash: String,
    content: String,
}

pub struct ReadView {
    pub snapshot: MemorySnapshot,
    records: Vec<SearchRecord>,
}

struct SearchRecord {
    detail: MemoryDetail,
    body: String,
}

type ViewHandle = Arc<dyn std::any::Any + Send + Sync>;

fn active_view<'a>(
    handle: &'a Option<ViewHandle>,
    directory: &Path,
) -> io::Result<Option<&'a ReadView>> {
    match handle {
        None => Ok(None),
        Some(current) => current
            .downcast_ref::<ReadView>()
            .filter(|view| {
                view.snapshot
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.directory == directory)
            })
            .map(Some)
            .ok_or_else(invisible),
    }
}

fn current_generation(connection: &Connection) -> io::Result<i64> {
    let (current, published) = crate::store::generations(connection).map_err(io::Error::other)?;
    if current != published {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "长期记忆修订等待发布",
        ));
    }
    Ok(current)
}

fn bound_generation(connection: &Connection, view: &ReadView) -> io::Result<()> {
    let (current, _) = crate::store::generations(connection).map_err(io::Error::other)?;
    if view
        .snapshot
        .scope
        .as_ref()
        .is_none_or(|scope| scope.generation != current)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "此轮长期记忆发布已撤下",
        ));
    }
    Ok(())
}

fn records(
    connection: &Connection,
    workspace: &Path,
    directory: Option<&Path>,
    input: Option<&str>,
) -> io::Result<Vec<SearchRecord>> {
    let mut result = Vec::new();
    if let Some(publication) = current_publication(connection)? {
        for (name, hash, entry) in classified::saved(connection, None)? {
            if !entry.visible_from(directory)
                || entry.body.is_empty()
                || entry.category == Category::AttentionClue
                    && input.is_some_and(|input| {
                        entry
                            .attention
                            .as_ref()
                            .is_none_or(|attention| !attention.related(input))
                    })
                || entry.category == Category::ExperienceClue
                    && input.is_some_and(|input| {
                        entry
                            .experience
                            .as_ref()
                            .is_none_or(|experience| !experience.related(input))
                    })
            {
                continue;
            }
            let body = entry.experience.as_ref().map_or_else(
                || entry.body.clone(),
                |experience| format!("{}\n{}", experience.topic, entry.body),
            );
            let detail = classified_detail(workspace, name, hash, entry, publication.clone())?;
            result.push(SearchRecord { detail, body });
        }
    }
    for indexed in sources(connection, None)? {
        if directory.is_some_and(|directory| indexed.scope.as_deref() != Some(directory)) {
            continue;
        }
        let body = indexed
            .content
            .split_once("\n\n")
            .and_then(|(_, rest)| rest.split_once("\n\n"))
            .map_or(indexed.content.as_str(), |(_, body)| body)
            .to_owned();
        let detail = detail(workspace, indexed)?;
        result.push(SearchRecord { detail, body });
    }
    Ok(result)
}

fn search_records(
    records: &[SearchRecord],
    query: &str,
    cancel: &CancellationToken,
) -> io::Result<Vec<MemoryMatch>> {
    let mut category = None;
    let mut scope = None;
    let mut words = Vec::new();
    for word in query.split_whitespace() {
        if let Some(value) = word.strip_prefix("category:") {
            category = Some(value);
        } else if let Some(value) = word.strip_prefix("scope:") {
            scope = Some(value);
        } else {
            words.push(word);
        }
    }
    let mut matches = Vec::new();
    for entry in records {
        check_cancel(cancel)?;
        let source = &entry.detail.source;
        let scope_name = match &source.scope {
            Some(Scope::Context) => "context",
            Some(Scope::Global) => "global",
            _ => "project",
        };
        if category.is_some_and(|category| {
            source
                .category
                .is_none_or(|value| category != value.label())
        }) || scope.is_some_and(|scope| scope != scope_name)
            || !contains_words(&entry.body, &words)
        {
            continue;
        }
        let excerpt = entry
            .body
            .lines()
            .find(|line| words.iter().any(|word| line.to_lowercase().contains(word)))
            .unwrap_or(&entry.body);
        matches.push(MemoryMatch {
            source: source.clone(),
            excerpt: excerpt[..excerpt.floor_char_boundary(MAX_EXCERPT_BYTES)].to_owned(),
        });
        if matches.len() == MAX_MATCHES {
            break;
        }
    }
    Ok(matches)
}

impl MemoryClient {
    pub async fn freeze(
        &self,
        directory: PathBuf,
        input: String,
    ) -> io::Result<Option<Arc<ReadView>>> {
        self.with_active_store(move |connection, workspace| {
            let generation = current_generation(connection)?;
            if current_publication(connection)?.is_none() {
                return Ok(None);
            }
            let mut snapshot = crate::publication::committed_snapshot(connection)?;
            let records = records(connection, workspace, Some(&directory), Some(&input))?;
            let mut references = Vec::new();
            let mut navigation = snapshot
                .summary
                .split_once("\n\n## 项目工作记忆")
                .map_or(snapshot.summary.as_str(), |(global, _)| global)
                .trim()
                .to_owned();
            let mut project_heading = false;
            for entry in &records {
                let source = &entry.detail.source;
                let experience = source.experience.as_deref();
                if experience.is_some_and(|value| value.state != ExperienceState::Supported) {
                    continue;
                }
                let global = source.scope == Some(Scope::Global);
                let recorded_in_summary = navigation.contains(&source.record);
                if !global || source.attention.is_some() || experience.is_some() {
                    let label = source.category.map_or("工作摘要", Category::label);
                    let label = match (source.attention.as_deref(), experience) {
                        (Some(attention), _) => attention.navigation(),
                        (_, Some(experience)) => experience.navigation(),
                        _ => label.to_owned(),
                    };
                    let line = format!(
                        "- {label} · [{}](rollout_summaries/{})\n",
                        source.session_id, source.record
                    );
                    if navigation.len() + line.len() + 80 > MAX_MEMORY_SUMMARY_BYTES {
                        break;
                    }
                    if !project_heading {
                        navigation.push_str("\n\n## 当前工作目录的记忆\n\n");
                        project_heading = true;
                    }
                    navigation.push_str(&line);
                } else if !recorded_in_summary {
                    continue;
                }
                references.push(MemoryReference {
                    session: source.session_id,
                    record: source.record.clone(),
                    revision: source.revision.clone(),
                    category: source.category.map(|category| category.label().to_owned()),
                    global,
                });
            }
            if navigation.trim().is_empty() {
                navigation.push_str("当前发布没有适用于此工作目录的记忆正文。");
            }
            snapshot.hash = sha256(&navigation);
            snapshot.summary = navigation;
            snapshot.scope = Some(MemoryScope {
                generation,
                directory,
                sources: references,
            });
            snapshot.validate().map_err(io::Error::other)?;
            Ok(Some(Arc::new(ReadView { snapshot, records })))
        })
        .await
    }

    pub async fn view_withdrawal(&self, view: Arc<ReadView>) -> Option<MemoryWithdrawal> {
        let epoch = view.snapshot.epoch;
        if self.epoch() != Some(epoch) {
            return Some(MemoryWithdrawal::EpochChanged);
        }
        let valid = self
            .with_active_store(move |connection, _| {
                let (current, _) =
                    crate::store::generations(connection).map_err(io::Error::other)?;
                Ok(view
                    .snapshot
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.generation == current))
            })
            .await;
        match valid {
            Ok(true) => None,
            Ok(false) => Some(MemoryWithdrawal::PublicationRevoked),
            Err(_) => Some(MemoryWithdrawal::StorageUnavailable),
        }
    }
    pub async fn inspect(&self, processing: bool) -> io::Result<MemoryInspection> {
        self.with_active_store(move |connection, workspace| {
            let (generation, published) =
                crate::store::generations(connection).map_err(io::Error::other)?;
            let (snapshot, mut notice) = if generation != published {
                (None, Some("已撤回旧发布，等待新的记忆发布。"))
            } else {
                match crate::publication::snapshot(connection, workspace) {
                    Ok(snapshot) => (snapshot, None),
                    Err(failure) => {
                        tracing::warn!(error = %failure, "读取摘要文件失败，尝试已提交的摘要副本");
                        (
                            Some(crate::publication::committed_snapshot(connection)?),
                            Some("读取已发布摘要文件失败，当前显示记忆库中已提交的副本。"),
                        )
                    }
                }
            };
            let epoch: i64 = connection
                .query_row(
                    "SELECT epoch FROM memory_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            let pending_revisions =
                match crate::revision::revisions(workspace, epoch).and_then(|revisions| {
                    let published = crate::revision::published_ids(connection)?;
                    Ok(revisions
                        .iter()
                        .filter(|revision| !published.contains(&revision.id))
                        .count())
                }) {
                    Ok(count) => count,
                    Err(failure) => {
                        tracing::warn!(error = %failure, "记忆修订待发布状态不可用");
                        notice.get_or_insert("无法核对待整合的记忆修订。");
                        0
                    }
                };
            let (pending_sources, failed_sources) = connection
                .query_row(
                    "SELECT COUNT(*) FILTER (WHERE status IN ('pending', 'processing')),
                     COUNT(*) FILTER (WHERE status = 'failed') FROM sources",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(io::Error::other)?;
            let (summary, records) = if let Some(snapshot) = snapshot {
                let published_at_ms = connection
                    .query_row("SELECT published_at_ms FROM publication", [], |row| {
                        row.get(0)
                    })
                    .map_err(io::Error::other)?;
                let records = inspection_records(connection)?;
                let redactor = Redactor::new()?;
                (
                    Some(PublishedSummary {
                        id: snapshot.publication_id.to_string(),
                        text: redactor.clean(&snapshot.summary, MAX_MEMORY_SUMMARY_BYTES),
                        published_at_ms,
                    }),
                    records,
                )
            } else {
                (None, BTreeMap::new())
            };
            let mut records = records;
            let groups = INSPECTION_CATEGORIES
                .into_iter()
                .map(|(category, title)| MemoryInspectionGroup {
                    title,
                    records: records.remove(&category).unwrap_or_default(),
                })
                .collect();
            Ok(MemoryInspection {
                summary,
                groups,
                processing,
                pending_sources,
                failed_sources,
                pending_revisions,
                notice,
            })
        })
        .await
    }

    pub async fn inspect_detail(
        &self,
        publication: String,
        record: String,
    ) -> io::Result<MemoryDetail> {
        let publication_id = Uuid::parse_str(&publication)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "记忆发布标识无效"))?;
        if publication_id.to_string() != publication {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "记忆发布标识无效",
            ));
        }
        if !detail_name(&record) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "记忆记录标识格式无效",
            ));
        }
        self.with_active_store(move |connection, workspace| {
            current_generation(connection)?;
            let current: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM publication WHERE publication_id = ?1)",
                    [&publication],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            if !current {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "记忆已重新发布，请刷新后查看来源",
                ));
            }
            let redactor = Redactor::new()?;
            let mut result = if let Some((name, hash, entry)) =
                classified::saved(connection, Some(&record))?.pop()
            {
                let text = inspection_text(&entry);
                let mut checked = classified_detail(workspace, name, hash, entry, publication)?;
                checked.text = text;
                checked
            } else {
                detail(workspace, source(connection, &record)?)?
            };
            result.text = redactor.clean(&result.text, MAX_DETAIL_BYTES);
            Ok(result)
        })
        .await
    }

    pub(crate) async fn search(
        &self,
        epoch: Option<i64>,
        directory: Option<PathBuf>,
        view: Option<ViewHandle>,
        query: String,
        cancel: CancellationToken,
    ) -> io::Result<Vec<MemoryMatch>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "记忆查询需为 1 至 256 字节",
            ));
        }
        self.with_epoch(epoch, move |connection, workspace| {
            check_cancel(&cancel)?;
            if let Some(directory) = &directory {
                let Some(view) = active_view(&view, directory)? else {
                    return Ok(Vec::new());
                };
                bound_generation(connection, view)?;
                return search_records(&view.records, &query, &cancel);
            }
            current_generation(connection)?;
            search_records(
                &records(connection, workspace, None, None)?,
                &query,
                &cancel,
            )
        })
        .await
    }

    pub(crate) async fn read_for_task(
        &self,
        epoch: Option<i64>,
        directory: Option<PathBuf>,
        view: Option<ViewHandle>,
        record: String,
        cancel: CancellationToken,
    ) -> io::Result<MemoryDetail> {
        if !detail_name(&record) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "记忆记录标识格式无效",
            ));
        }
        self.with_epoch(epoch, move |connection, workspace| {
            check_cancel(&cancel)?;
            let detail = if let Some(directory) = &directory {
                let view = active_view(&view, directory)?.ok_or_else(invisible)?;
                bound_generation(connection, view)?;
                view.records.iter().find(|entry| entry.detail.source.record == record)
                    .map(|entry| entry.detail.clone()).ok_or_else(invisible)?
            } else {
                current_generation(connection)?;
                if let Some((name, hash, entry)) = classified::saved(connection, Some(&record))?.pop() {
                    let publication = current_publication(connection)?.ok_or_else(|| io::Error::other("分类记忆缺少发布身份"))?;
                    classified_detail(workspace, name, hash, entry, publication)?
                } else {
                    detail(workspace, source(connection, &record)?)?
                }
            };
            check_cancel(&cancel)?;
            let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?.as_millis()).map_err(io::Error::other)?;
            connection.execute(
                "UPDATE sources SET usage_count = usage_count + 1, last_usage_ms = ?2 WHERE session_id = ?1",
                params![detail.source.session_id.to_string(), now],
            ).map_err(io::Error::other)?;
            Ok(detail)
        }).await
    }

    pub(crate) async fn management_summary(
        &self,
        epoch: Option<i64>,
        cancel: CancellationToken,
    ) -> io::Result<Option<String>> {
        self.with_epoch(epoch, move |connection, workspace| {
            check_cancel(&cancel)?;
            current_generation(connection)?;
            Ok(crate::publication::snapshot(connection, workspace)?
                .map(|published| published.summary))
        })
        .await
    }
}

pub(crate) fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "记忆操作已取消"));
    }
    Ok(())
}

fn source(connection: &Connection, record: &str) -> io::Result<IndexedSource> {
    if !detail_name(record) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "记忆记录标识格式无效",
        ));
    }
    sources(connection, Some(record))?
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到当前已发布的记忆记录"))
}

fn invisible() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "当前目录没有这份已发布记录")
}

fn contains_words(body: &str, words: &[&str]) -> bool {
    let text = body.to_lowercase();
    words.iter().all(|word| text.contains(word))
}

fn current_publication(connection: &Connection) -> io::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT publication_id FROM publication CROSS JOIN classified_publication",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)
}

fn classified_source(
    name: String,
    entry: classified::Entry,
    publication: String,
) -> io::Result<MemorySource> {
    let directory = match &entry.scope {
        Scope::Project { directory } => {
            Some(Redactor::new()?.clean(directory, MAX_SCOPE_DISPLAY_BYTES))
        }
        _ => None,
    };
    Ok(MemorySource {
        session_id: entry.session,
        record: name,
        sequence: entry.sequence().and_then(|value| i64::try_from(value).ok()),
        revision: entry.id,
        activity_ms: i64::try_from(entry.at_ms).ok(),
        publication: Some(publication),
        directory,
        category: Some(entry.category),
        scope: Some(entry.scope),
        state: Some(entry.state),
        provenance: Some(Box::new(entry.source)),
        attention: entry.attention.map(Box::new),
        experience: entry.experience.map(Box::new),
    })
}

fn inspection_records(
    connection: &Connection,
) -> io::Result<BTreeMap<Category, Vec<MemoryInspectionRecord>>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    let redactor = Redactor::new()?;
    for (record, _, entry) in classified::saved(connection, None)? {
        let (scope, directory) = match &entry.scope {
            Scope::Global => ("全局", None),
            Scope::Context => ("原对话情境", None),
            Scope::Project { directory } => (
                "项目",
                Some(redactor.clean(directory, MAX_SCOPE_DISPLAY_BYTES)),
            ),
        };
        let status = match &entry.experience {
            Some(experience) if experience.state == ExperienceState::Candidate => "候选",
            _ => entry.state.label(),
        };
        let (status, summary) = if entry.body.is_empty() {
            (entry.state.label(), "")
        } else if let Some(attention) = &entry.attention {
            (attention.state.label(), attention.question.as_str())
        } else if let Some(experience) = &entry.experience {
            (status, experience.topic.as_str())
        } else {
            (status, entry.body.as_str())
        };
        groups
            .entry(entry.category)
            .or_default()
            .push(MemoryInspectionRecord {
                record,
                session_id: entry.session,
                activity_ms: i64::try_from(entry.at_ms).ok(),
                scope,
                directory,
                status,
                provenance: inspection_provenance(&entry.source),
                preview: redactor.clean(summary, MAX_INSPECTION_PREVIEW_BYTES),
            });
    }
    for indexed in sources(connection, None)? {
        let source = indexed.source;
        groups
            .entry(Category::WorkFact)
            .or_default()
            .push(MemoryInspectionRecord {
                record: source.record,
                session_id: source.session_id,
                activity_ms: source.activity_ms,
                scope: if indexed.scope.is_some() {
                    "项目"
                } else {
                    "目录待核对"
                },
                directory: source.directory,
                status: "来源摘要",
                provenance: source.sequence.map_or_else(
                    || "会话存档".into(),
                    |sequence| format!("会话存档 · 序号 {sequence}"),
                ),
                preview: String::new(),
            });
    }
    Ok(groups)
}

fn inspection_provenance(source: &classified::Provenance) -> String {
    match source {
        classified::Provenance::Observation { anchor, evidence } => {
            let label = match evidence {
                EvidenceKind::UserStatement => "用户陈述",
                EvidenceKind::RootExpression => "Origin 表达",
                EvidenceKind::ToolObservation => "工具回执",
                EvidenceKind::Inference => "据来源推断",
            };
            format!(
                "{label} · 第 {} 轮 · 序号 {}",
                anchor.turn.0, anchor.sequence
            )
        }
        classified::Provenance::Revision { turn, .. } => format!("用户授权修订 · 第 {turn} 轮"),
    }
}

fn inspection_text(entry: &classified::Entry) -> String {
    if entry.body.is_empty() {
        return "此记录只保留状态与来源。".into();
    }
    let mut text = entry.body.clone();
    if let Some(attention) = &entry.attention {
        text.push_str(&format!(
            "\n\n{}\n{}",
            attention.author.label(),
            attention.question
        ));
        if let Some(reason) = &attention.reason {
            text.push_str(&format!("\n理由：{reason}"));
        }
    }
    if let Some(experience) = &entry.experience {
        text.push_str(&format!(
            "\n\n{}\n方法：{}\n适用条件：{}\n理由：{}",
            experience.topic, experience.method, experience.conditions, experience.reason
        ));
    }
    text
}

fn classified_detail(
    workspace: &Path,
    name: String,
    hash: String,
    entry: classified::Entry,
    publication: String,
) -> io::Result<MemoryDetail> {
    check_path(workspace, true)?;
    let directory = workspace.join(DETAILS);
    check_path(&directory, true)?;
    let text = read_checked(&directory.join(&name), MAX_DETAIL_BYTES, &hash)?;
    Ok(MemoryDetail {
        source: classified_source(name, entry, publication)?,
        text,
    })
}

fn sources(connection: &Connection, record: Option<&str>) -> io::Result<Vec<IndexedSource>> {
    let redactor = Redactor::new()?;
    let mut statement = connection.prepare(SOURCE_QUERY).map_err(io::Error::other)?;
    let mut rows = statement.query([record]).map_err(io::Error::other)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let id: String = row.get(0).map_err(io::Error::other)?;
        let session_id = SessionId::parse_str(&id).map_err(io::Error::other)?;
        let revision = row.get(1).map_err(io::Error::other)?;
        let hash: String = row.get(2).map_err(io::Error::other)?;
        let record: String = row.get(3).map_err(io::Error::other)?;
        let directory: Option<String> = row.get(4).map_err(io::Error::other)?;
        let sequence = row.get(5).map_err(io::Error::other)?;
        let activity_ms = row.get(6).map_err(io::Error::other)?;
        let content: String = row.get(7).map_err(io::Error::other)?;
        let directory_verified: bool = row.get(8).map_err(io::Error::other)?;
        if !detail_name(&record)
            || record != format!("{session_id}-{hash}.md")
            || content.len() > MAX_DETAIL_BYTES
            || sha256(&content) != hash
        {
            return Err(io::Error::other("已发布记忆索引与内容校验不符"));
        }
        result.push(IndexedSource {
            source: MemorySource {
                session_id,
                revision,
                record,
                directory: directory
                    .as_ref()
                    .map(|path| redactor.clean(path, MAX_SCOPE_DISPLAY_BYTES)),
                sequence,
                activity_ms,
                publication: row.get(9).map_err(io::Error::other)?,
                category: None,
                scope: None,
                state: None,
                provenance: None,
                attention: None,
                experience: None,
            },
            scope: directory.filter(|_| directory_verified).map(PathBuf::from),
            hash,
            content,
        });
    }
    Ok(result)
}

fn detail(workspace: &Path, indexed: IndexedSource) -> io::Result<MemoryDetail> {
    check_path(workspace, true)?;
    let directory = workspace.join(DETAILS);
    check_path(&directory, true)?;
    let text = read_checked(
        &directory.join(&indexed.source.record),
        MAX_DETAIL_BYTES,
        &indexed.hash,
    )?;
    Ok(MemoryDetail {
        source: indexed.source,
        text,
    })
}
