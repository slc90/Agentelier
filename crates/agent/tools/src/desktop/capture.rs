use super::{ScreenRect, WindowId, window};
use crate::payload::{MAX_ENCODED_IMAGE_BYTES, MAX_IMAGE_EDGE};
use crate::{ImageDescriptor, ImagePayload, ToolFailure, process::powershell_path};
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use winsafe::{HDC, prelude::Handle};

const CAPTURE_COMMAND: &str = include_str!("capture.ps1");
const MAX_CAPTURE_PIXELS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowPoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenshotPixel {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Geometry {
    monitors: Vec<ScreenRect>,
    region: ScreenRect,
    target: Option<(WindowId, ScreenRect)>,
}

pub struct Capture {
    image: ImagePayload,
    mapping: CaptureMapping,
}

#[derive(Clone)]
pub(crate) struct CaptureMapping {
    size: (u32, u32),
    geometry: Geometry,
}

impl Capture {
    pub fn image(&self) -> &ImagePayload {
        &self.image
    }

    pub fn into_image(self) -> ImagePayload {
        self.image
    }

    pub fn screen_bounds(&self) -> ScreenRect {
        self.mapping.screen_bounds()
    }

    pub fn window_bounds(&self) -> Option<ScreenRect> {
        self.mapping.window_bounds()
    }

    pub fn screen_point(&self, pixel: ScreenshotPixel) -> Result<ScreenPoint, ToolFailure> {
        self.mapping.screen_point(pixel)
    }

    pub fn window_point(&self, pixel: ScreenshotPixel) -> Result<WindowPoint, ToolFailure> {
        self.mapping.window_point(pixel)
    }

    pub(crate) fn into_parts(self) -> (ImagePayload, CaptureMapping) {
        (self.image, self.mapping)
    }
}

impl CaptureMapping {
    pub(crate) fn screen_bounds(&self) -> ScreenRect {
        self.geometry.region
    }

    pub(crate) fn window_bounds(&self) -> Option<ScreenRect> {
        self.geometry.target.map(|(_, bounds)| bounds)
    }

    pub(crate) fn screen_point(&self, pixel: ScreenshotPixel) -> Result<ScreenPoint, ToolFailure> {
        if geometry(self.geometry.target.map(|(id, _)| id))? != self.geometry {
            return Err(geometry_changed());
        }
        let (width, height) = self.size;
        if pixel.x >= width || pixel.y >= height {
            return Err(outside_capture());
        }
        let region = self.geometry.region;
        let point = ScreenPoint {
            x: pixel_center(pixel.x, width, region.left, region.right),
            y: pixel_center(pixel.y, height, region.top, region.bottom),
        };
        if self
            .geometry
            .monitors
            .iter()
            .any(|rect| rect.contains(point))
        {
            Ok(point)
        } else {
            Err(outside_capture())
        }
    }

    pub(crate) fn window_point(&self, pixel: ScreenshotPixel) -> Result<WindowPoint, ToolFailure> {
        let bounds = self.window_bounds().ok_or_else(|| {
            ToolFailure::new("window_capture_required", "只有窗口截图具有窗口局部坐标")
        })?;
        let point = self.screen_point(pixel)?;
        Ok(WindowPoint {
            x: point.x.abs_diff(bounds.left),
            y: point.y.abs_diff(bounds.top),
        })
    }
}

impl ScreenRect {
    pub fn dimensions(self) -> (u32, u32) {
        (
            self.right.abs_diff(self.left),
            self.bottom.abs_diff(self.top),
        )
    }

    pub fn contains(self, point: ScreenPoint) -> bool {
        (self.left..self.right).contains(&point.x) && (self.top..self.bottom).contains(&point.y)
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let result = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (result.left < result.right && result.top < result.bottom).then_some(result)
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

pub fn window_to_screen(id: WindowId, point: WindowPoint) -> Result<ScreenPoint, ToolFailure> {
    let (_, bounds) = visible_target(id)?;
    let x = i32::try_from(i64::from(bounds.left) + i64::from(point.x))
        .map_err(|_| outside_capture())?;
    let y =
        i32::try_from(i64::from(bounds.top) + i64::from(point.y)).map_err(|_| outside_capture())?;
    let resolved = ScreenPoint { x, y };
    if bounds.contains(resolved) && monitor_bounds()?.iter().any(|rect| rect.contains(resolved)) {
        Ok(resolved)
    } else {
        Err(outside_capture())
    }
}

pub fn capture_desktop() -> Result<Capture, ToolFailure> {
    capture(None)
}

pub fn capture_window(id: WindowId) -> Result<Capture, ToolFailure> {
    capture(Some(id))
}

fn capture(target: Option<WindowId>) -> Result<Capture, ToolFailure> {
    let before = geometry(target)?;
    let image = capture_region(before.region)?;
    if geometry(target)? != before {
        return Err(geometry_changed());
    }
    Ok(Capture {
        mapping: CaptureMapping {
            size: image.dimensions(),
            geometry: before,
        },
        image,
    })
}

pub(super) fn monitor_bounds() -> Result<Vec<ScreenRect>, ToolFailure> {
    let mut monitors = Vec::new();
    HDC::NULL
        .EnumDisplayMonitors(None, |_, _, rect| {
            if rect.left < rect.right && rect.top < rect.bottom {
                monitors.push(ScreenRect {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                });
            }
            true
        })
        .map_err(|error| ToolFailure::new("desktop_layout_failed", error.to_string()))?;
    if monitors.is_empty() {
        return Err(ToolFailure::new(
            "desktop_layout_failed",
            "当前桌面没有显示器",
        ));
    }
    monitors.sort_unstable_by_key(|rect| (rect.left, rect.top, rect.right, rect.bottom));
    Ok(monitors)
}

fn visible_target(id: WindowId) -> Result<(WindowId, ScreenRect), ToolFailure> {
    let info = window(id)?;
    if info.minimized {
        return Err(ToolFailure::new("window_minimized", "目标窗口已最小化"));
    }
    Ok((id, info.bounds))
}

fn geometry(target: Option<WindowId>) -> Result<Geometry, ToolFailure> {
    let monitors = monitor_bounds()?;
    let target = target.map(visible_target).transpose()?;
    let region = match target {
        Some((_, bounds)) => monitors
            .iter()
            .filter_map(|monitor| bounds.intersection(*monitor))
            .reduce(ScreenRect::union),
        None => monitors.iter().copied().reduce(ScreenRect::union),
    }
    .ok_or_else(|| ToolFailure::new("capture_offscreen", "目标范围未与任何显示器相交"))?;
    Ok(Geometry {
        monitors,
        region,
        target,
    })
}

fn capture_region(region: ScreenRect) -> Result<ImagePayload, ToolFailure> {
    let (width, height) = region.dimensions();
    if u64::from(width) * u64::from(height) > MAX_CAPTURE_PIXELS {
        return Err(ToolFailure::new(
            "capture_size_limit",
            "截图源尺寸超过像素预算",
        ));
    }
    let edge = width.max(height);
    let scaled_edge = edge.min(MAX_IMAGE_EDGE);
    let scale = |dimension: u32| {
        (u64::from(dimension) * u64::from(scaled_edge) / u64::from(edge)).max(1) as u32
    };
    let (output_width, output_height) = (scale(width), scale(height));
    let worker = powershell_path().map_err(capture_failed)?;
    let output = Command::new(worker)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            CAPTURE_COMMAND,
        ])
        .envs([
            ("AGENTELIER_CAPTURE_X", region.left.to_string()),
            ("AGENTELIER_CAPTURE_Y", region.top.to_string()),
            ("AGENTELIER_CAPTURE_WIDTH", width.to_string()),
            ("AGENTELIER_CAPTURE_HEIGHT", height.to_string()),
            ("AGENTELIER_CAPTURE_OUT_WIDTH", output_width.to_string()),
            ("AGENTELIER_CAPTURE_OUT_HEIGHT", output_height.to_string()),
            (
                "AGENTELIER_CAPTURE_MAX_BYTES",
                MAX_ENCODED_IMAGE_BYTES.to_string(),
            ),
        ])
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .output()
        .map_err(capture_failed)?;
    if !output.status.success() {
        return Err(capture_failed(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let mime = match image::guess_format(&output.stdout) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        _ => return Err(capture_failed("截图进程未返回 PNG 或 JPEG")),
    };
    let descriptor = ImageDescriptor {
        mime: mime.into(),
        digest: format!("{:x}", Sha256::digest(&output.stdout)),
        width: output_width,
        height: output_height,
        encoded_bytes: output.stdout.len(),
    };
    ImagePayload::from_encoded(&descriptor, output.stdout).map_err(capture_failed)
}

fn pixel_center(point: u32, size: u32, start: i32, end: i32) -> i32 {
    let span = i64::from(end) - i64::from(start);
    let numerator = i64::from(point) * 2 + 1;
    (i64::from(start) + numerator * span / (i64::from(size) * 2)) as i32
}

fn outside_capture() -> ToolFailure {
    ToolFailure::new(
        "coordinate_outside_capture",
        "坐标位于截图、窗口或显示器范围外",
    )
}

fn geometry_changed() -> ToolFailure {
    ToolFailure::new(
        "capture_geometry_changed",
        "窗口边界或显示器布局已变化，请重新截图",
    )
}

fn capture_failed(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new("capture_failed", format!("Windows 屏幕捕获失败：{error}"))
}
