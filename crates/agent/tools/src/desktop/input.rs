use super::capture::monitor_bounds;
use super::{ScreenPoint, WindowId, WindowPoint, window_to_screen};
use crate::ToolFailure;
use std::thread;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use winsafe::{HwKbMouse, MOUSEINPUT, co};

mod keyboard;
use keyboard::HeldKey;
pub use keyboard::Key;

const WHEEL_UNITS_PER_DETENT: i32 = 120;
const CANCELLATION_POLL: Duration = Duration::from_millis(10);
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(25);
const DRAG_EVENT_GAP: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    fn key(self) -> co::VK {
        match self {
            Self::Left => co::VK::LBUTTON,
            Self::Right => co::VK::RBUTTON,
            Self::Middle => co::VK::MBUTTON,
        }
    }

    fn event(self, pressed: bool) -> co::MOUSEEVENTF {
        match (self, pressed) {
            (Self::Left, true) => co::MOUSEEVENTF::LEFTDOWN,
            (Self::Left, false) => co::MOUSEEVENTF::LEFTUP,
            (Self::Right, true) => co::MOUSEEVENTF::RIGHTDOWN,
            (Self::Right, false) => co::MOUSEEVENTF::RIGHTUP,
            (Self::Middle, true) => co::MOUSEEVENTF::MIDDLEDOWN,
            (Self::Middle, false) => co::MOUSEEVENTF::MIDDLEUP,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PointerTarget {
    Screen(ScreenPoint),
    Window(WindowId, WindowPoint),
}

impl PointerTarget {
    fn window(self) -> Option<WindowId> {
        match self {
            Self::Screen(_) => None,
            Self::Window(id, _) => Some(id),
        }
    }

    fn screen(self) -> Result<ScreenPoint, ToolFailure> {
        match self {
            Self::Window(id, point) => window_to_screen(id, point),
            Self::Screen(point) => {
                if monitor_bounds()?.iter().any(|rect| rect.contains(point)) {
                    Ok(point)
                } else {
                    Err(ToolFailure::new(
                        "pointer_offscreen",
                        "鼠标目标不在显示器上",
                    ))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug)]
pub struct TrajectoryPoint {
    pub target: PointerTarget,
    pub after: Duration,
}

pub struct DesktopInput {
    cancel: CancellationToken,
    buttons: Vec<MouseButton>,
    keys: Vec<HeldKey>,
}

impl DesktopInput {
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            buttons: Vec::new(),
            keys: Vec::new(),
        }
    }

    pub fn move_mouse(&self, target: PointerTarget) -> Result<ScreenPoint, ToolFailure> {
        self.check_cancel()?;
        let point = target.screen()?;
        winsafe::SetPhysicalCursorPos(point.x, point.y).map_err(input_failed)?;
        let actual = cursor_position()?;
        if actual == point {
            Ok(actual)
        } else {
            Err(ToolFailure::new(
                "pointer_clipped",
                format!("鼠标未到达 {point:?}，当前坐标为 {actual:?}"),
            ))
        }
    }

    pub fn mouse_down(&mut self, button: MouseButton) -> Result<(), ToolFailure> {
        self.check_cancel()?;
        if self.buttons.contains(&button) {
            return Ok(());
        }
        if winsafe::GetAsyncKeyState(button.key()) {
            return Err(ToolFailure::new(
                "mouse_button_in_use",
                format!("鼠标按钮 {button:?} 已被其他输入按住"),
            ));
        }
        send_mouse(button.event(true), 0)?;
        self.buttons.push(button);
        Ok(())
    }

    pub fn mouse_up(&mut self, button: MouseButton) -> Result<(), ToolFailure> {
        if let Some(index) = self.buttons.iter().position(|held| *held == button) {
            send_mouse(button.event(false), 0)?;
            self.buttons.remove(index);
        }
        Ok(())
    }

    pub fn click(&mut self, button: MouseButton) -> Result<(), ToolFailure> {
        self.with_button(button, |input| input.check_cancel())
    }

    pub fn double_click(&mut self, button: MouseButton) -> Result<(), ToolFailure> {
        self.click(button)?;
        let configured = Duration::from_millis(u64::from(winsafe::GetDoubleClickTime()));
        self.wait(DOUBLE_CLICK_GAP.min(configured / 4))?;
        self.click(button)
    }

    pub fn scroll(&self, axis: ScrollAxis, detents: i32) -> Result<(), ToolFailure> {
        self.check_cancel()?;
        let amount = detents.checked_mul(WHEEL_UNITS_PER_DETENT).ok_or_else(|| {
            ToolFailure::new("invalid_scroll_amount", "滚轮格数超过 Windows 输入范围")
        })?;
        if amount == 0 {
            return Ok(());
        }
        let event = match axis {
            ScrollAxis::Vertical => co::MOUSEEVENTF::WHEEL,
            ScrollAxis::Horizontal => co::MOUSEEVENTF::HWHEEL,
        };
        send_mouse(event, amount as u32)
    }

    pub fn drag(
        &mut self,
        button: MouseButton,
        from: PointerTarget,
        to: PointerTarget,
    ) -> Result<(), ToolFailure> {
        to.screen()?;
        self.move_mouse(from)?;
        self.with_button(button, |input| {
            input.wait(DRAG_EVENT_GAP)?;
            input.move_mouse(to)?;
            input.wait(DRAG_EVENT_GAP)
        })
    }

    pub fn drag_trajectory(
        &mut self,
        button: MouseButton,
        points: &[TrajectoryPoint],
    ) -> Result<(), ToolFailure> {
        let Some((first, rest)) = points.split_first() else {
            return Err(invalid_trajectory());
        };
        let window = first.target.window();
        if rest.is_empty()
            || !first.after.is_zero()
            || rest.iter().any(|point| point.target.window() != window)
        {
            return Err(invalid_trajectory());
        }
        self.move_mouse(first.target)?;
        self.with_button(button, |input| {
            input.wait(DRAG_EVENT_GAP)?;
            let start = Instant::now();
            let mut elapsed = Duration::ZERO;
            for point in rest {
                elapsed = elapsed.saturating_add(point.after);
                input.wait(elapsed.saturating_sub(start.elapsed()))?;
                input.move_mouse(point.target)?;
            }
            input.wait(DRAG_EVENT_GAP)
        })
    }

    pub fn release_all(&mut self) -> Result<(), ToolFailure> {
        let keys = self.release_keys(0);
        let buttons = self.release_buttons();
        after_cleanup(keys, buttons)
    }

    fn release_buttons(&mut self) -> Result<(), ToolFailure> {
        let mut errors = Vec::new();
        for index in (0..self.buttons.len()).rev() {
            let button = self.buttons[index];
            match send_mouse(button.event(false), 0) {
                Ok(()) => {
                    self.buttons.remove(index);
                }
                Err(error) => errors.push(format!("{button:?}: {error}")),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ToolFailure::new(
                "desktop_input_cleanup",
                errors.join(" | "),
            ))
        }
    }

    pub fn wait(&self, duration: Duration) -> Result<(), ToolFailure> {
        let start = Instant::now();
        loop {
            self.check_cancel()?;
            let remaining = duration.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Ok(());
            }
            thread::sleep(remaining.min(CANCELLATION_POLL));
        }
    }

    fn with_button(
        &mut self,
        button: MouseButton,
        action: impl FnOnce(&Self) -> Result<(), ToolFailure>,
    ) -> Result<(), ToolFailure> {
        if self.buttons.contains(&button) {
            return Err(ToolFailure::new(
                "mouse_button_already_held",
                format!("先释放已按下的鼠标按钮 {button:?}"),
            ));
        }
        self.mouse_down(button)?;
        let result = action(self);
        let release = self.mouse_up(button);
        after_cleanup(result, release)
    }

    fn check_cancel(&self) -> Result<(), ToolFailure> {
        if self.cancel.is_cancelled() {
            Err(ToolFailure::new("cancelled", "已停止后续桌面输入"))
        } else {
            Ok(())
        }
    }
}

impl Drop for DesktopInput {
    fn drop(&mut self) {
        if let Err(error) = self.release_all() {
            tracing::error!(%error, "释放 Windows 桌面输入失败");
        }
    }
}

pub fn cursor_position() -> Result<ScreenPoint, ToolFailure> {
    let point = winsafe::GetPhysicalCursorPos().map_err(input_failed)?;
    Ok(ScreenPoint {
        x: point.x,
        y: point.y,
    })
}

fn send_mouse(event: co::MOUSEEVENTF, amount: u32) -> Result<(), ToolFailure> {
    let input = MOUSEINPUT {
        mouseData: amount,
        dwFlags: event,
        ..MOUSEINPUT::default()
    };
    send_input(HwKbMouse::Mouse(input))
}

fn send_input(input: HwKbMouse) -> Result<(), ToolFailure> {
    match winsafe::SendInput(&[input]) {
        Ok(1) => Ok(()),
        Ok(count) => Err(input_failed(format!("Windows 接受了 {count} 个输入事件"))),
        Err(error) => Err(input_failed(error)),
    }
}

fn input_failed(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::new(
        "desktop_input_failed",
        format!("Windows 输入调用失败：{error}"),
    )
}

fn after_cleanup(
    result: Result<(), ToolFailure>,
    cleanup: Result<(), ToolFailure>,
) -> Result<(), ToolFailure> {
    match (result, cleanup) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(error), Err(cleanup)) => Err(ToolFailure::new(
            "desktop_input_cleanup",
            format!("{error}；输入清理失败：{cleanup}"),
        )),
    }
}

fn invalid_trajectory() -> ToolFailure {
    ToolFailure::new(
        "invalid_trajectory",
        "鼠标轨迹至少需要两个点，首点间隔为零，且所有点须在同一坐标空间",
    )
}
