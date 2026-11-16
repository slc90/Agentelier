use crate::{
    ImagePayload, ToolFailure, ToolPayload, ToolResult, ToolStatus, WorkingDirectory, blocking,
};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Cursor, Read, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
const MAX_PIXELS: u64 = 25_000_000;
const MAX_BYTES_PER_PIXEL: u64 = 8;
use crate::payload::{MAX_ENCODED_IMAGE_BYTES as MAX_ENCODED_BYTES, MAX_IMAGE_EDGE as MAX_EDGE};
const JPEG_QUALITY: u8 = 85;
const PNG_DIMENSIONS_START: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: PathBuf,
}

fn failure(code: &'static str, error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(code, error.to_string())
}

struct Encoded(Vec<u8>);
impl Write for Encoded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ENCODED_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("规范化图像超过 4 MiB，未生成截断载荷"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn read(
    input: Arguments,
    directory: PathBuf,
    control: &blocking::Control,
) -> Result<ToolPayload, ToolFailure> {
    control.check()?;
    let path = WorkingDirectory::validate(&directory)
        .and_then(|base| base.resolve(&input.path))
        .map_err(|error| failure("image_path", error))?;
    if !std::fs::metadata(&path)
        .map_err(|error| failure("image_read", format!("{}: {error}", path.display())))?
        .is_file()
    {
        return Err(failure(
            "image_type",
            format!("图片路径不是普通文件: {}", path.display()),
        ));
    }
    let file = File::open(&path)
        .map_err(|error| failure("image_read", format!("{}: {error}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|error| failure("image_read", error))?;
    if !metadata.is_file() {
        return Err(failure("image_type", "图片路径不是普通文件"));
    }
    if metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err(failure("image_input_limit", "原始图片超过 10 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failure("image_read", error))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(failure("image_input_limit", "实际读取图片超过 10 MiB"));
    }
    control.check()?;
    let source_digest = format!("{:x}", Sha256::digest(&bytes));
    let format = image::guess_format(&bytes).map_err(|error| failure("image_format", error))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * MAX_BYTES_PER_PIXEL);
    if format == ImageFormat::Png {
        let dimension = |range: std::ops::Range<usize>| -> Result<u32, ToolFailure> {
            let raw = bytes
                .get(range)
                .ok_or_else(|| failure("image_decode", "PNG IHDR 不完整"))?;
            Ok(u32::from_be_bytes(raw.try_into().map_err(|_| {
                failure("image_decode", "PNG IHDR 尺寸无效")
            })?))
        };
        let height_start = PNG_DIMENSIONS_START + size_of::<u32>();
        check_dimensions(
            dimension(PNG_DIMENSIONS_START..height_start)?,
            dimension(height_start..height_start + size_of::<u32>())?,
        )?;
    }
    let mut decoder: Box<dyn ImageDecoder> = match format {
        ImageFormat::Png => {
            let decoder =
                image::codecs::png::PngDecoder::with_limits(Cursor::new(&bytes), limits.clone())
                    .map_err(|error| failure("image_decode", error))?;
            if decoder
                .is_apng()
                .map_err(|error| failure("image_decode", error))?
            {
                return Err(failure(
                    "image_format",
                    "只支持静态 PNG 和 JPEG，不支持 APNG",
                ));
            }
            Box::new(decoder)
        }
        ImageFormat::Jpeg => Box::new(
            image::codecs::jpeg::JpegDecoder::new(Cursor::new(&bytes))
                .map_err(|error| failure("image_decode", error))?,
        ),
        _ => return Err(failure("image_format", "只支持静态 PNG 和 JPEG")),
    };
    let (width, height) = decoder.dimensions();
    check_dimensions(width, height)?;
    decoder
        .set_limits(limits)
        .map_err(|error| failure("image_decode", error))?;
    control.check()?;
    let orientation = decoder
        .orientation()
        .map_err(|error| failure("image_decode", error))?;
    let mut decoded =
        DynamicImage::from_decoder(decoder).map_err(|error| failure("image_decode", error))?;
    decoded.apply_orientation(orientation);
    control.check()?;
    let resized = if width.max(height) > MAX_EDGE {
        decoded.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let (sent_width, sent_height) = (resized.width(), resized.height());
    let mut encoded = Encoded(Vec::new());
    let mime = if format == ImageFormat::Png {
        let pixels = resized.to_rgba8();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(
                &pixels,
                sent_width,
                sent_height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| failure("image_encode", error))?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
            .encode_image(&resized.to_rgb8())
            .map_err(|error| failure("image_encode", error))?;
        "image/jpeg"
    };
    control.check()?;
    let digest = format!("{:x}", Sha256::digest(&encoded.0));
    let summary = json!({"ok":true,"data":{"path":path,"original_format":if format==ImageFormat::Png {"png"} else {"jpeg"},"original_width":width,"original_height":height,"width":sent_width,"height":sent_height,"mime":mime,"source_sha256":source_digest,"sha256":digest,"encoded_bytes":encoded.0.len()}}).to_string();
    if summary.len() > crate::MAX_TOOL_BODY_BYTES {
        return Err(failure("image_summary_limit", "图片说明超过文字预算"));
    }
    Ok(ToolPayload::Image {
        summary,
        image: Arc::new(ImagePayload {
            bytes: encoded.0,
            mime,
            digest,
            width: sent_width,
            height: sent_height,
        }),
    })
}

fn check_dimensions(width: u32, height: u32) -> Result<(), ToolFailure> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        Err(failure(
            "image_pixel_limit",
            "解码图片超过 2500 万像素或尺寸无效",
        ))
    } else {
        Ok(())
    }
}

pub(crate) async fn run(text: &str, directory: PathBuf, cancel: &CancellationToken) -> ToolResult {
    let input = match crate::process::arguments(text, &["path"]) {
        Ok(input) => input,
        Err(error) => return blocking::failure(error),
    };
    blocking::run(cancel, move |control| {
        match read(input, directory, &control) {
            Ok(output) => ToolResult {
                status: ToolStatus::Succeeded,
                output,
                cleanup_failed: false,
            },
            Err(error) => blocking::failure(error),
        }
    })
    .await
}
