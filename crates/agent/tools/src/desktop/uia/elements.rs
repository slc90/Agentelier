use super::properties::CachedElement;
use super::{RequestControl, Runtime, Worker, provider_error};
use crate::{
    ToolFailure,
    desktop::{self, WindowId},
};
use std::collections::HashMap;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{UIElement, types::Handle};
use uuid::Uuid;

const MAX_REFERENCES: usize = 4096;
const MAX_SUMMARY_CHARS: usize = 256;

#[derive(Default)]
pub(super) struct Registry {
    entries: HashMap<Uuid, Element>,
}

#[derive(Clone)]
pub(super) struct Element {
    pub(super) element: UIElement,
    pub(super) window: Option<WindowId>,
    pub(super) is_root: bool,
    pub(super) cache: Option<CachedElement>,
}

impl Element {
    pub(super) fn new(element: UIElement, window: Option<WindowId>, is_root: bool) -> Self {
        Self {
            element,
            window,
            is_root,
            cache: None,
        }
    }
}

pub(crate) struct RootSummary {
    pub reference: String,
    pub window_id: Option<String>,
    pub name: String,
    pub class_name: String,
    pub control_type: String,
}

impl Runtime {
    pub(crate) fn desktop_root(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<RootSummary, ToolFailure> {
        self.call(cancel, deadline, |worker, control| {
            worker.root(None, control)
        })
    }

    pub(crate) fn window_root(
        &self,
        window_token: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<RootSummary, ToolFailure> {
        let window = WindowId::from_token(window_token)
            .ok_or_else(|| ToolFailure::new("uia_window_unavailable", "无效的窗口标识"))?;
        self.call(cancel, deadline, move |worker, control| {
            worker.root(Some(window), control)
        })
    }

    pub(crate) fn root_info(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<RootSummary, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            let entry = worker.elements.get(id)?;
            summarize(id, entry, control)
        })
    }
}

impl Worker {
    fn root(
        &mut self,
        window: Option<WindowId>,
        control: &RequestControl,
    ) -> Result<RootSummary, ToolFailure> {
        let element = match window {
            Some(window) => {
                validate_window(window)?;
                control.check()?;
                self.automation
                    .element_from_handle(Handle::from(window.handle().cast_signed()))
            }
            None => self.automation.get_root_element(),
        }
        .map_err(provider_error)?;
        let id = Uuid::new_v4();
        let entry = Element::new(element, window, true);
        let summary = summarize(id, &entry, control)?;
        self.elements.insert(id, entry)?;
        Ok(summary)
    }
}

impl Registry {
    pub(super) fn get(&self, id: Uuid) -> Result<&Element, ToolFailure> {
        self.entries.get(&id).ok_or_else(element_unavailable)
    }

    pub(super) fn get_mut(&mut self, id: Uuid) -> Result<&mut Element, ToolFailure> {
        self.entries.get_mut(&id).ok_or_else(element_unavailable)
    }

    pub(super) fn insert(&mut self, id: Uuid, element: Element) -> Result<(), ToolFailure> {
        self.reserve(1)?;
        self.entries.insert(id, element);
        Ok(())
    }

    pub(super) fn reserve(&self, additional: usize) -> Result<(), ToolFailure> {
        if additional > MAX_REFERENCES.saturating_sub(self.entries.len()) {
            return Err(ToolFailure::new(
                "uia_reference_limit",
                "UIA 会话元素引用已达上限",
            ));
        }
        Ok(())
    }
}

pub(super) fn parse_reference(reference: &str) -> Result<Uuid, ToolFailure> {
    reference
        .strip_prefix("uia:")
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(element_unavailable)
}

pub(super) fn validate_window(window: WindowId) -> Result<(), ToolFailure> {
    desktop::resolve(window)
        .map(|_| ())
        .map_err(|error| ToolFailure::new("uia_window_unavailable", error.to_string()))
}

fn summarize(
    id: Uuid,
    entry: &Element,
    control: &RequestControl,
) -> Result<RootSummary, ToolFailure> {
    control.check()?;
    if let Some(window) = entry.window {
        validate_window(window)?;
    }
    let summary = summarize_node(id, entry, control)?;
    if let Some(window) = entry.window {
        validate_window(window)?;
    }
    Ok(summary)
}

pub(super) fn summarize_node(
    id: Uuid,
    entry: &Element,
    control: &RequestControl,
) -> Result<RootSummary, ToolFailure> {
    control.check()?;
    let name = entry.element.get_name().map_err(provider_error)?;
    control.check()?;
    let class_name = entry.element.get_classname().map_err(provider_error)?;
    control.check()?;
    let control_type = entry.element.get_control_type().map_err(provider_error)?;
    Ok(RootSummary {
        reference: reference_token(id),
        window_id: entry.window.map(WindowId::token),
        name: name.chars().take(MAX_SUMMARY_CHARS).collect(),
        class_name: class_name.chars().take(MAX_SUMMARY_CHARS).collect(),
        control_type: format!("{control_type:?}"),
    })
}

pub(super) fn reference_token(id: Uuid) -> String {
    format!("uia:{id}")
}

fn element_unavailable() -> ToolFailure {
    ToolFailure::new(
        "uia_element_unavailable",
        "元素引用无效或不属于当前 UIA 会话",
    )
}
