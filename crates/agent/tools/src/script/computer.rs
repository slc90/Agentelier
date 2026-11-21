use super::Control;
use crate::desktop::{
    self, Capture, CaptureMapping, DesktopInput, Key, MouseButton, PointerTarget, ScreenPoint,
    ScreenRect, ScreenshotPixel, ScrollAxis, TrajectoryPoint, WindowId, WindowInfo, WindowPoint,
};
use crate::{ImagePayload, ToolFailure};
use rhai::{Array, Dynamic, Engine, EvalAltResult, INT, ImmutableString, Map, NativeCallContext};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

pub(super) struct Run {
    control: Control,
    input: DesktopInput,
    mapping: Option<CaptureMapping>,
    image: Option<Arc<ImagePayload>>,
}

impl Run {
    pub(super) fn new(control: Control, mapping: Option<CaptureMapping>) -> Self {
        Self {
            input: DesktopInput::new(control.cancel.clone()),
            control,
            mapping,
            image: None,
        }
    }

    pub(super) fn finish(
        &mut self,
    ) -> Result<(Option<CaptureMapping>, Option<Arc<ImagePayload>>), ToolFailure> {
        self.input.release_all()?;
        Ok((self.mapping.take(), self.image.take()))
    }

    fn scan(&mut self, capture: Capture) -> Map {
        let (image, mapping) = capture.into_parts();
        let (width, height) = image.dimensions();
        let mut info = fields([
            ("width", INT::from(width).into()),
            ("height", INT::from(height).into()),
            ("screen", rect_fields(mapping.screen_bounds()).into()),
        ]);
        if let Some(bounds) = mapping.window_bounds() {
            info.insert("window".into(), rect_fields(bounds).into());
        }
        self.mapping = Some(mapping);
        self.image = Some(Arc::new(image));
        info
    }

    fn last_capture(&self) -> Result<&CaptureMapping, ToolFailure> {
        self.mapping
            .as_ref()
            .ok_or_else(|| ToolFailure::new("capture_required", "请先获取一张截图"))
    }

    fn check_duration(&self, duration: Duration) -> Result<(), ToolFailure> {
        if duration
            > self
                .control
                .deadline
                .saturating_duration_since(Instant::now())
        {
            Err(ToolFailure::new(
                "timeout",
                "请求的操作时长超过脚本剩余时间",
            ))
        } else {
            Ok(())
        }
    }
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<ToolFailure>("ComputerError")
        .register_get("code", |error: &mut ToolFailure| error.code.to_owned())
        .register_get("message", |error: &mut ToolFailure| error.to_string())
        .register_fn("windows", list_windows)
        .register_fn("window", window_info)
        .register_fn("activate", activate)
        .register_fn("capture_desktop", capture_desktop)
        .register_fn("capture_window", capture_window)
        .register_fn("screen_at", screen_at)
        .register_fn("window_at", window_at)
        .register_fn("mouse_position", mouse_position)
        .register_fn("mouse_move", mouse_move)
        .register_fn("mouse_move", mouse_move_in_window)
        .register_fn("mouse_down", mouse_down)
        .register_fn("mouse_up", mouse_up)
        .register_fn("click", click)
        .register_fn("double_click", double_click)
        .register_fn("scroll", scroll)
        .register_fn("drag", drag)
        .register_fn("drag", drag_window)
        .register_fn("trajectory", trajectory)
        .register_fn("trajectory", trajectory_window)
        .register_fn("key_down", key_down)
        .register_fn("key_up", key_up)
        .register_fn("press", press)
        .register_fn("shortcut", shortcut)
        .register_fn("type_text", type_text)
        .register_fn("wait", wait);
}

fn with_run<T>(
    ctx: NativeCallContext<'_>,
    act: impl FnOnce(&mut Run) -> Result<T, ToolFailure>,
) -> ScriptResult<T> {
    let result = (|| {
        let state = ctx.tag().and_then(|tag| tag.read_lock::<Arc<Mutex<Run>>>());
        let state = state.ok_or_else(|| {
            ToolFailure::new("desktop_context_missing", "当前脚本没有桌面执行上下文")
        })?;
        let mut run = state
            .lock()
            .map_err(|error| ToolFailure::new("desktop_context_failed", error.to_string()))?;
        run.control.check()?;
        act(&mut run)
    })();
    result.map_err(|error| {
        EvalAltResult::ErrorRuntime(Dynamic::from(error), ctx.call_position()).into()
    })
}

pub(super) fn script_failure(error: &EvalAltResult) -> ToolFailure {
    if let EvalAltResult::ErrorRuntime(value, _) = error.unwrap_inner()
        && let Some(failure) = value.read_lock::<ToolFailure>()
    {
        return (*failure).clone();
    }
    ToolFailure::new("script_evaluation_failed", error.to_string())
}

fn list_windows(ctx: NativeCallContext<'_>) -> ScriptResult<Array> {
    with_run(ctx, |_| {
        Ok(desktop::windows()?
            .into_iter()
            .map(|info| window_fields(info).into())
            .collect())
    })
}

fn window_info(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |_| desktop::window(window_id(&id)?).map(window_fields))
}

fn activate(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |_| {
        desktop::activate_window(window_id(&id)?).map(window_fields)
    })
}

fn capture_desktop(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_run(ctx, |run| Ok(run.scan(desktop::capture_desktop()?)))
}

fn capture_window(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        Ok(run.scan(desktop::capture_window(window_id(&id)?)?))
    })
}

fn screen_at(ctx: NativeCallContext<'_>, x: INT, y: INT) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        let (x, y) = local_coords(x, y)?;
        run.last_capture()?
            .screen_point(ScreenshotPixel { x, y })
            .map(screen_fields)
    })
}

fn window_at(ctx: NativeCallContext<'_>, x: INT, y: INT) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        let (x, y) = local_coords(x, y)?;
        let point = run.last_capture()?.window_point(ScreenshotPixel { x, y })?;
        Ok(fields([
            ("x", INT::from(point.x).into()),
            ("y", INT::from(point.y).into()),
        ]))
    })
}

fn mouse_position(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_run(ctx, |_| desktop::cursor_position().map(screen_fields))
}

fn mouse_move(ctx: NativeCallContext<'_>, x: INT, y: INT) -> ScriptResult<Map> {
    move_in(ctx, None, x, y)
}

fn mouse_move_in_window(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    x: INT,
    y: INT,
) -> ScriptResult<Map> {
    move_in(ctx, Some(&id), x, y)
}

fn move_in(ctx: NativeCallContext<'_>, window: Option<&str>, x: INT, y: INT) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.input
            .move_mouse(target(window.map(window_id).transpose()?, x, y)?)
            .map(screen_fields)
    })
}

fn mouse_down(ctx: NativeCallContext<'_>, button: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.mouse_down(mouse_button(&button)?))
}

fn mouse_up(ctx: NativeCallContext<'_>, button: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.mouse_up(mouse_button(&button)?))
}

fn click(ctx: NativeCallContext<'_>, button: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.click(mouse_button(&button)?))
}

fn double_click(ctx: NativeCallContext<'_>, button: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.double_click(mouse_button(&button)?))
}

fn scroll(ctx: NativeCallContext<'_>, axis: ImmutableString, detents: INT) -> ScriptResult<()> {
    with_run(ctx, |run| {
        let axis = match axis.as_str() {
            "vertical" => ScrollAxis::Vertical,
            "horizontal" => ScrollAxis::Horizontal,
            _ => return invalid("滚轮方向须为 vertical 或 horizontal"),
        };
        let detents = i32::try_from(detents).map_err(|_| bad_argument("滚轮格数越界"))?;
        run.input.scroll(axis, detents)
    })
}

fn drag(
    ctx: NativeCallContext<'_>,
    button: ImmutableString,
    from: Array,
    to: Array,
) -> ScriptResult<()> {
    drag_in(ctx, None, button, from, to)
}

fn drag_window(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    button: ImmutableString,
    from: Array,
    to: Array,
) -> ScriptResult<()> {
    drag_in(ctx, Some(&id), button, from, to)
}

fn drag_in(
    ctx: NativeCallContext<'_>,
    window: Option<&str>,
    button: ImmutableString,
    from: Array,
    to: Array,
) -> ScriptResult<()> {
    with_run(ctx, |run| {
        let id = window.map(window_id).transpose()?;
        let parse = |point: Array| {
            let [x, y] = point.as_slice() else {
                return invalid("拖动坐标须为 [x, y]");
            };
            target(id, number(x)?, number(y)?)
        };
        run.input
            .drag(mouse_button(&button)?, parse(from)?, parse(to)?)
    })
}

fn trajectory(
    ctx: NativeCallContext<'_>,
    button: ImmutableString,
    points: Array,
) -> ScriptResult<()> {
    trajectory_in(ctx, None, button, points)
}

fn trajectory_window(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    button: ImmutableString,
    points: Array,
) -> ScriptResult<()> {
    trajectory_in(ctx, Some(&id), button, points)
}

fn trajectory_in(
    ctx: NativeCallContext<'_>,
    window: Option<&str>,
    button: ImmutableString,
    points: Array,
) -> ScriptResult<()> {
    with_run(ctx, |run| {
        let id = window.map(window_id).transpose()?;
        let points = points
            .into_iter()
            .map(|point| {
                let tuple = point
                    .try_cast::<Array>()
                    .ok_or_else(|| bad_argument("轨迹点须为 [x, y, dt]"))?;
                let [x, y, delay] = tuple.as_slice() else {
                    return invalid("轨迹点须为 [x, y, dt]");
                };
                Ok(TrajectoryPoint {
                    target: target(id, number(x)?, number(y)?)?,
                    after: duration(number(delay)?)?,
                })
            })
            .collect::<Result<Vec<_>, ToolFailure>>()?;
        let total = points
            .iter()
            .fold(Duration::ZERO, |total, pt| total.saturating_add(pt.after));
        run.check_duration(total)?;
        run.input.drag_trajectory(mouse_button(&button)?, &points)
    })
}

fn key_down(ctx: NativeCallContext<'_>, key: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.key_down(Key::from_name(&key)?))
}

fn key_up(ctx: NativeCallContext<'_>, key: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.key_up(Key::from_name(&key)?))
}

fn press(ctx: NativeCallContext<'_>, key: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.press(Key::from_name(&key)?))
}

fn shortcut(ctx: NativeCallContext<'_>, keys: Array) -> ScriptResult<()> {
    with_run(ctx, |run| {
        let keys = keys
            .into_iter()
            .map(|key| {
                let key = key
                    .try_cast::<ImmutableString>()
                    .ok_or_else(|| bad_argument("组合键使用键名字符串"))?;
                Key::from_name(&key)
            })
            .collect::<Result<Vec<_>, ToolFailure>>()?;
        run.input.shortcut(&keys)
    })
}

fn type_text(ctx: NativeCallContext<'_>, text: ImmutableString) -> ScriptResult<()> {
    with_run(ctx, |run| run.input.type_text(&text))
}

fn wait(ctx: NativeCallContext<'_>, millis: INT) -> ScriptResult<()> {
    with_run(ctx, |run| {
        let duration = duration(millis)?;
        run.check_duration(duration)?;
        run.input.wait(duration)
    })
}

fn fields<const N: usize>(values: [(&str, Dynamic); N]) -> Map {
    values
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
}

fn window_fields(info: WindowInfo) -> Map {
    fields([
        ("id", info.id.token().into()),
        ("handle", format!("0x{:x}", info.id.handle()).into()),
        ("title", info.title.into()),
        ("bounds", rect_fields(info.bounds).into()),
        ("minimized", info.minimized.into()),
        ("foreground", info.foreground.into()),
    ])
}

fn rect_fields(rect: ScreenRect) -> Map {
    let (width, height) = rect.dimensions();
    fields([
        ("left", INT::from(rect.left).into()),
        ("top", INT::from(rect.top).into()),
        ("right", INT::from(rect.right).into()),
        ("bottom", INT::from(rect.bottom).into()),
        ("width", INT::from(width).into()),
        ("height", INT::from(height).into()),
    ])
}

fn screen_fields(point: ScreenPoint) -> Map {
    fields([
        ("x", INT::from(point.x).into()),
        ("y", INT::from(point.y).into()),
    ])
}

fn window_id(id: &str) -> Result<WindowId, ToolFailure> {
    WindowId::from_token(id).ok_or_else(|| bad_argument("需要 windows() 返回的窗口 id"))
}

fn mouse_button(button: &str) -> Result<MouseButton, ToolFailure> {
    match button {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        _ => invalid("鼠标按钮须为 left、right 或 middle"),
    }
}

fn target(window: Option<WindowId>, x: INT, y: INT) -> Result<PointerTarget, ToolFailure> {
    if let Some(id) = window {
        let (x, y) = local_coords(x, y)?;
        Ok(PointerTarget::Window(id, WindowPoint { x, y }))
    } else {
        let x = i32::try_from(x).map_err(|_| bad_argument("屏幕坐标 x 超出范围"))?;
        let y = i32::try_from(y).map_err(|_| bad_argument("屏幕坐标 y 超出范围"))?;
        Ok(PointerTarget::Screen(ScreenPoint { x, y }))
    }
}

fn local_coords(x: INT, y: INT) -> Result<(u32, u32), ToolFailure> {
    let x = u32::try_from(x).map_err(|_| bad_argument("局部坐标 x 超出范围"))?;
    let y = u32::try_from(y).map_err(|_| bad_argument("局部坐标 y 超出范围"))?;
    Ok((x, y))
}

fn number(value: &Dynamic) -> Result<INT, ToolFailure> {
    value
        .as_int()
        .map_err(|_| bad_argument("坐标和时间参数须为整数"))
}

fn duration(millis: INT) -> Result<Duration, ToolFailure> {
    u64::try_from(millis)
        .map(Duration::from_millis)
        .map_err(|_| bad_argument("等待时间须为非负整数"))
}

fn invalid<T>(message: &str) -> Result<T, ToolFailure> {
    Err(bad_argument(message))
}

fn bad_argument(message: &str) -> ToolFailure {
    ToolFailure::new("invalid_computer_argument", message)
}
