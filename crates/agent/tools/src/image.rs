use crate::{
    ImagePayload, ToolFailure, ToolPayload, ToolResult, ToolStatus, WorkingDirectory, blocking,
};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

const MAX_INPUT_BYTES: usize = crate::payload::MAX_PROMPT_IMAGE_BYTES;
const MAX_PIXELS: u64 = 10_240_000;
const MAX_BYTES_PER_PIXEL: u64 = 8;
use crate::payload::MAX_IMAGE_EDGE as MAX_EDGE;
const JPEG_QUALITY: u8 = 90;
const PNG_DIMENSIONS_START: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: PathBuf,
}

fn failure(code: &'static str, error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(code, error.to_string())
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
        return Err(failure("image_input_limit", "原始图片超过 79 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failure("image_read", error))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(failure("image_input_limit", "实际读取图片超过 79 MiB"));
    }
    let (image, original_width, original_height, source_digest) = decode(bytes, None, control)?;
    let summary = json!({"ok":true,"data":{"path":path,"original_format":if image.mime == "image/png" {"png"} else {"jpeg"},"original_width":original_width,"original_height":original_height,"width":image.width,"height":image.height,"mime":image.mime,"source_sha256":source_digest,"sha256":image.digest,"encoded_bytes":image.bytes.len()}}).to_string();
    if summary.len() > crate::MAX_TOOL_BODY_BYTES {
        return Err(failure("image_summary_limit", "图片说明超过文字预算"));
    }
    Ok(ToolPayload::Image {
        summary,
        image: Arc::new(image),
    })
}

fn decode(
    bytes: Vec<u8>,
    declared_mime: Option<&str>,
    control: &blocking::Control,
) -> Result<(ImagePayload, u32, u32, String), ToolFailure> {
    control.check()?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(failure("image_input_limit", "原始图片超过 79 MiB"));
    }
    let source_digest = format!("{:x}", Sha256::digest(&bytes));
    let format = image::guess_format(&bytes).map_err(|error| failure("image_format", error))?;
    if declared_mime.is_some_and(|mime| {
        !matches!(
            (mime, format),
            ("image/png", ImageFormat::Png) | ("image/jpeg", ImageFormat::Jpeg)
        )
    }) {
        return Err(failure(
            "image_format",
            "MCP 图片 MIME 与实际静态 PNG/JPEG 格式不匹配",
        ));
    }
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
    let image = normalize(decoded, format == ImageFormat::Png)?;
    control.check()?;
    Ok((image, width, height, source_digest))
}

pub(crate) fn prompt_dimensions(width: u32, height: u32) -> (u32, u32) {
    const PATCH: u32 = 32;
    const MAX_PATCHES: u32 = 2_500;
    let (width, height) = (width.max(1), height.max(1));
    let edge = width.max(height);
    let scale = (f64::from(MAX_EDGE) / f64::from(edge)).min(1.0);
    let width = (f64::from(width) * scale).round().max(1.0) as u32;
    let height = (f64::from(height) * scale).round().max(1.0) as u32;
    if width.div_ceil(PATCH) * height.div_ceil(PATCH) <= MAX_PATCHES {
        return (width, height);
    }
    let (width, height, patch) = (f64::from(width), f64::from(height), f64::from(PATCH));
    let mut scale = (patch * patch * f64::from(MAX_PATCHES) / width / height).sqrt();
    let horizontal = width * scale / patch;
    let vertical = height * scale / patch;
    scale *= (horizontal.floor() / horizontal).min(vertical.floor() / vertical);
    (
        (width * scale).floor().max(1.0) as u32,
        (height * scale).floor().max(1.0) as u32,
    )
}

pub(crate) fn normalize(decoded: DynamicImage, png: bool) -> Result<ImagePayload, ToolFailure> {
    let (width, height) = prompt_dimensions(decoded.width(), decoded.height());
    let resized = if (width, height) != (decoded.width(), decoded.height()) {
        decoded.resize_exact(width, height, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let (width, height) = (resized.width(), resized.height());
    let mut bytes = Vec::new();
    let mime = if png {
        let (pixels, color) = if resized.color().has_alpha() {
            (
                resized.into_rgba8().into_raw(),
                image::ExtendedColorType::Rgba8,
            )
        } else {
            (
                resized.into_rgb8().into_raw(),
                image::ExtendedColorType::Rgb8,
            )
        };
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Default, FilterType::Adaptive)
            .write_image(&pixels, width, height, color)
            .map_err(|error| failure("image_encode", error))?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
            .encode_image(&resized.into_rgb8())
            .map_err(|error| failure("image_encode", error))?;
        "image/jpeg"
    };
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok(ImagePayload {
        bytes,
        mime,
        digest,
        width,
        height,
    })
}

pub async fn from_mcp_bytes(
    bytes: Vec<u8>,
    mime: String,
    summary: String,
    cancel: &CancellationToken,
) -> ToolResult {
    blocking::run(cancel, move |control| {
        match decode(bytes, Some(&mime), &control) {
            Ok((image, _, _, _)) => ToolResult {
                status: ToolStatus::Succeeded,
                output: ToolPayload::Image {
                    summary,
                    image: Arc::new(image),
                },
                cleanup_failed: false,
            },
            Err(error) => blocking::failure(error),
        }
    })
    .await
}

fn check_dimensions(width: u32, height: u32) -> Result<(), ToolFailure> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        Err(failure(
            "image_pixel_limit",
            "解码图片超过 1024 万像素或尺寸无效",
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
