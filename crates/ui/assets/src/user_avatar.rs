use bevy::asset::RenderAssetUsages;
use bevy::prelude::Image;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use image::{ImageFormat, ImageReader, Limits, guess_format};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;
use uuid::Uuid;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
const PROFILE: &str = "profile";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarFile {
    id: Uuid,
    format: AvatarFormat,
}

impl AvatarFile {
    fn path(self, root: &Path) -> PathBuf {
        let extension = match self.format {
            AvatarFormat::Png => "png",
            AvatarFormat::Jpeg => "jpg",
        };
        root.join(PROFILE)
            .join(format!("avatar-{}.{extension}", self.id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum AvatarFormat {
    Png,
    Jpeg,
}

#[derive(Clone)]
pub struct AvatarOriginal {
    bytes: Arc<[u8]>,
    format: AvatarFormat,
}

pub struct LoadedAvatar {
    pub original: AvatarOriginal,
    pub image: Image,
}

impl LoadedAvatar {
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "头像文件超过 32 MiB",
            ));
        }
        let kind = guess_format(&bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "头像必须是 PNG 或 JPEG 图片")
        })?;
        let format = match kind {
            ImageFormat::Png => AvatarFormat::Png,
            ImageFormat::Jpeg => AvatarFormat::Jpeg,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "头像必须是 PNG 或 JPEG 图片",
                ));
            }
        };
        let (width, height) = ImageReader::with_format(Cursor::new(bytes.as_slice()), kind)
            .into_dimensions()
            .map_err(decode_error)?;
        if width != height || !matches!(width, 1..=MAX_SIDE) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "头像必须为不超过 4096 × 4096 的方形图片",
            ));
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), kind);
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let rgba = reader.decode().map_err(decode_error)?.into_rgba8();
        let image = Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            rgba.into_raw(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        Ok(Self {
            original: AvatarOriginal {
                bytes: bytes.into(),
                format,
            },
            image,
        })
    }

    pub fn restore(root: &Path, file: AvatarFile) -> io::Result<Self> {
        let loaded = Self::read(&file.path(root))?;
        if loaded.original.format != file.format {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "头像原图与配置格式不一致",
            ));
        }
        Ok(loaded)
    }
}

impl AvatarOriginal {
    pub fn store(&self, root: &Path) -> io::Result<AvatarFile> {
        let file = AvatarFile {
            id: Uuid::new_v4(),
            format: self.format,
        };
        let profile = root.join(PROFILE);
        fs::create_dir_all(&profile)?;
        let mut temporary = NamedTempFile::with_prefix_in(".avatar-", &profile)?;
        temporary.write_all(&self.bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(file.path(root))?;
        Ok(file)
    }
}

impl AvatarFile {
    pub fn discard(self, root: &Path) -> io::Result<()> {
        match fs::remove_file(self.path(root)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn decode_error(error: image::ImageError) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("无法解码头像图片：{error}"),
    )
}
