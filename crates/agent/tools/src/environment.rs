use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct WorkingDirectory(PathBuf);

impl WorkingDirectory {
    pub fn validate(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "本地工作目录必须为绝对路径",
            ));
        }
        let path = path.canonicalize()?;
        if !path.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "本地工作目录不是文件夹",
            ));
        }
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn resolve(&self, path: &Path) -> PathBuf {
        self.0.join(path)
    }

    pub fn directory(&self, candidate: Option<&Path>) -> io::Result<Self> {
        Self::validate(
            &candidate
                .map(|path| self.resolve(path))
                .unwrap_or_else(|| self.0.clone()),
        )
    }
}
