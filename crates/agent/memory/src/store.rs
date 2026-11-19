use rusqlite::Connection;
use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct Memory {
    client: MemoryClient,
}

#[derive(Clone)]
pub struct MemoryClient {
    connection: Arc<Mutex<Connection>>,
    workspace: Arc<PathBuf>,
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
        transaction
            .execute(
                "UPDATE sources SET status = 'pending' WHERE status = 'processing'",
                [],
            )
            .map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        Ok(Self {
            client: MemoryClient {
                connection: Arc::new(Mutex::new(connection)),
                workspace: Arc::new(workspace),
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
