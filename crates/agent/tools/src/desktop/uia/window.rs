use super::elements::{Element, parse_reference};
use super::patterns::{get_pattern, invalid_pattern_argument, pattern_result, provider};
use super::{RequestControl, Runtime, Worker};
use crate::{
    ToolFailure,
    desktop::{self, WindowId},
};
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{patterns::UIWindowPattern, types::WindowVisualState};
use uuid::Uuid;

const DEFAULT_IDLE_WAIT_MS: i32 = 500;
const MAX_IDLE_WAIT_MS: i64 = 5000;

pub(crate) enum WindowAction {
    State,
    Visual(WindowVisualState),
    Close,
    WaitIdle(i32),
}

impl WindowAction {
    pub(crate) fn from_name(name: &str) -> Result<Self, ToolFailure> {
        match name {
            "restore" => Ok(Self::Visual(WindowVisualState::Normal)),
            "maximize" => Ok(Self::Visual(WindowVisualState::Maximized)),
            "minimize" => Ok(Self::Visual(WindowVisualState::Minimized)),
            "close" => Ok(Self::Close),
            "wait_for_input_idle" => Ok(Self::WaitIdle(DEFAULT_IDLE_WAIT_MS)),
            _ => Err(invalid_pattern_argument("未知的 UIA Window 操作")),
        }
    }

    pub(crate) fn wait_idle(timeout_ms: i64) -> Result<Self, ToolFailure> {
        if !(0..=MAX_IDLE_WAIT_MS).contains(&timeout_ms) {
            return Err(invalid_pattern_argument(format!(
                "窗口空闲等待须在 0..={MAX_IDLE_WAIT_MS} 毫秒"
            )));
        }
        Ok(Self::WaitIdle(timeout_ms as i32))
    }
}

impl Runtime {
    pub(crate) fn window_pattern(
        &self,
        reference: &str,
        action: WindowAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.window_pattern(id, action, control)
        })
    }
}

impl Worker {
    fn window_pattern(
        &mut self,
        id: Uuid,
        action: WindowAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let pattern: UIWindowPattern = get_pattern(&entry.element, control)?;
        let top_level = top_level_window(&entry, control)?;
        let (operation, result) = match action {
            WindowAction::State => ("state", read_state(&pattern, top_level, control)?),
            WindowAction::Visual(target) => {
                let supported = match target {
                    WindowVisualState::Maximized => provider(control, || pattern.can_maximize())?,
                    WindowVisualState::Minimized => provider(control, || pattern.can_minimize())?,
                    WindowVisualState::Normal => true,
                };
                if !supported {
                    return Err(ToolFailure::new(
                        "uia_action_not_supported",
                        "Provider 不支持所请求的窗口状态",
                    ));
                }
                provider(control, || pattern.set_window_visual_state(target))?;
                let mut state = read_state(&pattern, top_level, control)?;
                let target_name = format!("{target:?}");
                state["requested_visual_state"] = json!(target_name);
                state["reached_requested_state"] =
                    json!(state["visual_state"] == state["requested_visual_state"]);
                ("set_visual_state", state)
            }
            WindowAction::Close => {
                provider(control, || pattern.close())?;
                let native_window_closed = match top_level {
                    Some(window) => desktop::id_for_handle(window.handle())? != Some(window),
                    None => false,
                };
                let state = if native_window_closed {
                    None
                } else {
                    match read_state(&pattern, top_level, control) {
                        Ok(state) => Some(state),
                        Err(error) if error.code == "uia_element_unavailable" => None,
                        Err(error) => return Err(error),
                    }
                };
                (
                    "close",
                    json!({"requested": true, "window_closed": native_window_closed,
                    "element_available": state.is_some(), "state": state}),
                )
            }
            WindowAction::WaitIdle(timeout) => {
                let remaining = control
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis();
                let timeout = remaining.min(timeout as u128) as i32;
                let idle = provider(control, || pattern.wait_for_input_idle(timeout))?;
                let mut state = read_state(&pattern, top_level, control)?;
                state["input_idle"] = json!(idle);
                state["wait_ms"] = json!(timeout);
                ("wait_for_input_idle", state)
            }
        };
        Ok(pattern_result(id, "Window", operation, result))
    }
}

fn top_level_window(
    entry: &Element,
    control: &RequestControl,
) -> Result<Option<WindowId>, ToolFailure> {
    let handle = provider(control, || entry.element.get_native_window_handle())?;
    let handle = isize::from(handle).cast_unsigned();
    if handle == 0 {
        Ok(None)
    } else {
        desktop::id_for_handle(handle)
    }
}

fn read_state(
    pattern: &UIWindowPattern,
    top_level: Option<WindowId>,
    control: &RequestControl,
) -> Result<Value, ToolFailure> {
    let visual_state = provider(control, || pattern.get_window_visual_state())?;
    let interaction_state = provider(control, || pattern.get_window_interaction_state())?;
    let can_minimize = provider(control, || pattern.can_minimize())?;
    let can_maximize = provider(control, || pattern.can_maximize())?;
    let modal = provider(control, || pattern.is_modal())?;
    let topmost = provider(control, || pattern.is_topmost())?;
    Ok(json!({"visual_state": format!("{visual_state:?}"),
        "interaction_state": format!("{interaction_state:?}"), "can_minimize": can_minimize,
        "can_maximize": can_maximize, "modal": modal, "topmost": topmost,
        "top_level": top_level.is_some()}))
}
