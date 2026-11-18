use rusqlite::Connection;
use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct Memory {
    connection: Connection,
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
        transaction.commit().map_err(io::Error::other)?;
        Ok(Self { connection })
    }

    pub fn close(self) -> io::Result<()> {
        self.connection
            .close()
            .map_err(|(_, error)| io::Error::other(error))
    }
}

fn check_path(path: &Path, directory: bool) -> io::Result<()> {
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
