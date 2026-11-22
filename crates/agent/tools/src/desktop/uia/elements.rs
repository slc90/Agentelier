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

struct Element {
    element: UIElement,
    window: Option<WindowId>,
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
        let entry = Element { element, window };
        let summary = summarize(id, &entry, control)?;
        self.elements.insert(id, entry)?;
        Ok(summary)
    }
}

impl Registry {
    fn get(&self, id: Uuid) -> Result<&Element, ToolFailure> {
        self.entries.get(&id).ok_or_else(element_unavailable)
    }

    fn insert(&mut self, id: Uuid, element: Element) -> Result<(), ToolFailure> {
        if self.entries.len() >= MAX_REFERENCES {
            return Err(ToolFailure::new(
                "uia_reference_limit",
                "UIA 会话元素引用已达上限",
            ));
        }
        self.entries.insert(id, element);
        Ok(())
    }
}

fn parse_reference(reference: &str) -> Result<Uuid, ToolFailure> {
    reference
        .strip_prefix("uia:")
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(element_unavailable)
}

fn validate_window(window: WindowId) -> Result<(), ToolFailure> {
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
    let name = entry.element.get_name().map_err(provider_error)?;
    control.check()?;
    let class_name = entry.element.get_classname().map_err(provider_error)?;
    control.check()?;
    let control_type = entry.element.get_control_type().map_err(provider_error)?;
    if let Some(window) = entry.window {
        validate_window(window)?;
    }
    Ok(RootSummary {
        reference: format!("uia:{id}"),
        window_id: entry.window.map(WindowId::token),
        name: name.chars().take(MAX_SUMMARY_CHARS).collect(),
        class_name: class_name.chars().take(MAX_SUMMARY_CHARS).collect(),
        control_type: format!("{control_type:?}"),
    })
}

fn element_unavailable() -> ToolFailure {
    ToolFailure::new(
        "uia_element_unavailable",
        "元素引用无效或不属于当前 UIA 会话",
    )
}
