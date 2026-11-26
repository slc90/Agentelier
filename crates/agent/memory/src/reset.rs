use crate::{MemoryClient, publication, revision};
use agentelier_session_store::{MemoryCutoff, SessionStoreClient};
use rusqlite::{Connection, params};
use std::{collections::BTreeMap, io, path::Path};

impl MemoryClient {
    pub async fn reset(&self, store: &SessionStoreClient) -> io::Result<()> {
        let cutoffs = if self.with_store(|connection, _| pending(connection)).await? {
            Vec::new()
        } else {
            store.memory_cutoffs().await?
        };
        self.with_store(move |connection, workspace| {
            if !pending(connection)? {
                begin(connection, cutoffs)?;
            }
            finish(connection, workspace)
        })
        .await
    }
}

pub(crate) fn pending(connection: &Connection) -> io::Result<bool> {
    connection
        .query_row("SELECT EXISTS (SELECT 1 FROM pending_reset)", [], |row| {
            row.get(0)
        })
        .map_err(io::Error::other)
}

pub(crate) fn available_epoch(connection: &Connection) -> io::Result<Option<i64>> {
    connection.query_row(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pending_reset) THEN NULL ELSE epoch END FROM memory_state",
        [],
        |row| row.get(0),
    ).map_err(io::Error::other)
}

pub(crate) fn last_turn(connection: &Connection, session: &str) -> io::Result<u64> {
    let turn: i64 = connection
        .query_row(
            "SELECT COALESCE((SELECT last_turn FROM reset_turn_cutoffs WHERE session_id = ?1), 0)",
            [session],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    u64::try_from(turn).map_err(io::Error::other)
}

fn begin(connection: &mut Connection, cutoffs: Vec<MemoryCutoff>) -> io::Result<()> {
    connection
        .pragma_update(None, "secure_delete", true)
        .map_err(io::Error::other)?;
    let tx = connection.transaction().map_err(io::Error::other)?;
    for cutoff in cutoffs {
        let session = cutoff.session.to_string();
        let sequence = i64::try_from(cutoff.sequence).map_err(io::Error::other)?;
        let turn = i64::try_from(cutoff.last_turn).map_err(io::Error::other)?;
        tx.execute(
            "INSERT INTO reset_cutoffs (session_id, sequence) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET sequence = max(sequence, excluded.sequence)",
            params![session, sequence],
        )
        .map_err(io::Error::other)?;
        tx.execute(
            "INSERT INTO reset_turn_cutoffs (session_id, last_turn) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_turn = max(last_turn, excluded.last_turn)",
            params![session, turn],
        )
        .map_err(io::Error::other)?;
    }
    tx.execute_batch(
        "UPDATE memory_state SET epoch = epoch + 1;
         UPDATE memory_read_guard SET generation = 0, published_generation = 0;
         INSERT INTO pending_reset (singleton) VALUES (1);
         DELETE FROM pending_publication;
         DELETE FROM consolidation_inputs;
         DELETE FROM published_artifacts;
         DELETE FROM published_classified;
         DELETE FROM classified_publication;
         DELETE FROM published_source_metadata;
         DELETE FROM published_sources;
         DELETE FROM publication;
         DELETE FROM revisions;
         DELETE FROM source_observations;
         DELETE FROM attention_boundaries;
         DELETE FROM experience_objections;
         DELETE FROM sources;",
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

pub(crate) fn finish(connection: &Connection, workspace: &Path) -> io::Result<()> {
    // The committed marker and cutoffs survive a crash between file removals.
    publication::install(workspace, &BTreeMap::new())?;
    let notes = revision::notes_directory(workspace, false)?;
    publication::prune(&notes, |name| {
        name.starts_with(revision::TEMP_PREFIX)
            || name.strip_suffix(".md").is_some_and(revision::revision_id)
    })?;
    connection
        .execute("DELETE FROM pending_reset", [])
        .map_err(io::Error::other)?;
    Ok(())
}
