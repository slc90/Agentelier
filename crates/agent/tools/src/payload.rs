use image::{ImageDecoder, ImageFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Cursor};
use std::sync::Arc;

pub const MAX_SESSION_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ENCODED_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 2048;
pub const MAX_ORIGINAL_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_SESSION_ORIGINAL_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ORIGINAL_IMAGE_EDGE: u32 = 4096;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDescriptor {
    pub mime: String,
    pub digest: String,
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: usize,
}

impl ImageDescriptor {
    pub fn extension(&self) -> io::Result<&'static str> {
        match self.mime.as_str() {
            "image/png" => Ok("png"),
            "image/jpeg" => Ok("jpg"),
            _ => Err(io::Error::other("附件格式不是 PNG 或 JPEG")),
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        self.validate_limits(MAX_ENCODED_IMAGE_BYTES, MAX_IMAGE_EDGE)
    }

    pub fn validate_original(&self) -> io::Result<()> {
        self.validate_limits(MAX_ORIGINAL_IMAGE_BYTES, MAX_ORIGINAL_IMAGE_EDGE)
    }

    fn validate_limits(&self, max_bytes: usize, max_edge: u32) -> io::Result<()> {
        const SHA256_HEX_LENGTH: usize = 64;
        self.extension()?;
        if self.digest.len() != SHA256_HEX_LENGTH
            || !self
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(io::Error::other("附件摘要不是有效的 SHA-256"));
        }
        if self.encoded_bytes == 0
            || self.encoded_bytes > max_bytes
            || self.width == 0
            || self.height == 0
            || self.width.max(self.height) > max_edge
        {
            return Err(io::Error::other("附件尺寸或编码字节数超出图片工具限额"));
        }
        Ok(())
    }
}

pub struct ImagePayload {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mime: &'static str,
    pub(crate) digest: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}
impl ImagePayload {
    pub fn descriptor(&self) -> ImageDescriptor {
        ImageDescriptor {
            mime: self.mime.into(),
            digest: self.digest.clone(),
            width: self.width,
            height: self.height,
            encoded_bytes: self.bytes.len(),
        }
    }

    pub fn from_encoded(descriptor: &ImageDescriptor, bytes: Vec<u8>) -> io::Result<Self> {
        descriptor.validate()?;
        if bytes.len() != descriptor.encoded_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != descriptor.digest
        {
            return Err(io::Error::other("附件长度或内容摘要不符"));
        }
        let (decoded, mime) = decode(&bytes, MAX_IMAGE_EDGE)?;
        if (decoded.width(), decoded.height()) != (descriptor.width, descriptor.height)
            || mime != descriptor.mime
        {
            return Err(io::Error::other("附件解码尺寸不符"));
        }
        Ok(Self {
            bytes,
            mime,
            digest: descriptor.digest.clone(),
            width: descriptor.width,
            height: descriptor.height,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn mime(&self) -> &'static str {
        self.mime
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

fn decode(bytes: &[u8], edge: u32) -> io::Result<(image::DynamicImage, &'static str)> {
    let mut limits = image::Limits::default();
    const MAX_BYTES_PER_PIXEL: u64 = 8;
    limits.max_image_width = Some(edge);
    limits.max_image_height = Some(edge);
    limits.max_alloc = Some(u64::from(edge).pow(2) * MAX_BYTES_PER_PIXEL);
    let (mut decoder, mime): (Box<dyn ImageDecoder>, _) =
        match image::guess_format(bytes).map_err(io::Error::other)? {
            ImageFormat::Png => {
                let png =
                    image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits.clone())
                        .map_err(io::Error::other)?;
                if png.is_apng().map_err(io::Error::other)? {
                    return Err(io::Error::other("附件不是静态 PNG"));
                }
                (Box::new(png), "image/png")
            }
            ImageFormat::Jpeg => (
                Box::new(
                    image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes))
                        .map_err(io::Error::other)?,
                ),
                "image/jpeg",
            ),
            _ => return Err(io::Error::other("附件格式不是 PNG 或 JPEG")),
        };
    decoder.set_limits(limits).map_err(io::Error::other)?;
    let decoded = image::DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
    Ok((decoded, mime))
}

pub const GENERATED_IMAGE_MODEL: &str = "gpt-image-2";
pub const GENERATE_IMAGE_TOOL: &str = "generate_image";
pub const PUBLISH_AUDIO_TOOL: &str = "publish_audio";
pub const PUBLISH_SCENE_TOOL: &str = "publish_scene";
pub const MAX_GENERATION_PROMPT_CHARS: usize = 8000;

pub struct OriginalImage {
    bytes: Vec<u8>,
    descriptor: ImageDescriptor,
}

impl OriginalImage {
    pub fn decode(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_ORIGINAL_IMAGE_BYTES {
            return Err(io::Error::other("原图编码长度超出限额"));
        }
        let (decoded, mime) = decode(&bytes, MAX_ORIGINAL_IMAGE_EDGE)?;
        let descriptor = ImageDescriptor {
            mime: mime.into(),
            digest: format!("{:x}", Sha256::digest(&bytes)),
            width: decoded.width(),
            height: decoded.height(),
            encoded_bytes: bytes.len(),
        };
        descriptor.validate_original()?;
        Ok(Self { bytes, descriptor })
    }

    pub fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_model_image(self) -> io::Result<ImagePayload> {
        let image = self.descriptor;
        if image.validate().is_ok() {
            let mime = match image.extension()? {
                "png" => "image/png",
                "jpg" => "image/jpeg",
                _ => return Err(io::Error::other("模型图片格式无效")),
            };
            return Ok(ImagePayload {
                bytes: self.bytes,
                mime,
                digest: image.digest,
                width: image.width,
                height: image.height,
            });
        }
        let (decoded, mime) = decode(&self.bytes, MAX_ORIGINAL_IMAGE_EDGE)?;
        crate::image::normalize(decoded, mime == "image/png").map_err(io::Error::other)
    }
}

#[derive(Clone)]
pub enum ToolPayload {
    Text(String),
    Image {
        summary: String,
        image: Arc<ImagePayload>,
    },
    GeneratedImage {
        prompt: String,
        image: Arc<OriginalImage>,
    },
    PublishAudio(std::path::PathBuf),
    PublishScene(Box<crate::ecs_access::SceneDefinition>),
}
impl From<String> for ToolPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl ToolPayload {
    pub fn summary(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Image { summary, .. } => summary,
            Self::GeneratedImage { .. } => "图片已生成，等待保存原件",
            Self::PublishAudio(_) => "音频等待保存到会话",
            Self::PublishScene(_) => "Scene 等待保存到会话",
        }
    }
    pub fn image(&self) -> Option<&Arc<ImagePayload>> {
        match self {
            Self::Image { image, .. } => Some(image),
            Self::Text(_)
            | Self::GeneratedImage { .. }
            | Self::PublishAudio(_)
            | Self::PublishScene(_) => None,
        }
    }
}
