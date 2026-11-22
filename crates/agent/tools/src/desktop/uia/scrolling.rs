use super::elements::{Element, parse_reference, reference_token, summarize_node, validate_window};
use super::patterns::{get_pattern, invalid_pattern_argument, pattern_result, provider};
use super::references::Locator;
use super::tree::{MAX_TREE_DEPTH, TreeView, neighbor};
use super::{RequestControl, Runtime, Worker, native};
use crate::{ToolFailure, desktop::WindowId};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIElement,
    patterns::{
        UIItemContainerPattern, UIScrollItemPattern, UIScrollPattern, UIVirtualizedItemPattern,
    },
    types::{ScrollAmount, UIProperty},
    variants::Variant,
};
use uuid::Uuid;

const MAX_VIRTUAL_ITEMS: usize = 128;
const MAX_ITEM_LOOKUP_CHARS: usize = 512;

#[derive(Clone, Copy)]
pub(crate) struct ScrollAmounts {
    horizontal: ScrollAmount,
    vertical: ScrollAmount,
}

impl ScrollAmounts {
    pub(crate) fn new(horizontal: &str, vertical: &str) -> Result<Self, ToolFailure> {
        Ok(Self {
            horizontal: scroll_amount(horizontal)?,
            vertical: scroll_amount(vertical)?,
        })
    }
}

#[derive(Clone)]
pub(crate) struct ItemLookup {
    property: UIProperty,
    value: String,
}

impl ItemLookup {
    pub(crate) fn new(property: &str, value: String) -> Result<Self, ToolFailure> {
        let property = match property.to_ascii_lowercase().as_str() {
            "name" => UIProperty::Name,
            "automationid" => UIProperty::AutomationId,
            "classname" => UIProperty::ClassName,
            _ => {
                return Err(invalid_pattern_argument(
                    "容器查找属性须为 Name、AutomationId 或 ClassName",
                ));
            }
        };
        if value.chars().nth(MAX_ITEM_LOOKUP_CHARS).is_some() {
            return Err(invalid_pattern_argument("容器查找文本超过长度上限"));
        }
        Ok(Self { property, value })
    }
}

#[derive(Default)]
pub(super) struct VirtualItems {
    items: HashMap<Uuid, VirtualItem>,
}

#[derive(Clone)]
struct VirtualItem {
    element: UIElement,
    container: Uuid,
    window: Option<WindowId>,
    lookup: ItemLookup,
}

impl VirtualItems {
    pub(super) fn info(&self, id: Uuid) -> Option<Result<Value, ToolFailure>> {
        let item = self.items.get(&id)?;
        let window_available = item.window.map(validate_window).transpose();
        Some(window_available.map(|_| json!({
            "ref": reference_token(id), "status": "virtualized", "generation": 1,
            "window_available": true, "window_id": item.window.map(WindowId::token),
        })).or_else(|error| {
            if error.code == "uia_window_unavailable" {
                Ok(json!({
                    "ref": reference_token(id), "status": "unavailable", "generation": 1,
                    "window_available": false, "window_id": item.window.map(WindowId::token),
                }))
            } else { Err(error) }
        }))
    }
}

impl Runtime {
    pub(crate) fn scroll(
        &self,
        reference: &str,
        amounts: Option<ScrollAmounts>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.scroll(id, amounts, control)
        })
    }

    pub(crate) fn scroll_item(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.scroll_item(id, control)
        })
    }

    pub(crate) fn find_item(
        &self,
        reference: &str,
        lookup: ItemLookup,
        after: Option<&str>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        let after = after.map(parse_reference).transpose()?;
        self.call(cancel, deadline, move |worker, control| {
            worker.find_item(id, lookup, after, control)
        })
    }

    pub(crate) fn realize(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.realize(id, control)
        })
    }
}

impl Worker {
    fn scroll(
        &mut self,
        id: Uuid,
        amounts: Option<ScrollAmounts>,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let pattern: UIScrollPattern = get_pattern(&entry.element, control)?;
        let horizontal = provider(control, || pattern.is_horizontally_scrollable())?;
        let vertical = provider(control, || pattern.is_vertically_scrollable())?;
        if let Some(amounts) = amounts {
            if (!horizontal && amounts.horizontal != ScrollAmount::NoAmount)
                || (!vertical && amounts.vertical != ScrollAmount::NoAmount)
            {
                return Err(invalid_pattern_argument("目标在指定方向上不可滚动"));
            }
            if amounts.horizontal != ScrollAmount::NoAmount
                || amounts.vertical != ScrollAmount::NoAmount
            {
                provider(control, || {
                    pattern.scroll(amounts.horizontal, amounts.vertical)
                })?;
            }
        }
        let h_percent = provider(control, || pattern.get_horizontal_scroll_percent())?;
        let v_percent = provider(control, || pattern.get_vertical_scroll_percent())?;
        let h_size = provider(control, || pattern.get_horizontal_view_size())?;
        let v_size = provider(control, || pattern.get_vertical_view_size())?;
        Ok(pattern_result(
            id,
            "Scroll",
            if amounts.is_some() { "scroll" } else { "state" },
            json!({
                "horizontal": {"scrollable": horizontal, "percent": horizontal.then_some(h_percent), "view_size": h_size},
                "vertical": {"scrollable": vertical, "percent": vertical.then_some(v_percent), "view_size": v_size},
            }),
        ))
    }

    fn scroll_item(&mut self, id: Uuid, control: &RequestControl) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let pattern: UIScrollItemPattern = get_pattern(&entry.element, control)?;
        provider(control, || pattern.scroll_into_view())?;
        let entry = self.resolve_element(id, control)?;
        let offscreen = provider(control, || entry.element.is_offscreen())?;
        let bounds = provider(control, || entry.element.get_bounding_rectangle())?;
        Ok(pattern_result(
            id,
            "ScrollItem",
            "scroll_into_view",
            json!({
                "offscreen": offscreen,
                "bounds": {"left": bounds.get_left(), "top": bounds.get_top(),
                           "right": bounds.get_right(), "bottom": bounds.get_bottom()},
            }),
        ))
    }

    fn find_item(
        &mut self,
        id: Uuid,
        lookup: ItemLookup,
        after: Option<Uuid>,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let container = self.resolve_element(id, control)?;
        let after = after
            .map(|after| self.item_start(id, after, &container, control))
            .transpose()?;
        let pattern: UIItemContainerPattern = get_pattern(&container.element, control)?;
        let element = lookup_item(&pattern, after.as_ref(), &lookup, control)?
            .ok_or_else(|| ToolFailure::new("uia_element_not_found", "容器中没有后续匹配元素"))?;
        let item_id = Uuid::new_v4();
        let data = match get_pattern::<UIVirtualizedItemPattern>(&element, control) {
            Err(error) if error.code == "uia_pattern_not_supported" => {
                let entry = capture_item(&self.automation, element, id, container.window, control)?;
                let data = item_fields(item_id, &entry, control)?;
                self.elements.insert(item_id, entry)?;
                data
            }
            Ok(_) => {
                if self.virtual_items.items.len() >= MAX_VIRTUAL_ITEMS {
                    return Err(ToolFailure::new(
                        "uia_reference_limit",
                        "UIA 虚拟项目引用已达上限",
                    ));
                }
                self.virtual_items.items.insert(
                    item_id,
                    VirtualItem {
                        element,
                        container: id,
                        window: container.window,
                        lookup,
                    },
                );
                json!({"ref": reference_token(item_id), "status": "virtualized", "generation": 1})
            }
            Err(error) => return Err(error),
        };
        Ok(pattern_result(id, "ItemContainer", "find_item", data))
    }

    fn item_start(
        &mut self,
        container_id: Uuid,
        after: Uuid,
        container: &Element,
        control: &RequestControl,
    ) -> Result<UIElement, ToolFailure> {
        if let Some(entry) = self.virtual_items.items.get(&after) {
            if entry.container != container_id {
                return Err(invalid_pattern_argument("起始虚拟项目属于其他容器"));
            }
            return Ok(entry.element.clone());
        }
        let entry = self.resolve_element(after, control)?;
        if entry.window != container.window {
            return Err(invalid_pattern_argument("起始项目属于其他窗口"));
        }
        Ok(entry.element)
    }

    fn realize(&mut self, id: Uuid, control: &RequestControl) -> Result<Value, ToolFailure> {
        let entry = if let Some(item) = self.virtual_items.items.get(&id).cloned() {
            self.elements.reserve(1)?;
            let container = self.resolve_element(item.container, control)?;
            let pattern: UIVirtualizedItemPattern = get_pattern(&item.element, control)?;
            provider(control, || pattern.realize())?;
            let mut entry = match capture_item(
                &self.automation,
                item.element,
                item.container,
                item.window,
                control,
            ) {
                Ok(entry) => entry,
                Err(error) if error.code == "uia_element_unavailable" => {
                    let pattern: UIItemContainerPattern = get_pattern(&container.element, control)?;
                    let first = lookup_item(&pattern, None, &item.lookup, control)?.ok_or(error)?;
                    if lookup_item(&pattern, Some(&first), &item.lookup, control)?.is_some() {
                        return Err(ToolFailure::new(
                            "uia_ambiguous_target",
                            "实例化后的重新查找得到多个候选",
                        ));
                    }
                    capture_item(
                        &self.automation,
                        first,
                        item.container,
                        item.window,
                        control,
                    )?
                }
                Err(error) => return Err(error),
            };
            if let Some(window) = entry.window {
                validate_window(window)?;
            }
            entry.generation += 1;
            self.elements.insert(id, entry.clone())?;
            self.virtual_items.items.remove(&id);
            entry
        } else {
            let entry = self.resolve_element(id, control)?;
            let pattern: UIVirtualizedItemPattern = get_pattern(&entry.element, control)?;
            provider(control, || pattern.realize())?;
            self.resolve_element(id, control)?
        };
        let mut data = item_fields(id, &entry, control)?;
        data["patterns"] = self.read_patterns(id, control)?["patterns"].clone();
        Ok(pattern_result(id, "VirtualizedItem", "realize", data))
    }
}

fn capture_item(
    automation: &uiautomation::UIAutomation,
    element: UIElement,
    container: Uuid,
    window: Option<WindowId>,
    control: &RequestControl,
) -> Result<Element, ToolFailure> {
    Element::capture(
        automation,
        element,
        window,
        false,
        Some(Locator::new(
            container,
            TreeView::Control,
            MAX_TREE_DEPTH as usize,
        )),
        control,
    )
}

fn lookup_item(
    pattern: &UIItemContainerPattern,
    after: Option<&UIElement>,
    lookup: &ItemLookup,
    control: &RequestControl,
) -> Result<Option<UIElement>, ToolFailure> {
    control.check()?;
    let result = native::find_item(
        pattern,
        after,
        lookup.property,
        &Variant::from(&lookup.value),
    );
    control.check()?;
    neighbor(result)
}

fn item_fields(id: Uuid, entry: &Element, control: &RequestControl) -> Result<Value, ToolFailure> {
    let summary = summarize_node(id, entry, control)?;
    Ok(
        json!({"ref":summary.reference, "name":summary.name, "class_name":summary.class_name,
        "control_type":summary.control_type, "window_id":summary.window_id,
        "generation":entry.generation, "status":"valid"}),
    )
}

fn scroll_amount(value: &str) -> Result<ScrollAmount, ToolFailure> {
    match value {
        "none" => Ok(ScrollAmount::NoAmount),
        "small_decrement" => Ok(ScrollAmount::SmallDecrement),
        "small_increment" => Ok(ScrollAmount::SmallIncrement),
        "large_decrement" => Ok(ScrollAmount::LargeDecrement),
        "large_increment" => Ok(ScrollAmount::LargeIncrement),
        _ => Err(invalid_pattern_argument("无效的 UIA 滚动量")),
    }
}
