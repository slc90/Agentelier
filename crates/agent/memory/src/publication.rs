use crate::{
    consolidation::{Consolidation, sha256},
    store::check_path,
};
use agentelier_conversation::SessionId;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use tempfile::NamedTempFile;
use uuid::Uuid;

const SUMMARY: &str = "memory_summary.md";
const DETAILS: &str = "rollout_summaries";
const TEMP_PREFIX: &str = ".agentelier-memory-publish-";
const UUID_TEXT_BYTES: usize = 36;
const DIGEST_BYTES: usize = 64;
type Artifacts = BTreeMap<String, String>;

pub(crate) struct Published {
    pub(crate) id: Option<String>,
    pub(crate) summary: String,
    pub(crate) sources: BTreeMap<String, (String, String)>,
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
        summary: String::new(),
        sources: BTreeMap::new(),
    };
    let mut artifacts = Artifacts::new();
    if let Some((id, hash)) = record {
        artifacts = load_artifacts(connection)?;
        let summary = artifacts
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
        if artifacts.len() != published.sources.len() + 1 {
            return Err(io::Error::other("已发布来源详情数量与数据库不一致"));
        }
        published.id = Some(id);
        published.summary = summary.clone();
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
    summary: Option<String>,
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
    if work.previous_id != current.id || work.epoch != epoch {
        return Ok(false);
    }
    connection
        .execute("INSERT INTO pending_publication (singleton) VALUES (1)", [])
        .map_err(io::Error::other)?;
    match commit(connection, workspace, work, summary, published_at_ms) {
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
    summary: Option<String>,
    published_at_ms: i64,
) -> io::Result<()> {
    let tx = connection.transaction().map_err(io::Error::other)?;
    tx.execute_batch(
        "DELETE FROM published_artifacts; DELETE FROM published_sources; DELETE FROM publication;",
    )
    .map_err(io::Error::other)?;
    let mut artifacts = Artifacts::new();
    if let Some(summary) = summary {
        let publication_id = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO publication (singleton, publication_id, summary_hash, published_at_ms) VALUES (1, ?1, ?2, ?3)",
            params![publication_id, sha256(&summary), published_at_ms]).map_err(io::Error::other)?;
        for (id, detail) in work.details {
            tx.execute("INSERT INTO published_sources (session_id, revision, content_hash, file_name) VALUES (?1, ?2, ?3, ?4)",
                params![id, detail.revision, detail.hash, detail.name]).map_err(io::Error::other)?;
            artifacts.insert(detail.name, detail.content);
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

fn install(workspace: &Path, artifacts: &Artifacts) -> io::Result<()> {
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

fn prune(directory: &Path, should_remove: impl Fn(&str) -> bool) -> io::Result<()> {
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

fn detail_name(name: &str) -> bool {
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
