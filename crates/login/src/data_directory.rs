use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;
use winsafe::{self as win, co, prelude::*};

const FILE_SHARE_READ_WRITE: u32 = 0x0000_0001 | 0x0000_0002;
const CREDENTIAL_UPDATE_ACCESS: u32 = 0x8000_0000 | 0x4000_0000 | 0x0001_0000;
const PROBE_CONTENT: &[u8] = b"Agentelier directory access check";

#[derive(Clone)]
pub struct DataEnvironment {
    root: PathBuf,
    credentials: PathBuf,
    _occupation: Arc<File>,
}

impl DataEnvironment {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn credentials_path(&self) -> &Path {
        &self.credentials
    }

    pub fn acquire(root: &Path) -> io::Result<Self> {
        let root = normalize_root(root)?;
        // Denying delete sharing keeps another process from replacing the locked file.
        let occupation = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ_WRITE)
            .open(root.join(".agentelier.lock"))?;
        match occupation.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "数据目录正在被另一个 Agentelier 实例使用，请关闭该实例后重试或重新选择目录",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        check_access(&root)?;
        Ok(Self {
            credentials: root.join("auth.json"),
            root,
            _occupation: Arc::new(occupation),
        })
    }

    pub fn open_in_explorer(&self) -> io::Result<()> {
        let root = normalize_root(&self.root)?;
        let path = root.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "目录路径无法交给 Windows Shell",
            )
        })?;
        let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
        };
        let _com = win::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
            .map_err(io::Error::other)?;
        win::HWND::NULL
            .ShellExecute("open", &path, None, None, co::SW::SHOWNORMAL)
            .map_err(io::Error::other)
    }
}

pub fn validate_data_directory(root: &Path) -> io::Result<PathBuf> {
    let root = normalize_root(root)?;
    check_access(&root)?;
    Ok(root)
}

fn normalize_root(root: &Path) -> io::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "数据目录必须是绝对路径，请重新选择",
        ));
    }
    if !fs::metadata(root)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, "路径不是目录"));
    }
    fs::read_dir(root)?;
    fs::canonicalize(root)
}

fn check_access(root: &Path) -> io::Result<()> {
    let credentials = root.join("auth.json");
    // Replacing an existing credential file also requires DELETE access on Windows.
    match OpenOptions::new()
        .access_mode(CREDENTIAL_UPDATE_ACCESS)
        .open(&credentials)
    {
        Ok(file) => {
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "auth.json 不是普通文件",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    probe_access(root)
}

fn probe_access(root: &Path) -> io::Result<()> {
    let target = NamedTempFile::with_prefix_in(".agentelier-probe-", root)?;
    let mut update = NamedTempFile::with_prefix_in(".agentelier-probe-", root)?;
    update.write_all(PROBE_CONTENT)?;
    update.as_file().sync_all()?;
    let target = target.into_temp_path();
    let mut committed = update.persist(&target)?;
    committed.seek(SeekFrom::Start(0))?;
    let mut content = Vec::new();
    committed.read_to_end(&mut content)?;
    if content != PROBE_CONTENT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "目录读写检查结果不一致",
        ));
    }
    drop(committed);
    target.close()
}
