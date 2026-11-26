use crate::{
    MemoryClient, classified,
    consolidation::sha256,
    observation::{Category, Scope},
    publication::{DETAILS, detail_name, read_checked},
    redaction::Redactor,
    store::check_path,
};
use agentelier_conversation::SessionId;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_QUERY_BYTES: usize = 256;
const MAX_MATCHES: usize = 5;
const MAX_EXCERPT_BYTES: usize = 640;
const MAX_SCOPE_DISPLAY_BYTES: usize = 512;
const MAX_DETAIL_BYTES: usize = 9 * 1024;
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
    provenance: Option<classified::Provenance>,
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
    pub sources: Vec<MemorySource>,
    pub processing: bool,
    pub pending_revisions: usize,
    pub notice: Option<&'static str>,
}

struct IndexedSource {
    source: MemorySource,
    scope: Option<PathBuf>,
    hash: String,
    content: String,
}

impl MemoryClient {
    pub async fn inspect(&self, processing: bool) -> io::Result<MemoryInspection> {
        self.with_active_store(move |connection, workspace| {
            let (snapshot, mut notice) = match crate::publication::snapshot(connection, workspace) {
                Ok(snapshot) => (snapshot, None),
                Err(failure) => {
                    tracing::warn!(error = %failure, "读取摘要文件失败，尝试已提交的摘要副本");
                    (Some(crate::publication::committed_snapshot(connection)?),
                     Some("读取已发布摘要文件失败，当前显示记忆库中已提交的副本。"))
                }
            };
            let epoch: i64 = connection.query_row("SELECT epoch FROM memory_state WHERE singleton = 1", [], |row| row.get(0)).map_err(io::Error::other)?;
            let pending_revisions = match crate::revision::revisions(workspace, epoch).and_then(|revisions| {
                let published = crate::revision::published_ids(connection)?;
                Ok(revisions.iter().filter(|revision| !published.contains(&revision.id)).count())
            }) {
                Ok(count) => count,
                Err(failure) => {
                    tracing::warn!(error = %failure, "记忆修订待发布状态不可用");
                    notice.get_or_insert("无法核对待整合的记忆修订。");
                    0
                }
            };
            let processing = processing || (snapshot.is_none() && connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE status = 'summary'
                    AND NOT EXISTS (SELECT 1 FROM consolidation_inputs WHERE session_id = sources.session_id))",
                [],
                |row| row.get(0),
            ).map_err(io::Error::other)?);
            let Some(snapshot) = snapshot else {
                return Ok(MemoryInspection { summary: None, sources: Vec::new(), processing, pending_revisions, notice });
            };
            let published_at_ms = connection.query_row("SELECT published_at_ms FROM publication", [], |row| row.get(0)).map_err(io::Error::other)?;
            let sources = sources(connection, None)?.into_iter().map(|source| source.source).collect();
            Ok(MemoryInspection {
                summary: Some(PublishedSummary { id: snapshot.publication_id.to_string(), text: snapshot.summary, published_at_ms }),
                sources,
                processing,
                pending_revisions,
                notice,
            })
        }).await
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
        self.with_active_store(move |connection, workspace| {
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
            if let Some((name, hash, entry)) = classified::saved(connection, Some(&record))?.pop() {
                return classified_detail(workspace, name, hash, entry, publication);
            }
            detail(workspace, source(connection, &record)?)
        })
        .await
    }

    pub(crate) async fn search(
        &self,
        epoch: Option<i64>,
        directory: Option<PathBuf>,
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
        self.with_epoch(epoch, move |connection, _| {
            check_cancel(&cancel)?;
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
            if let Some(publication) = current_publication(connection)? {
                for (name, _, entry) in classified::saved(connection, None)? {
                    check_cancel(&cancel)?;
                    let scope_name = match &entry.scope {
                        Scope::Context => "context",
                        Scope::Project { .. } => "project",
                        Scope::Global => "global",
                    };
                    if !entry.visible_from(directory.as_deref())
                        || entry.body.is_empty()
                        || category.is_some_and(|category| category != entry.category.label())
                        || scope.is_some_and(|scope| scope != scope_name)
                        || !contains_words(&entry.body, &words)
                    {
                        continue;
                    }
                    let excerpt =
                        entry.body[..entry.body.floor_char_boundary(MAX_EXCERPT_BYTES)].to_owned();
                    matches.push(MemoryMatch {
                        source: classified_source(name, entry, publication.clone())?,
                        excerpt,
                    });
                    if matches.len() == MAX_MATCHES {
                        return Ok(matches);
                    }
                }
            }
            if category.is_some() || scope.is_some_and(|scope| scope != "project") {
                return Ok(matches);
            }
            for indexed in sources(connection, None)? {
                check_cancel(&cancel)?;
                if directory
                    .as_ref()
                    .is_some_and(|directory| indexed.scope.as_deref() != Some(directory.as_path()))
                {
                    continue;
                }
                let body = indexed
                    .content
                    .split_once("\n\n")
                    .and_then(|(_, rest)| rest.split_once("\n\n"))
                    .map_or(indexed.content.as_str(), |(_, body)| body);
                if contains_words(body, &words) {
                    let line = body
                        .lines()
                        .find(|line| {
                            let line = line.to_lowercase();
                            words.iter().any(|word| line.contains(word))
                        })
                        .unwrap_or(body);
                    let excerpt = &line[..line.floor_char_boundary(MAX_EXCERPT_BYTES)];
                    matches.push(MemoryMatch {
                        source: indexed.source,
                        excerpt: excerpt.to_owned(),
                    });
                }
                if matches.len() == MAX_MATCHES {
                    break;
                }
            }
            Ok(matches)
        })
        .await
    }

    pub(crate) async fn read_for_task(
        &self,
        epoch: Option<i64>,
        directory: Option<PathBuf>,
        record: String,
        cancel: CancellationToken,
    ) -> io::Result<MemoryDetail> {
        self.with_epoch(epoch, move |connection, workspace| {
            check_cancel(&cancel)?;
            let detail = if let Some((name, hash, entry)) = classified::saved(connection, Some(&record))?.pop() {
                let publication = current_publication(connection)?.ok_or_else(|| io::Error::other("分类记忆缺少发布身份"))?;
                if !entry.visible_from(directory.as_deref()) {
                    return Err(invisible());
                }
                classified_detail(workspace, name, hash, entry, publication)?
            } else {
                let indexed = source(connection, &record)?;
                if directory.as_ref().is_some_and(|directory| indexed.scope.as_deref() != Some(directory.as_path())) {
                    return Err(invisible());
                }
                detail(workspace, indexed)?
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
        provenance: Some(entry.source),
    })
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
