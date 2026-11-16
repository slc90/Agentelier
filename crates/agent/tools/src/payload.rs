use image::{ImageDecoder, ImageFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Cursor};
use std::sync::Arc;

pub const MAX_SESSION_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ENCODED_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 2048;

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
            || self.encoded_bytes > MAX_ENCODED_IMAGE_BYTES
            || self.width == 0
            || self.height == 0
            || self.width.max(self.height) > MAX_IMAGE_EDGE
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
        let (format, mime) = match descriptor.mime.as_str() {
            "image/png" => (ImageFormat::Png, "image/png"),
            "image/jpeg" => (ImageFormat::Jpeg, "image/jpeg"),
            _ => return Err(io::Error::other("附件格式无效")),
        };
        if image::guess_format(&bytes).map_err(io::Error::other)? != format {
            return Err(io::Error::other("附件内容与 MIME 不一致"));
        }
        let mut limits = image::Limits::default();
        const MAX_BYTES_PER_PIXEL: u64 = 8;
        limits.max_alloc = Some(u64::from(MAX_IMAGE_EDGE).pow(2) * MAX_BYTES_PER_PIXEL);
        let mut decoder: Box<dyn ImageDecoder> = match format {
            ImageFormat::Png => {
                let png = image::codecs::png::PngDecoder::with_limits(
                    Cursor::new(&bytes),
                    limits.clone(),
                )
                .map_err(io::Error::other)?;
                if png.is_apng().map_err(io::Error::other)? {
                    return Err(io::Error::other("附件不是静态 PNG"));
                }
                Box::new(png)
            }
            ImageFormat::Jpeg => Box::new(
                image::codecs::jpeg::JpegDecoder::new(Cursor::new(&bytes))
                    .map_err(io::Error::other)?,
            ),
            _ => return Err(io::Error::other("附件解码格式无效")),
        };
        if decoder.dimensions() != (descriptor.width, descriptor.height) {
            return Err(io::Error::other("附件解码尺寸不符"));
        }
        decoder.set_limits(limits).map_err(io::Error::other)?;
        image::DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
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

#[derive(Clone)]
pub enum ToolPayload {
    Text(String),
    Image {
        summary: String,
        image: Arc<ImagePayload>,
    },
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
        }
    }
    pub fn image(&self) -> Option<&Arc<ImagePayload>> {
        match self {
            Self::Image { image, .. } => Some(image),
            Self::Text(_) => None,
        }
    }
}
