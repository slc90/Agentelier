use crate::{
    classified,
    consolidation::{Consolidation, PublicationContent, sha256},
    store::{FILE_ATTRIBUTE_REPARSE_POINT, check_path},
};
use agentelier_conversation::SessionId;
use agentelier_llm_model::{MAX_MEMORY_SUMMARY_BYTES, MemorySnapshot};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use tempfile::NamedTempFile;
use uuid::Uuid;

impl crate::MemoryClient {
    pub async fn snapshot(&self) -> io::Result<Option<MemorySnapshot>> {
        self.with_active_store(|connection, workspace| {
            let current: bool = connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM classified_publication)",
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            if current {
                snapshot(connection, workspace)
            } else {
                Ok(None)
            }
        })
        .await
    }
}

pub(crate) fn snapshot(
    connection: &Connection,
    workspace: &Path,
) -> io::Result<Option<MemorySnapshot>> {
    let record: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT publication_id, summary_hash, epoch FROM publication CROSS JOIN memory_state",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    let Some((publication_id, hash, epoch)) = record else {
        return Ok(None);
    };
    check_path(workspace, true)?;
    let publication_id = Uuid::parse_str(&publication_id).map_err(io::Error::other)?;
    let summary = read_checked(&workspace.join(SUMMARY), MAX_MEMORY_SUMMARY_BYTES, &hash)?;
    let snapshot = MemorySnapshot {
        publication_id,
        hash,
        epoch,
        summary,
    };
    snapshot.validate().map_err(io::Error::other)?;
    Ok(Some(snapshot))
}

pub(crate) fn committed_snapshot(connection: &Connection) -> io::Result<MemorySnapshot> {
    let record: Option<(String, String, i64, String)> = connection
        .query_row(
            "SELECT publication_id, summary_hash, epoch, content FROM publication
         CROSS JOIN memory_state JOIN published_artifacts ON file_name = ?1",
            [SUMMARY],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    let Some((publication_id, hash, epoch, summary)) = record else {
        return Err(io::Error::other("记忆库缺少已提交的摘要副本"));
    };
    let snapshot = MemorySnapshot {
        publication_id: Uuid::parse_str(&publication_id).map_err(io::Error::other)?,
        hash,
        epoch,
        summary,
    };
    snapshot.validate().map_err(io::Error::other)?;
    Ok(snapshot)
}

pub(crate) fn read_checked(path: &Path, limit: usize, hash: &str) -> io::Result<String> {
    let text = read_bounded(path, limit)?;
    if sha256(&text) != hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "已发布记忆文件校验值无效",
        ));
    }
    Ok(text)
}

pub(crate) fn read_bounded(path: &Path, limit: usize) -> io::Result<String> {
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("已发布记忆必须是普通文件"));
    }
    let mut text = String::new();
    file.take(limit as u64 + 1).read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(io::Error::other("记忆文件超过大小限制"));
    }
    Ok(text)
}

const SUMMARY: &str = "memory_summary.md";
pub(crate) const DETAILS: &str = "rollout_summaries";
const TEMP_PREFIX: &str = ".agentelier-memory-publish-";
const UUID_TEXT_BYTES: usize = 36;
const DIGEST_BYTES: usize = 64;
type Artifacts = BTreeMap<String, String>;

pub(crate) struct Published {
    pub(crate) id: Option<String>,
    pub(crate) sources: BTreeMap<String, (String, String)>,
    pub(crate) classified_hash: Option<String>,
}

pub(crate) fn restore(connection: &Connection, workspace: &Path) -> io::Result<Published> {
    let pending = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pending_publication)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(io::Error::other)?;
    let record: Option<(String, String)> = connection
        .query_row(
            "SELECT publication_id, summary_hash FROM publication WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    let mut published = Published {
        id: None,
        sources: BTreeMap::new(),
        classified_hash: connection
            .query_row(
                "SELECT input_hash FROM classified_publication WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?,
    };
    let mut artifacts = Artifacts::new();
    if let Some((id, hash)) = record {
        artifacts = load_artifacts(connection)?;
        artifacts
            .get(SUMMARY)
            .filter(|content| sha256(content) == hash)
            .ok_or_else(|| io::Error::other("已发布全局摘要与数据库校验不符"))?;
        let mut statement = connection
            .prepare("SELECT session_id, revision, content_hash, file_name FROM published_sources")
            .map_err(io::Error::other)?;
        let mut rows = statement.query([]).map_err(io::Error::other)?;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let source: String = row.get(0).map_err(io::Error::other)?;
            let revision: String = row.get(1).map_err(io::Error::other)?;
            let hash: String = row.get(2).map_err(io::Error::other)?;
            let name: String = row.get(3).map_err(io::Error::other)?;
            if !detail_name(&name)
                || !name.starts_with(&source)
                || !artifacts
                    .get(&name)
                    .is_some_and(|body| sha256(body) == hash)
            {
                return Err(io::Error::other("已发布来源详情与数据库校验不符"));
            }
            published.sources.insert(source, (revision, hash));
        }
        let classified = classified::saved(connection, None)?;
        for (name, hash, _) in &classified {
            if !artifacts
                .get(name)
                .is_some_and(|body| sha256(body) == *hash)
            {
                return Err(io::Error::other("已发布分类详情与数据库校验不符"));
            }
        }
        if artifacts.len() != published.sources.len() + classified.len() + 1 {
            return Err(io::Error::other("已发布来源详情数量与数据库不一致"));
        }
        published.id = Some(id);
    }
    let mut inputs = connection
        .prepare("SELECT session_id, revision, content_hash FROM consolidation_inputs")
        .map_err(io::Error::other)?;
    let rows = inputs
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
            ))
        })
        .map_err(io::Error::other)?;
    let inputs = rows
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(io::Error::other)?;
    if !inputs.is_empty() {
        published.sources = inputs;
    }
    if pending || published.id.is_some() {
        install(workspace, &artifacts)?;
        if pending {
            connection
                .execute("DELETE FROM pending_publication", [])
                .map_err(io::Error::other)?;
        }
    }
    Ok(published)
}

pub(crate) fn publish(
    connection: &mut Connection,
    workspace: &Path,
    work: Consolidation,
    content: PublicationContent,
    published_at_ms: i64,
) -> io::Result<bool> {
    let current = restore(connection, workspace)?;
    let epoch: i64 = connection
        .query_row(
            "SELECT epoch FROM memory_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    let notes = crate::revision::revisions(workspace, epoch)?;
    if work.previous_id != current.id
        || work.epoch != epoch
        || !notes
            .iter()
            .map(|note| &note.id)
            .eq(work.revisions.iter().map(|note| &note.id))
    {
        return Ok(false);
    }
    connection
        .execute("INSERT INTO pending_publication (singleton) VALUES (1)", [])
        .map_err(io::Error::other)?;
    match commit(connection, workspace, work, content, published_at_ms) {
        Ok(()) => Ok(true),
        Err(failure) => match restore(connection, workspace) {
            Ok(_) => Err(failure),
            Err(recovery) => Err(io::Error::other(format!(
                "整合发布失败：{failure}。恢复上次发布失败：{recovery}"
            ))),
        },
    }
}

fn commit(
    connection: &mut Connection,
    workspace: &Path,
    work: Consolidation,
    content: PublicationContent,
    published_at_ms: i64,
) -> io::Result<()> {
    let tx = connection.transaction().map_err(io::Error::other)?;
    tx.execute_batch(
        "DELETE FROM published_artifacts; DELETE FROM published_sources; DELETE FROM published_source_metadata;
         DELETE FROM published_classified; DELETE FROM publication;",
    )
    .map_err(io::Error::other)?;
    tx.execute_batch("DELETE FROM consolidation_inputs; DELETE FROM revisions;")
        .map_err(io::Error::other)?;
    tx.execute(
        "INSERT INTO classified_publication (singleton, input_hash) VALUES (1, ?1)
         ON CONFLICT(singleton) DO UPDATE SET input_hash = excluded.input_hash",
        [&work.classified_hash],
    )
    .map_err(io::Error::other)?;
    for (id, detail) in &work.details {
        tx.execute("INSERT INTO consolidation_inputs (session_id, revision, content_hash) VALUES (?1, ?2, ?3)",
            params![id, detail.revision, detail.hash]).map_err(io::Error::other)?;
    }
    for note in &work.revisions {
        tx.execute(
            "INSERT INTO revisions (revision_id, published) VALUES (?1, 1)",
            [&note.id],
        )
        .map_err(io::Error::other)?;
    }
    let mut artifacts = Artifacts::new();
    if let Some(summary) = content.summary {
        let publication_id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO publication (singleton, publication_id, summary_hash, published_at_ms) VALUES (1, ?1, ?2, ?3)",
            params![publication_id, sha256(&summary), published_at_ms]).map_err(io::Error::other)?;
        for (id, detail) in work.details {
            if content.excluded.contains(&id) {
                continue;
            }
            tx.execute("INSERT INTO published_sources (session_id, revision, content_hash, file_name) VALUES (?1, ?2, ?3, ?4)",
                params![id, detail.revision, detail.hash, detail.name]).map_err(io::Error::other)?;
            tx.execute("INSERT INTO published_source_metadata (session_id, work_directory, sequence, activity_ms, directory_verified)
                VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, detail.directory, detail.sequence, detail.activity_ms, detail.directory_verified]).map_err(io::Error::other)?;
            artifacts.insert(detail.name, detail.content);
        }
        for entry in content.classified {
            let (name, hash, body) = entry.artifact()?;
            let metadata = serde_json::to_string(&entry).map_err(io::Error::other)?;
            tx.execute("INSERT INTO published_classified (file_name, content_hash, metadata) VALUES (?1, ?2, ?3)",
                params![name, hash, metadata]).map_err(io::Error::other)?;
            artifacts.insert(name, body);
        }
        artifacts.insert(SUMMARY.to_owned(), summary);
    }
    for (name, content) in &artifacts {
        tx.execute(
            "INSERT INTO published_artifacts (file_name, content) VALUES (?1, ?2)",
            params![name, content],
        )
        .map_err(io::Error::other)?;
    }
    for id in work.deleted {
        tx.execute(
            "DELETE FROM source_observations WHERE session_id = ?1",
            [&id],
        )
        .map_err(io::Error::other)?;
        tx.execute("DELETE FROM sources WHERE session_id = ?1", [id])
            .map_err(io::Error::other)?;
    }
    // The separately committed marker makes the previous files recoverable after a crash.
    install(workspace, &artifacts)?;
    tx.execute("DELETE FROM pending_publication", [])
        .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn load_artifacts(connection: &Connection) -> io::Result<Artifacts> {
    let mut statement = connection
        .prepare("SELECT file_name, content FROM published_artifacts")
        .map_err(io::Error::other)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(io::Error::other)?;
    rows.collect::<rusqlite::Result<Artifacts>>()
        .map_err(io::Error::other)
}

pub(crate) fn install(workspace: &Path, artifacts: &Artifacts) -> io::Result<()> {
    check_path(workspace, true)?;
    let details = workspace.join(DETAILS);
    check_path(&details, true)?;
    if !artifacts.is_empty() {
        match fs::create_dir(&details) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        check_path(&details, true)?;
    }
    for (name, content) in artifacts
        .iter()
        .filter(|(name, _)| name.as_str() != SUMMARY)
    {
        if !detail_name(name) {
            return Err(io::Error::other("来源详情文件名无效"));
        }
        replace(&details.join(name), content)?;
    }
    let summary_path = workspace.join(SUMMARY);
    match artifacts.get(SUMMARY) {
        Some(content) => replace(&summary_path, content)?,
        None => remove(&summary_path)?,
    }
    prune(&details, |name| {
        (detail_name(name) && !artifacts.contains_key(name)) || name.starts_with(TEMP_PREFIX)
    })?;
    prune(workspace, |name| name.starts_with(TEMP_PREFIX))
}

fn replace(path: &Path, content: &str) -> io::Result<()> {
    check_path(path, false)?;
    match fs::File::open(path) {
        Ok(file) if file.metadata()?.len() == content.len() as u64 => {
            let mut bytes = Vec::new();
            file.take(content.len() as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes == content.as_bytes() {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("记忆文件缺少父目录"))?;
    let mut temp = NamedTempFile::with_prefix_in(TEMP_PREFIX, directory)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn prune(directory: &Path, should_remove: impl Fn(&str) -> bool) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(&should_remove) {
            remove(&entry.path())?;
        }
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    check_path(path, false)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn detail_name(name: &str) -> bool {
    let Some((id, tail)) = name.split_at_checked(UUID_TEXT_BYTES) else {
        return false;
    };
    let Some(hash) = tail
        .strip_prefix('-')
        .and_then(|tail| tail.strip_suffix(".md"))
    else {
        return false;
    };
    SessionId::parse_str(id).is_ok_and(|session| !session.is_nil() && session.to_string() == id)
        && hash.len() == DIGEST_BYTES
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
