use rusqlite::Connection;
use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI64, Ordering},
};

const SUSPENDED_EPOCH: i64 = -1;
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct Memory {
    client: MemoryClient,
}

#[derive(Clone)]
pub struct MemoryClient {
    connection: Arc<Mutex<Connection>>,
    workspace: Arc<PathBuf>,
    active_epoch: Arc<AtomicI64>,
    pub(crate) revision_signal: Arc<tokio::sync::Notify>,
}

impl Memory {
    pub fn open(data_root: &Path) -> io::Result<Self> {
        let workspace = data_root.join("memory");
        check_path(&workspace, true)?;
        match fs::create_dir(&workspace) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        check_path(&workspace, true)?;

        for name in [
            "memory.sqlite",
            "memory.sqlite-journal",
            "memory.sqlite-wal",
            "memory.sqlite-shm",
        ] {
            check_path(&data_root.join(name), false)?;
        }
        let mut connection =
            Connection::open(data_root.join("memory.sqlite")).map_err(io::Error::other)?;
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction
            .execute_batch(include_str!("schema.sql"))
            .map_err(io::Error::other)?;
        for table in ["sources", "published_source_metadata"] {
            let present: bool = transaction.query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = 'directory_verified')",
                [table],
                |row| row.get(0),
            ).map_err(io::Error::other)?;
            if !present {
                transaction
                    .execute(
                        &format!(
                    "ALTER TABLE {table} ADD COLUMN directory_verified INTEGER NOT NULL DEFAULT 0
                     CHECK (directory_verified IN (0, 1))"
                ),
                        [],
                    )
                    .map_err(io::Error::other)?;
            }
        }
        transaction
            .execute(
                "UPDATE sources SET status = 'pending' WHERE status = 'processing'",
                [],
            )
            .map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        if crate::reset::pending(&connection)?
            && let Err(failure) = crate::reset::finish(&connection, &workspace)
        {
            tracing::error!(error = %failure, "启动时未完成长期记忆重置清理");
        }
        let epoch = crate::reset::available_epoch(&connection)?;

        Ok(Self {
            client: MemoryClient {
                connection: Arc::new(Mutex::new(connection)),
                workspace: Arc::new(workspace),
                active_epoch: Arc::new(AtomicI64::new(epoch.unwrap_or(SUSPENDED_EPOCH))),
                revision_signal: Arc::default(),
            },
        })
    }

    pub fn client(&self) -> MemoryClient {
        self.client.clone()
    }

    pub fn close(self) -> io::Result<()> {
        Arc::into_inner(self.client.connection)
            .ok_or_else(|| io::Error::other("记忆数据库仍有未回收的使用者"))?
            .into_inner()
            .map_err(|_| io::Error::other("记忆数据库连接状态异常"))?
            .close()
            .map_err(|(_, error)| io::Error::other(error))
    }
}

impl MemoryClient {
    pub async fn invalidate_current(&self) -> io::Result<()> {
        self.with_connection(|connection| invalidate_reads(connection))
            .await?;
        self.revision_signal.notify_one();
        Ok(())
    }

    pub async fn invalidate_sources(
        &self,
        sessions: Vec<agentelier_conversation::SessionId>,
    ) -> io::Result<()> {
        let changed = self
            .with_connection(move |connection| {
                let published: Vec<String> = connection
                    .prepare(
                        "SELECT session_id FROM published_sources
                 UNION SELECT json_extract(metadata, '$.session') FROM published_classified
                 WHERE json_extract(metadata, '$.source.kind') = 'observation'",
                    )?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                if published.iter().any(|id| {
                    agentelier_conversation::SessionId::parse_str(id)
                        .map_or(true, |id| !sessions.contains(&id))
                }) {
                    let (current, published) = generations(connection)?;
                    if current == published {
                        invalidate_reads(connection)?;
                        return Ok(true);
                    }
                }
                Ok(false)
            })
            .await?;
        if changed {
            self.revision_signal.notify_one();
        }
        Ok(())
    }

    pub fn epoch(&self) -> Option<i64> {
        let epoch = self.active_epoch.load(Ordering::Acquire);
        (epoch >= 0).then_some(epoch)
    }

    pub fn suspend(&self) {
        self.active_epoch.store(SUSPENDED_EPOCH, Ordering::Release);
    }

    pub async fn resume_if_ready(&self) -> io::Result<()> {
        let epoch = self
            .with_store(|connection, _| crate::reset::available_epoch(connection))
            .await?;
        self.active_epoch
            .store(epoch.unwrap_or(SUSPENDED_EPOCH), Ordering::Release);
        Ok(())
    }

    pub(crate) async fn with_active_store<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection, &Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let epoch = self.epoch().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "长期记忆重置进行中或清理尚未完成",
            )
        })?;
        self.with_epoch(Some(epoch), operation).await
    }

    pub(crate) async fn with_epoch<T: Send + 'static>(
        &self,
        expected: Option<i64>,
        operation: impl FnOnce(&mut Connection, &Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let paused = || {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "工具接纳时的长期记忆纪元已不可用",
            )
        };
        let epoch = expected.ok_or_else(paused)?;
        let current = self.active_epoch.clone();
        self.with_store(move |connection, workspace| {
            if current.load(Ordering::Acquire) != epoch {
                return Err(paused());
            }
            operation(connection, workspace)
        })
        .await
    }

    pub(crate) async fn with_connection<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        self.with_store(move |connection, _| operation(connection).map_err(io::Error::other))
            .await
    }

    pub(crate) async fn with_store<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection, &Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let connection = self.connection.clone();
        let workspace = self.workspace.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = connection
                .lock()
                .map_err(|_| io::Error::other("记忆数据库连接状态异常"))?;
            operation(&mut guard, &workspace)
        })
        .await
        .map_err(io::Error::other)?
    }
}

pub(crate) fn generations(connection: &Connection) -> rusqlite::Result<(i64, i64)> {
    connection.query_row(
        "SELECT generation, published_generation FROM memory_read_guard WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

pub(crate) fn invalidate_reads(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE memory_read_guard SET generation = generation + 1 WHERE singleton = 1",
        [],
    )?;
    Ok(())
}

pub(crate) fn check_path(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("记忆存储路径类型无效：{}", path.display()),
        ));
    }
    Ok(())
}
