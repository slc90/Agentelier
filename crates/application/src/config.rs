use bevy::prelude::Resource;
use bevy_widgetry_path_display::display_path;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::NamedTempFile;

const DEFAULT_SCALE_FACTOR: f32 = 1.25;
const ALLOWED_SCALE_FACTORS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];
const DEFAULT_BRP_PORT: u16 = 15702;
const RENDER_WORLD_BRP_PORT: u16 = 15703;
const BRP_PORT_RANGE: std::ops::RangeInclusive<u16> = 1024..=65534;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppConfig {
    pub(crate) scale_factor: f32,
    pub(crate) brp_enabled: bool,
    pub(crate) brp_port: u16,
    #[serde(default)]
    pub(crate) data_directory: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            scale_factor: DEFAULT_SCALE_FACTOR,
            brp_enabled: true,
            brp_port: DEFAULT_BRP_PORT,
            data_directory: None,
        }
    }
}

impl AppConfig {
    fn validate(&self) -> io::Result<()> {
        if !ALLOWED_SCALE_FACTORS.contains(&self.scale_factor) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "scale_factor 必须为 1.0、1.25、1.5 或 2.0",
            ));
        }
        if !BRP_PORT_RANGE.contains(&self.brp_port) || self.brp_port == RENDER_WORLD_BRP_PORT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "brp_port 必须为 1024..=65534，且不能占用 15703",
            ));
        }
        Ok(())
    }

    fn serialized(&self) -> io::Result<String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(io::Error::other)
    }

    fn save(&self, file: &ConfigFile) -> io::Result<()> {
        file.prepare(self)?.persist(&file.path)?;
        Ok(())
    }
}

pub(crate) struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    fn prepare(&self, config: &AppConfig) -> io::Result<NamedTempFile> {
        let content = config.serialized()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "配置文件缺少父目录"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::with_prefix_in(".agentelier-config-", parent)?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        Ok(temporary)
    }

    fn create_default(&self, config: &AppConfig) -> io::Result<()> {
        self.prepare(config)?.persist_noclobber(&self.path)?;
        Ok(())
    }
}

pub(crate) struct LoadedConfig {
    pub(crate) config: AppConfig,
    pub(crate) file: ConfigFile,
}

#[derive(Resource, Clone)]
pub(crate) struct ConfigStore(Arc<Mutex<LoadedConfig>>);

impl ConfigStore {
    pub(crate) fn new(loaded: LoadedConfig) -> Self {
        Self(Arc::new(Mutex::new(loaded)))
    }

    pub(crate) fn remember_data_directory(&self, path: &Path) -> io::Result<()> {
        let mut current = self
            .0
            .lock()
            .map_err(|_| io::Error::other("应用配置状态不可用"))?;
        let mut updated = current.config.clone();
        updated.data_directory = Some(path.to_path_buf());
        updated.save(&current.file).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("配置 {}：{error}", display_path(&current.file.path)),
            )
        })?;
        current.config = updated;
        Ok(())
    }
}

pub(crate) fn load(path: PathBuf) -> io::Result<LoadedConfig> {
    let file = ConfigFile { path };
    let config = match fs::read_to_string(&file.path) {
        Ok(content) => ron::from_str::<AppConfig>(&content).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let config = AppConfig::default();
            file.create_default(&config)?;
            config
        }
        Err(error) => return Err(error),
    };
    config.validate()?;
    Ok(LoadedConfig { config, file })
}
