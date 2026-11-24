use bevy_widgetry_path_display::simplify_path;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use winsafe::{self as win, co, prelude::*};

const PROBE_CONTENT: &[u8] = b"Agentelier directory access check";

#[derive(Clone)]
pub struct DataEnvironment {
    root: PathBuf,
}

impl DataEnvironment {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: &Path) -> io::Result<Self> {
        let root = validate_data_directory(root)?;
        Ok(Self { root })
    }

    pub fn open_in_explorer(&self) -> io::Result<()> {
        let root = normalize_root(&self.root)?;
        let path = simplify_path(&root);
        let path = path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "目录路径无法交给 Windows Shell",
            )
        })?;
        let _com = win::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
            .map_err(io::Error::other)?;
        win::HWND::NULL
            .ShellExecute("open", path, None, None, co::SW::SHOWNORMAL)
            .map_err(io::Error::other)
    }
}

pub fn validate_data_directory(root: &Path) -> io::Result<PathBuf> {
    let root = normalize_root(root)?;
    probe_access(&root)?;
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
