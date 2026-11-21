use crate::ToolFailure;
use std::thread;
use std::time::{Duration, Instant};
use winsafe::{DwmAttr, HWND, co};

mod capture;
mod input;
pub(crate) use capture::CaptureMapping;
pub use capture::{
    Capture, ScreenPoint, ScreenshotPixel, WindowPoint, capture_desktop, capture_window,
    window_to_screen,
};
pub use input::{
    DesktopInput, Key, MouseButton, PointerTarget, ScrollAxis, TrajectoryPoint, cursor_position,
};

const ACTIVATION_TIMEOUT: Duration = Duration::from_millis(300);
const ACTIVATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowId {
    handle: usize,
    process: u32,
    thread: u32,
}

impl WindowId {
    pub fn handle(self) -> usize {
        self.handle
    }

    pub(crate) fn token(self) -> String {
        format!("{:x}:{}:{}", self.handle, self.process, self.thread)
    }

    pub(crate) fn from_token(token: &str) -> Option<Self> {
        let (handle, owner) = token.split_once(':')?;
        let (process, thread) = owner.split_once(':')?;
        Some(Self {
            handle: usize::from_str_radix(handle, 16).ok()?,
            process: process.parse().ok()?,
            thread: thread.parse().ok()?,
        })
    }

    fn read(window: &HWND) -> Self {
        let (thread, process) = window.GetWindowThreadProcessId();
        Self {
            handle: window.ptr().addr(),
            process,
            thread,
        }
    }

    fn matches(self, window: &HWND) -> bool {
        self.handle == window.ptr().addr() && self == Self::read(window)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub bounds: ScreenRect,
    pub minimized: bool,
    pub foreground: bool,
}

pub fn windows() -> Result<Vec<WindowInfo>, ToolFailure> {
    let mut windows = Vec::new();
    for window in top_level_windows()? {
        if let Some(info) = read_window(&window)? {
            windows.push(info);
        }
    }
    Ok(windows)
}

pub fn window(id: WindowId) -> Result<WindowInfo, ToolFailure> {
    let target = resolve(id)?;
    read_window(&target)?
        .filter(|info| info.id == id)
        .ok_or_else(window_unavailable)
}

pub fn activate_window(id: WindowId) -> Result<WindowInfo, ToolFailure> {
    let target = resolve(id)?;
    let initial = read_window(&target)?
        .filter(|info| info.id == id)
        .ok_or_else(window_unavailable)?;
    if initial.minimized {
        return Err(ToolFailure::new("window_minimized", "目标窗口已最小化"));
    }
    if initial.foreground {
        return Ok(initial);
    }

    let requested = target.SetForegroundWindow();
    let deadline = Instant::now() + ACTIVATION_TIMEOUT;
    loop {
        if !id.matches(&target) {
            return Err(window_unavailable());
        }
        if HWND::GetForegroundWindow().as_ref() == Some(&target) {
            let current = read_window(&target)?
                .filter(|info| info.id == id)
                .ok_or_else(window_unavailable)?;
            if current.foreground && !current.minimized {
                return Ok(current);
            }
        }
        if !requested || Instant::now() >= deadline {
            return Err(ToolFailure::new(
                "window_activation_failed",
                "Windows 未将目标窗口切换到前台",
            ));
        }
        thread::sleep(ACTIVATION_POLL_INTERVAL);
    }
}

fn resolve(id: WindowId) -> Result<HWND, ToolFailure> {
    top_level_windows()?
        .into_iter()
        .find(|window| id.matches(window))
        .ok_or_else(window_unavailable)
}

fn top_level_windows() -> Result<Vec<HWND>, ToolFailure> {
    let mut windows = Vec::new();
    winsafe::EnumWindows(|window| {
        windows.push(window);
        true
    })
    .map_err(|error| ToolFailure::new("window_enumeration_failed", error.to_string()))?;
    Ok(windows)
}

fn read_window(window: &HWND) -> Result<Option<WindowInfo>, ToolFailure> {
    if !window.IsWindowVisible()
        || *window == HWND::GetDesktopWindow()
        || HWND::GetShellWindow().as_ref() == Some(window)
    {
        return Ok(None);
    }
    let style = window.style_ex();
    if style.has(co::WS_EX::TOOLWINDOW) && !style.has(co::WS_EX::APPWINDOW) {
        return Ok(None);
    }

    let id = WindowId::read(window);
    let info = read_details(window, id);
    if id.process == 0 || !id.matches(window) || !window.IsWindowVisible() {
        return Ok(None);
    }
    info
}

fn read_details(window: &HWND, id: WindowId) -> Result<Option<WindowInfo>, ToolFailure> {
    let title = window
        .GetWindowText()
        .map_err(|error| ToolFailure::new("window_title_failed", error.to_string()))?;
    if title.trim().is_empty() {
        return Ok(None);
    }
    let cloaked = window
        .DwmGetWindowAttribute(co::DWMWA::CLOAKED)
        .map_err(|error| ToolFailure::new("window_visibility_failed", error.to_string()))?;
    if matches!(cloaked, DwmAttr::Cloaked(flags) if flags.raw() != 0) {
        return Ok(None);
    }

    // DWM frame bounds are physical pixels, regardless of the calling thread's DPI context.
    let frame = window
        .DwmGetWindowAttribute(co::DWMWA::EXTENDED_FRAME_BOUNDS)
        .map_err(|error| ToolFailure::new("window_bounds_failed", error.to_string()))?;
    let DwmAttr::ExtendedFrameBounds(rect) = frame else {
        return Err(ToolFailure::new(
            "window_bounds_failed",
            "Windows 未返回窗口边界",
        ));
    };
    Ok(Some(WindowInfo {
        id,
        title,
        bounds: ScreenRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
        minimized: window.IsIconic(),
        foreground: HWND::GetForegroundWindow().as_ref() == Some(window),
    }))
}

fn window_unavailable() -> ToolFailure {
    ToolFailure::new("window_unavailable", "目标窗口已关闭或已不可用于桌面操作")
}
