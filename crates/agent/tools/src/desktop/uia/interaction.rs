use super::elements::parse_reference;
use super::patterns::{
    get_pattern, invalid_pattern_argument, optional_provider, pattern_result, provider,
};
use super::{RequestControl, Runtime, Worker, provider_error};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    patterns::{
        UICustomNavigationPattern, UIDragPattern, UIDropTargetPattern, UISynchronizedInputPattern,
    },
    types::NavigateDirection,
};
use uuid::Uuid;
use windows::Win32::UI::Accessibility::{
    SynchronizedInputType, SynchronizedInputType_KeyDown, SynchronizedInputType_KeyUp,
    SynchronizedInputType_LeftMouseDown, SynchronizedInputType_LeftMouseUp,
    SynchronizedInputType_RightMouseDown, SynchronizedInputType_RightMouseUp,
};

const MAX_DROP_EFFECTS: usize = 32;
const MAX_EFFECT_CHARS: usize = 256;

pub(crate) enum InteractionAction {
    Drag,
    DropTarget,
    Navigate(NavigateDirection),
    Listen {
        input: SynchronizedInputType,
        name: &'static str,
    },
    CancelInput,
    InputStatus,
}

impl InteractionAction {
    pub(crate) fn navigate(direction: &str) -> Result<Self, ToolFailure> {
        let direction = match direction {
            "parent" => NavigateDirection::Parent,
            "next_sibling" => NavigateDirection::NextSibling,
            "previous_sibling" => NavigateDirection::PreviousSibling,
            "first_child" => NavigateDirection::FirstChild,
            "last_child" => NavigateDirection::LastChild,
            _ => return Err(invalid_pattern_argument("未知的自定义导航方向")),
        };
        Ok(Self::Navigate(direction))
    }

    pub(crate) fn synchronized(action: &str, input: Option<&str>) -> Result<Self, ToolFailure> {
        match (action, input) {
            ("cancel", None) => Ok(Self::CancelInput),
            ("status", None) => Ok(Self::InputStatus),
            ("start", Some(input)) => {
                let (input, name) = match input {
                    "key_down" => (SynchronizedInputType_KeyDown, "key_down"),
                    "key_up" => (SynchronizedInputType_KeyUp, "key_up"),
                    "left_mouse_down" => (SynchronizedInputType_LeftMouseDown, "left_mouse_down"),
                    "left_mouse_up" => (SynchronizedInputType_LeftMouseUp, "left_mouse_up"),
                    "right_mouse_down" => {
                        (SynchronizedInputType_RightMouseDown, "right_mouse_down")
                    }
                    "right_mouse_up" => (SynchronizedInputType_RightMouseUp, "right_mouse_up"),
                    _ => return Err(invalid_pattern_argument("未知的同步输入类型")),
                };
                Ok(Self::Listen { input, name })
            }
            _ => Err(invalid_pattern_argument(
                "同步输入操作须为 start 加输入类型、cancel 或 status",
            )),
        }
    }
}

#[derive(Default)]
pub(super) struct SynchronizedInput {
    active: Option<Listener>,
}

struct Listener {
    id: Uuid,
    pattern: UISynchronizedInputPattern,
    name: &'static str,
}

impl SynchronizedInput {
    pub(super) fn close(&mut self) -> Result<(), ToolFailure> {
        if let Some(active) = self.active.as_ref() {
            active.pattern.cancel().map_err(provider_error)?;
            self.active = None;
        }
        Ok(())
    }

    fn status(&self, id: Uuid) -> Value {
        let request = self.active.as_ref().filter(|active| active.id == id);
        json!({"request_owned": request.is_some(), "input_type": request.map(|request| request.name)})
    }
}

impl Runtime {
    pub(crate) fn interact(
        &self,
        reference: &str,
        action: InteractionAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.interact(id, action, control)
        })
    }
}

impl Worker {
    fn interact(
        &mut self,
        id: Uuid,
        action: InteractionAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        if matches!(action, InteractionAction::CancelInput) {
            if !self
                .synchronized_input
                .active
                .as_ref()
                .is_some_and(|listener| listener.id == id)
            {
                let entry = self.resolve_element(id, control)?;
                let _: UISynchronizedInputPattern = get_pattern(&entry.element, control)?;
                return Err(ToolFailure::new(
                    "uia_input_not_listening",
                    "当前引用没有会话持有的同步输入请求",
                ));
            }
            control.check()?;
            self.synchronized_input.close()?;
            return Ok(pattern_result(
                id,
                "SynchronizedInput",
                "cancel",
                self.synchronized_input.status(id),
            ));
        }
        let entry = self.resolve_element(id, control)?;
        let (pattern, operation, data) = match action {
            InteractionAction::Drag => {
                let pattern: UIDragPattern = get_pattern(&entry.element, control)?;
                let grabbed = provider(control, || pattern.is_grabbed())?;
                let effect = provider(control, || pattern.get_drop_effect())?;
                let effects =
                    optional_provider(control, || pattern.get_drop_effects())?.unwrap_or_default();
                let items =
                    optional_provider(control, || pattern.get_grabbed_items())?.unwrap_or_default();
                let count = items.len();
                let items = self.register_pattern_elements(id, &entry, items, control)?;
                (
                    "Drag",
                    "info",
                    json!({"grabbed":grabbed, "effect":bounded_effect(effect),
                    "effects":bounded_effects(effects), "items":items, "item_count":count,
                    "items_truncated":count > items.len()}),
                )
            }
            InteractionAction::DropTarget => {
                let pattern: UIDropTargetPattern = get_pattern(&entry.element, control)?;
                let effect = provider(control, || pattern.get_drop_target_effect())?;
                let effects = optional_provider(control, || pattern.get_drop_target_effects())?
                    .unwrap_or_default();
                (
                    "DropTarget",
                    "info",
                    json!({"effect":bounded_effect(effect), "effects":bounded_effects(effects)}),
                )
            }
            InteractionAction::Navigate(direction) => {
                let pattern: UICustomNavigationPattern = get_pattern(&entry.element, control)?;
                let related = optional_provider(control, || pattern.navigate(direction))?;
                let element = if let Some(element) = related {
                    self.register_pattern_elements(id, &entry, vec![element], control)?
                        .pop()
                } else {
                    None
                };
                (
                    "CustomNavigation",
                    "navigate",
                    json!({"direction":format!("{direction:?}"), "element":element}),
                )
            }
            InteractionAction::Listen { input, name } => {
                if self.synchronized_input.active.is_some() {
                    return Err(ToolFailure::new(
                        "uia_input_already_listening",
                        "请先取消当前会话持有的同步输入请求",
                    ));
                }
                let pattern: UISynchronizedInputPattern = get_pattern(&entry.element, control)?;
                control.check()?;
                pattern.start_listening(input).map_err(provider_error)?;
                self.synchronized_input.active = Some(Listener { id, pattern, name });
                if let Err(error) = control.check() {
                    self.synchronized_input.close()?;
                    return Err(error);
                }
                (
                    "SynchronizedInput",
                    "start",
                    self.synchronized_input.status(id),
                )
            }
            InteractionAction::InputStatus => {
                let _: UISynchronizedInputPattern = get_pattern(&entry.element, control)?;
                (
                    "SynchronizedInput",
                    "status",
                    self.synchronized_input.status(id),
                )
            }
            InteractionAction::CancelInput => {
                return Err(invalid_pattern_argument("同步输入取消操作未执行"));
            }
        };
        Ok(pattern_result(id, pattern, operation, data))
    }
}

fn bounded_effect(effect: String) -> String {
    effect.chars().take(MAX_EFFECT_CHARS).collect()
}

fn bounded_effects(effects: Vec<String>) -> Value {
    let truncated = effects.len() > MAX_DROP_EFFECTS;
    let values: Vec<_> = effects
        .into_iter()
        .take(MAX_DROP_EFFECTS)
        .map(bounded_effect)
        .collect();
    json!({"values":values, "truncated":truncated})
}
