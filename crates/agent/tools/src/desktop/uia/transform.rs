use super::elements::{Element, parse_reference};
use super::patterns::{get_pattern, invalid_pattern_argument, pattern_result, provider};
use super::{RequestControl, Runtime, Worker, provider_error};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    patterns::{UIDockPattern, UITransformPattern},
    types::{DockPosition, ZoomUnit},
};
use uuid::Uuid;
use windows::Win32::Foundation::E_NOINTERFACE;

pub(crate) enum TransformAction {
    Capabilities,
    Move(f64, f64),
    Resize(f64, f64),
    Rotate(f64),
    Zoom(f64),
    ZoomUnit(ZoomUnit),
    Dock(Option<DockPosition>),
}

impl TransformAction {
    pub(crate) fn new(action: &str, values: &[Value]) -> Result<Self, ToolFailure> {
        match (action, values) {
            ("move", [x, y]) => Ok(Self::Move(finite(x)?, finite(y)?)),
            ("resize", [width, height]) => {
                let (width, height) = (finite(width)?, finite(height)?);
                if width <= 0.0 || height <= 0.0 {
                    return Err(invalid_pattern_argument("窗口宽度和高度须为正数"));
                }
                Ok(Self::Resize(width, height))
            }
            ("rotate", [degrees]) => Ok(Self::Rotate(finite(degrees)?)),
            ("zoom", [level]) => Ok(Self::Zoom(finite(level)?)),
            ("zoom_by_unit", [Value::String(unit)]) => Ok(Self::ZoomUnit(zoom_unit(unit)?)),
            _ => Err(invalid_pattern_argument("未知的变换操作或参数数量不正确")),
        }
    }

    pub(crate) fn dock_position(position: &str) -> Result<Self, ToolFailure> {
        let position = match position {
            "top" => DockPosition::Top,
            "left" => DockPosition::Left,
            "bottom" => DockPosition::Bottom,
            "right" => DockPosition::Right,
            "fill" => DockPosition::Fill,
            "none" => DockPosition::None,
            _ => return Err(invalid_pattern_argument("未知的 UIA 停靠位置")),
        };
        Ok(Self::Dock(Some(position)))
    }
}

impl Runtime {
    pub(crate) fn transform(
        &self,
        reference: &str,
        action: TransformAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.transform(id, action, control)
        })
    }
}

impl Worker {
    fn transform(
        &mut self,
        id: Uuid,
        action: TransformAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        if let TransformAction::Dock(position) = action {
            let pattern: UIDockPattern = get_pattern(&entry.element, control)?;
            if let Some(position) = position {
                provider(control, || pattern.set_dock_position(position))?;
            }
            let actual = provider(control, || pattern.get_dock_position())?;
            return Ok(pattern_result(
                id,
                "Dock",
                if position.is_some() {
                    "set_position"
                } else {
                    "position"
                },
                json!({"position": format!("{actual:?}"), "bounds": bounds(&entry, control)?}),
            ));
        }
        let pattern: UITransformPattern = get_pattern(&entry.element, control)?;
        let (kind, operation, zoom) = match action {
            TransformAction::Move(x, y) => {
                require_capability(provider(control, || pattern.can_move())?, "Move")?;
                provider(control, || pattern.move_to(x, y))?;
                ("Transform", "move", Value::Null)
            }
            TransformAction::Resize(width, height) => {
                require_capability(provider(control, || pattern.can_resize())?, "Resize")?;
                provider(control, || pattern.resize(width, height))?;
                ("Transform", "resize", Value::Null)
            }
            TransformAction::Rotate(degrees) => {
                require_capability(provider(control, || pattern.can_rotate())?, "Rotate")?;
                provider(control, || pattern.rotate(degrees))?;
                ("Transform", "rotate", Value::Null)
            }
            TransformAction::Zoom(level) => {
                require_capability(can_zoom(&pattern, control)?.unwrap_or(false), "Zoom")?;
                let min = provider(control, || pattern.get_zoom_minimum())?;
                let max = provider(control, || pattern.get_zoom_maximum())?;
                if !(min..=max).contains(&level) {
                    return Err(invalid_pattern_argument("缩放比例超出 Provider 报告的范围"));
                }
                provider(control, || pattern.zoom(level))?;
                ("Transform2", "zoom", zoom_state(&pattern, control)?)
            }
            TransformAction::ZoomUnit(unit) => {
                require_capability(can_zoom(&pattern, control)?.unwrap_or(false), "ZoomByUnit")?;
                provider(control, || pattern.zoom_by_unit(unit))?;
                ("Transform2", "zoom_by_unit", zoom_state(&pattern, control)?)
            }
            TransformAction::Capabilities => {
                let zoom_supported = can_zoom(&pattern, control)?;
                let result = json!({
                    "can_move": provider(control, || pattern.can_move())?,
                    "can_resize": provider(control, || pattern.can_resize())?,
                    "can_rotate": provider(control, || pattern.can_rotate())?,
                    "transform2": zoom_supported.is_some(),
                    "can_zoom": zoom_supported.unwrap_or(false),
                    "zoom": if zoom_supported == Some(true) { zoom_state(&pattern, control)? } else { Value::Null },
                    "bounds": bounds(&entry, control)?,
                });
                return Ok(pattern_result(id, "Transform", "capabilities", result));
            }
            TransformAction::Dock(_) => return Err(invalid_pattern_argument("无法执行停靠读取")),
        };
        Ok(pattern_result(
            id,
            kind,
            operation,
            json!({"bounds": bounds(&entry, control)?, "zoom": zoom}),
        ))
    }
}

fn bounds(entry: &Element, control: &RequestControl) -> Result<Value, ToolFailure> {
    let rect = provider(control, || entry.element.get_bounding_rectangle())?;
    let left = rect.get_left();
    let top = rect.get_top();
    let right = rect.get_right();
    let bottom = rect.get_bottom();
    // The upstream Rect size helpers count both edges. UIA bounds use exclusive upper edges.
    Ok(
        json!({"left": left, "top": top, "right": right, "bottom": bottom,
        "width": i64::from(right) - i64::from(left), "height": i64::from(bottom) - i64::from(top)}),
    )
}

fn can_zoom(
    pattern: &UITransformPattern,
    control: &RequestControl,
) -> Result<Option<bool>, ToolFailure> {
    control.check()?;
    let supported = pattern.can_zoom();
    control.check()?;
    match supported {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.result() == Some(E_NOINTERFACE) => Ok(None),
        Err(error) => Err(provider_error(error)),
    }
}

fn zoom_state(
    pattern: &UITransformPattern,
    control: &RequestControl,
) -> Result<Value, ToolFailure> {
    Ok(json!({
        "level": provider(control, || pattern.get_zoom_level())?,
        "minimum": provider(control, || pattern.get_zoom_minimum())?,
        "maximum": provider(control, || pattern.get_zoom_maximum())?,
    }))
}

fn require_capability(allowed: bool, operation: &str) -> Result<(), ToolFailure> {
    if allowed {
        Ok(())
    } else {
        Err(ToolFailure::new(
            "uia_action_not_supported",
            format!("Provider 不支持 {operation}"),
        ))
    }
}

fn finite(value: &Value) -> Result<f64, ToolFailure> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid_pattern_argument("UIA 变换参数须为有限数字"))
}

fn zoom_unit(unit: &str) -> Result<ZoomUnit, ToolFailure> {
    match unit {
        "none" => Ok(ZoomUnit::NoAmount),
        "small_increment" => Ok(ZoomUnit::SmallIncrement),
        "small_decrement" => Ok(ZoomUnit::SmallDecrement),
        "large_increment" => Ok(ZoomUnit::LargeIncrement),
        "large_decrement" => Ok(ZoomUnit::LargeDecrement),
        _ => Err(invalid_pattern_argument("未知的缩放量")),
    }
}
