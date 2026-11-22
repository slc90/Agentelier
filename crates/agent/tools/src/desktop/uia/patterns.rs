use super::elements::{Element, parse_reference, reference_token, summarize_node};
use super::references::Locator;
use super::tree::{MAX_TREE_DEPTH, TreeView};
use super::{RequestControl, Runtime, Worker, provider_error};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::str::FromStr;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIElement,
    errors::Error as UiaError,
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UIPattern, UIRangeValuePattern,
        UISelectionItemPattern, UISelectionPattern, UITogglePattern, UIValuePattern,
    },
};
use uuid::Uuid;
use windows::{
    Win32::{Foundation::E_NOINTERFACE, UI::Accessibility::UIA_E_NOTSUPPORTED},
    core::IUnknown,
};

const MAX_PATTERN_ELEMENTS: usize = 64;
const MAX_RETURNED_VALUE_CHARS: usize = 4096;

pub(crate) enum BasicAction {
    Invoke,
    ValueGet,
    ValueSet(String),
    Toggle { change: bool },
    RangeGet,
    RangeSet(f64),
    Selection,
    Select(SelectionChange),
    ExpandCollapse(Expansion),
}

pub(crate) enum SelectionChange {
    Select,
    Add,
    Remove,
}

impl FromStr for SelectionChange {
    type Err = ToolFailure;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "select" => Ok(Self::Select),
            "add" => Ok(Self::Add),
            "remove" => Ok(Self::Remove),
            _ => Err(invalid_pattern_argument(
                "选择操作须为 select、add 或 remove",
            )),
        }
    }
}

pub(crate) enum Expansion {
    Read,
    Expand,
    Collapse,
}

impl FromStr for Expansion {
    type Err = ToolFailure;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "state" => Ok(Self::Read),
            "expand" => Ok(Self::Expand),
            "collapse" => Ok(Self::Collapse),
            _ => Err(invalid_pattern_argument(
                "展开操作须为 state、expand 或 collapse",
            )),
        }
    }
}

impl Runtime {
    pub(crate) fn basic_pattern(
        &self,
        reference: &str,
        action: BasicAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.basic_pattern(id, action, control)
        })
    }
}

impl Worker {
    fn basic_pattern(
        &mut self,
        id: Uuid,
        action: BasicAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let (pattern, operation, data) = match action {
            BasicAction::Invoke => {
                let pattern: UIInvokePattern = get_pattern(&entry.element, control)?;
                provider(control, || pattern.invoke())?;
                ("Invoke", "invoke", json!({"invoked": true}))
            }
            BasicAction::ValueGet => ("Value", "read", value(&entry, None, control)?),
            BasicAction::ValueSet(value_to_set) => {
                ("Value", "set", value(&entry, Some(&value_to_set), control)?)
            }
            BasicAction::Toggle { change } => {
                let pattern: UITogglePattern = get_pattern(&entry.element, control)?;
                if change {
                    provider(control, || pattern.toggle())?;
                }
                let state = provider(control, || pattern.get_toggle_state())?;
                (
                    "Toggle",
                    if change { "toggle" } else { "state" },
                    json!({"state": format!("{state:?}")}),
                )
            }
            BasicAction::RangeGet => ("RangeValue", "read", range(&entry, None, control)?),
            BasicAction::RangeSet(value) => {
                ("RangeValue", "set", range(&entry, Some(value), control)?)
            }
            BasicAction::Selection => {
                let pattern: UISelectionPattern = get_pattern(&entry.element, control)?;
                let multiple = provider(control, || pattern.can_select_multiple())?;
                let required = provider(control, || pattern.is_selection_required())?;
                let items = provider(control, || pattern.get_selection())?;
                let count = items.len();
                let results = self.register_pattern_elements(id, &entry, items, control)?;
                (
                    "Selection",
                    "read",
                    json!({"multiple": multiple, "required": required,
                    "count": count, "truncated": count > results.len(), "selected": results}),
                )
            }
            BasicAction::Select(change) => {
                let pattern: UISelectionItemPattern = get_pattern(&entry.element, control)?;
                let operation = match change {
                    SelectionChange::Select => {
                        provider(control, || pattern.select())?;
                        "select"
                    }
                    SelectionChange::Add => {
                        provider(control, || pattern.add_to_selection())?;
                        "add"
                    }
                    SelectionChange::Remove => {
                        provider(control, || pattern.remove_from_selection())?;
                        "remove"
                    }
                };
                let selected = provider(control, || pattern.is_selected())?;
                ("SelectionItem", operation, json!({"selected": selected}))
            }
            BasicAction::ExpandCollapse(action) => {
                let pattern: UIExpandCollapsePattern = get_pattern(&entry.element, control)?;
                let operation = match action {
                    Expansion::Read => "state",
                    Expansion::Expand => {
                        provider(control, || pattern.expand())?;
                        "expand"
                    }
                    Expansion::Collapse => {
                        provider(control, || pattern.collapse())?;
                        "collapse"
                    }
                };
                let state = provider(control, || pattern.get_state())?;
                (
                    "ExpandCollapse",
                    operation,
                    json!({"state": format!("{state:?}")}),
                )
            }
        };
        Ok(pattern_result(id, pattern, operation, data))
    }

    pub(super) fn register_pattern_elements(
        &mut self,
        anchor: Uuid,
        owner: &Element,
        elements: Vec<UIElement>,
        control: &RequestControl,
    ) -> Result<Vec<Value>, ToolFailure> {
        let count = elements.len().min(MAX_PATTERN_ELEMENTS);
        self.elements.reserve(count)?;
        let mut entries = Vec::with_capacity(count);
        let mut summaries = Vec::with_capacity(count);
        for element in elements.into_iter().take(count) {
            let id = Uuid::new_v4();
            let entry = Element::capture(
                &self.automation,
                element,
                owner.window,
                false,
                Some(Locator::new(
                    anchor,
                    TreeView::Control,
                    MAX_TREE_DEPTH as usize,
                )),
                control,
            )?;
            let summary = summarize_node(id, &entry, control)?;
            summaries.push(json!({"ref": summary.reference, "name": summary.name,
                "class_name": summary.class_name, "control_type": summary.control_type,
                "window_id": summary.window_id}));
            entries.push((id, entry));
        }
        control.check()?;
        for (id, entry) in entries {
            self.elements.insert(id, entry)?;
        }
        Ok(summaries)
    }
}

fn value(
    entry: &Element,
    set: Option<&str>,
    control: &RequestControl,
) -> Result<Value, ToolFailure> {
    let pattern: UIValuePattern = get_pattern(&entry.element, control)?;
    let read_only = provider(control, || pattern.is_readonly())?;
    if let Some(value) = set {
        if read_only {
            return Err(read_only_pattern());
        }
        provider(control, || pattern.set_value(value))?;
    }
    let value = provider(control, || pattern.get_value())?;
    let mut characters = value.chars();
    let value: String = characters.by_ref().take(MAX_RETURNED_VALUE_CHARS).collect();
    Ok(json!({"value": value, "read_only": read_only, "truncated": characters.next().is_some()}))
}

fn range(
    entry: &Element,
    set: Option<f64>,
    control: &RequestControl,
) -> Result<Value, ToolFailure> {
    let pattern: UIRangeValuePattern = get_pattern(&entry.element, control)?;
    let read_only = provider(control, || pattern.is_readonly())?;
    let minimum = provider(control, || pattern.get_minimum())?;
    let maximum = provider(control, || pattern.get_maximum())?;
    if let Some(value) = set {
        if read_only {
            return Err(read_only_pattern());
        }
        if !value.is_finite() || !(minimum..=maximum).contains(&value) {
            return Err(invalid_pattern_argument(
                "RangeValue 超出 Provider 报告的范围",
            ));
        }
        provider(control, || pattern.set_value(value))?;
    }
    let value = provider(control, || pattern.get_value())?;
    let large_change = provider(control, || pattern.get_large_change())?;
    let small_change = provider(control, || pattern.get_small_change())?;
    Ok(
        json!({"value": value, "minimum": minimum, "maximum": maximum,
        "large_change": large_change, "small_change": small_change, "read_only": read_only}),
    )
}

pub(super) fn get_pattern<T>(
    element: &UIElement,
    control: &RequestControl,
) -> Result<T, ToolFailure>
where
    T: UIPattern + TryFrom<IUnknown, Error = UiaError>,
{
    control.check()?;
    element.get_pattern::<T>().map_err(|error| {
        if error.code() == 0
            || error.code().cast_unsigned() == UIA_E_NOTSUPPORTED
            || error.result() == Some(E_NOINTERFACE)
        {
            ToolFailure::new(
                "uia_pattern_not_supported",
                format!("目标元素不支持 {:?} Pattern", T::TYPE),
            )
        } else {
            provider_error(error)
        }
    })
}

pub(super) fn provider<T>(
    control: &RequestControl,
    operation: impl FnOnce() -> Result<T, UiaError>,
) -> Result<T, ToolFailure> {
    control.check()?;
    let result = operation().map_err(provider_error);
    control.check()?;
    result
}

pub(super) fn optional_provider<T>(
    control: &RequestControl,
    operation: impl FnOnce() -> Result<T, UiaError>,
) -> Result<Option<T>, ToolFailure> {
    control.check()?;
    let result = operation();
    control.check()?;
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == 0 => Ok(None),
        Err(error) => Err(provider_error(error)),
    }
}

pub(super) fn pattern_result(id: Uuid, pattern: &str, operation: &str, data: Value) -> Value {
    json!({"ref": reference_token(id), "pattern": pattern, "operation": operation,
        "result": data})
}

pub(super) fn invalid_pattern_argument(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}

fn read_only_pattern() -> ToolFailure {
    ToolFailure::new("uia_read_only", "目标 Pattern 为只读")
}
