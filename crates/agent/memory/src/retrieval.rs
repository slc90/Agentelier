use crate::{
    MemoryClient,
    consolidation::sha256,
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

const MAX_QUERY_BYTES: usize = 256;
const MAX_MATCHES: usize = 5;
const MAX_EXCERPT_BYTES: usize = 640;
const MAX_SCOPE_DISPLAY_BYTES: usize = 512;
const MAX_DETAIL_BYTES: usize = 9 * 1024;
const SOURCE_QUERY: &str = "SELECT p.session_id, p.revision, p.content_hash, p.file_name,
        COALESCE(m.work_directory, s.work_directory), COALESCE(m.sequence, s.sequence),
        COALESCE(m.activity_ms, s.activity_ms), a.content
    FROM published_sources p
    LEFT JOIN published_source_metadata m ON m.session_id = p.session_id
    LEFT JOIN sources s ON s.session_id = p.session_id AND s.revision = p.revision
    LEFT JOIN published_artifacts a ON a.file_name = p.file_name
    WHERE (?1 IS NULL OR p.file_name = ?1)
    ORDER BY COALESCE(m.activity_ms, s.activity_ms) DESC, p.session_id";

#[derive(Serialize)]
pub(crate) struct MemorySource {
    pub(crate) session_id: SessionId,
    pub(crate) record: String,
    pub(crate) revision: String,
    pub(crate) directory: Option<String>,
    pub(crate) sequence: Option<i64>,
    pub(crate) activity_ms: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct MemoryMatch {
    source: MemorySource,
    excerpt: String,
}

#[derive(Serialize)]
pub(crate) struct MemoryDetail {
    pub(crate) source: MemorySource,
    pub(crate) text: String,
}

struct IndexedSource {
    source: MemorySource,
    scope: Option<PathBuf>,
    hash: String,
    content: String,
}

impl MemoryClient {
    pub(crate) async fn search(
        &self,
        directory: PathBuf,
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
        self.with_store(move |connection, _| {
            check_cancel(&cancel)?;
            let words: Vec<_> = query.split_whitespace().collect();
            let mut matches = Vec::new();
            for indexed in sources(connection, None)? {
                check_cancel(&cancel)?;
                if indexed.scope.as_deref() != Some(directory.as_path()) {
                    continue;
                }
                let body = indexed
                    .content
                    .split_once("\n\n")
                    .and_then(|(_, rest)| rest.split_once("\n\n"))
                    .map_or(indexed.content.as_str(), |(_, body)| body);
                let text = body.to_lowercase();
                if words.iter().all(|word| text.contains(word)) {
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
        directory: PathBuf,
        record: String,
        cancel: CancellationToken,
    ) -> io::Result<MemoryDetail> {
        self.with_store(move |connection, workspace| {
            check_cancel(&cancel)?;
            let indexed = source(connection, &record)?;
            if indexed.scope.as_deref() != Some(directory.as_path()) {
                return Err(io::Error::new(io::ErrorKind::NotFound, "当前目录没有这份已发布记录"));
            }
            let detail = detail(workspace, indexed)?;
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
            },
            scope: directory.map(PathBuf::from),
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
