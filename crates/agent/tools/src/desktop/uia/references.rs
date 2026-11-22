use super::elements::{Element, parse_reference, reference_token, summarize_node, validate_window};
use super::find::{Condition, SearchScope};
use super::tree::TreeView;
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::{ToolFailure, desktop::WindowId};
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation, UIElement,
    types::{ControlType, UIProperty},
};
use uuid::Uuid;

const MAX_RELOCATION_NODES: usize = 512;
const MAX_ANCESTORS: usize = 32;
const MAX_LOCATOR_CHARS: usize = 512;

#[derive(Clone, Copy)]
pub(super) struct Locator {
    anchor: Uuid,
    view: TreeView,
    max_depth: usize,
}

impl Locator {
    pub(super) fn new(anchor: Uuid, view: TreeView, max_depth: usize) -> Self {
        Self {
            anchor,
            view,
            max_depth,
        }
    }
}

#[derive(Clone)]
pub(super) struct Identity {
    pub(super) runtime_id: Vec<i32>,
    pub(super) name: String,
    pub(super) class_name: String,
    pub(super) control_type: ControlType,
    pub(super) automation_id: Option<String>,
    process_id: u32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ReferenceState {
    Valid,
    Unavailable,
    Ambiguous,
}

impl ReferenceState {
    fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unavailable => "unavailable",
            Self::Ambiguous => "ambiguous",
        }
    }
}

impl Identity {
    pub(super) fn read(
        automation: &UIAutomation,
        element: &UIElement,
        control: &RequestControl,
    ) -> Result<Self, ToolFailure> {
        control.check()?;
        let runtime_id = element.get_runtime_id().map_err(provider_error)?;
        control.check()?;
        let name = element.get_name().map_err(provider_error)?;
        control.check()?;
        let class_name = element.get_classname().map_err(provider_error)?;
        control.check()?;
        let control_type = element.get_control_type().map_err(provider_error)?;
        control.check()?;
        let automation_id = native::property(automation, element, UIProperty::AutomationId, false)?
            .map(|value| value.get_string().map_err(provider_error))
            .transpose()?;
        control.check()?;
        let process_id = element.get_process_id().map_err(provider_error)?;
        Ok(Self {
            runtime_id,
            name,
            class_name,
            control_type,
            automation_id,
            process_id,
        })
    }

    pub(super) fn same_element(&self, current: &Self) -> bool {
        let data_item = matches!(
            self.control_type,
            ControlType::ListItem | ControlType::TreeItem | ControlType::DataItem
        );
        self.runtime_id == current.runtime_id
            && self.control_type == current.control_type
            && self.class_name == current.class_name
            && self.automation_id == current.automation_id
            && self.process_id == current.process_id
            && (!data_item || self.name == current.name)
    }

    fn criteria(&self) -> Option<Condition> {
        let texts = [
            (
                UIProperty::AutomationId,
                self.automation_id.as_deref().unwrap_or_default(),
            ),
            (UIProperty::Name, self.name.as_str()),
            (UIProperty::ClassName, self.class_name.as_str()),
        ];
        if texts[0].1.is_empty() && texts[1].1.is_empty() {
            return None;
        }
        let mut conditions = vec![Condition::ControlType(self.control_type)];
        for (property, text) in texts {
            if text.chars().nth(MAX_LOCATOR_CHARS).is_some() {
                return None;
            }
            if !text.is_empty() {
                conditions.push(Condition::Text(property, text.to_owned()));
            }
        }
        Some(Condition::All(conditions))
    }
}

impl Runtime {
    pub(crate) fn ref_info(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.reference_info(id, control)
        })
    }

    pub(crate) fn refresh_element(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            let entry = worker.resolve_element(id, control)?;
            let summary = summarize_node(id, &entry, control)?;
            Ok(json!({
                "ref": summary.reference, "window_id": summary.window_id, "generation": entry.generation,
                "name": summary.name, "class_name": summary.class_name,
                "control_type": summary.control_type, "automation_id": entry.identity.automation_id,
                "runtime_id": entry.identity.runtime_id, "status": entry.state.name(), "source": "live",
            }))
        })
    }
}

impl Worker {
    pub(super) fn resolve_element(
        &mut self,
        id: Uuid,
        control: &RequestControl,
    ) -> Result<Element, ToolFailure> {
        self.resolve_at(id, control, 0)
    }

    fn resolve_at(
        &mut self,
        id: Uuid,
        control: &RequestControl,
        ancestors: usize,
    ) -> Result<Element, ToolFailure> {
        control.check()?;
        if ancestors >= MAX_ANCESTORS {
            return Err(unavailable("UIA 重定位超出父级范围"));
        }
        let mut entry = self.elements.get(id)?.clone();
        let current = self.check_identity(&entry, control);
        match current {
            Ok(Some(identity)) => {
                entry.identity = identity;
                entry.state = ReferenceState::Valid;
                *self.elements.get_mut(id)? = entry.clone();
                return Ok(entry);
            }
            Err(error) if error.code == "uia_window_unavailable" => {
                self.elements.get_mut(id)?.state = ReferenceState::Unavailable;
                return Err(error);
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }
        match self.relocate(&entry, control, ancestors) {
            Ok((element, identity)) => {
                entry.generation = entry
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| unavailable("UIA 引用代次已耗尽"))?;
                entry.element = element;
                entry.identity = identity;
                entry.cache = None;
                entry.state = ReferenceState::Valid;
                *self.elements.get_mut(id)? = entry.clone();
                Ok(entry)
            }
            Err(error) => {
                let entry = self.elements.get_mut(id)?;
                entry.state = if error.code == "uia_ambiguous_target" {
                    ReferenceState::Ambiguous
                } else {
                    ReferenceState::Unavailable
                };
                Err(error)
            }
        }
    }

    fn check_identity(
        &self,
        entry: &Element,
        control: &RequestControl,
    ) -> Result<Option<Identity>, ToolFailure> {
        if let Some(window) = entry.window {
            validate_window(window)?;
        }
        match Identity::read(&self.automation, &entry.element, control) {
            Ok(identity) => Ok(entry.identity.same_element(&identity).then_some(identity)),
            Err(error) if error.code == "uia_element_unavailable" => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn relocate(
        &mut self,
        entry: &Element,
        control: &RequestControl,
        ancestors: usize,
    ) -> Result<(UIElement, Identity), ToolFailure> {
        let Some(locator) = entry.locator else {
            return Err(unavailable("UIA 元素已失效，且没有可用的定位范围"));
        };
        let condition = entry
            .identity
            .criteria()
            .ok_or_else(|| unavailable("UIA 元素缺少可重定位的属性"))?;
        let anchor = self.resolve_at(locator.anchor, control, ancestors + 1)?;
        let scope = SearchScope {
            view: locator.view,
            include_root: false,
            max_depth: locator.max_depth,
            max_nodes: MAX_RELOCATION_NODES,
            max_results: 2,
            first: false,
        };
        let found = scope.scan(&self.automation, &anchor.element, &condition, control)?;
        if found.candidates.len() > 1 {
            return Err(ToolFailure::new(
                "uia_ambiguous_target",
                "UIA 重定位得到多个匹配元素",
            ));
        }
        if found.truncated {
            return Err(unavailable("已用完 UIA 重定位的搜索预算，无法确认目标唯一"));
        }
        let (element, _) = found
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| unavailable("原搜索范围中没有匹配的 UIA 元素"))?;
        let identity = Identity::read(&self.automation, &element, control)?;
        if identity.process_id != entry.identity.process_id {
            return Err(unavailable("找到的 UIA 元素归属其他进程"));
        }
        if let Some(window) = entry.window {
            validate_window(window)?;
        }
        Ok((element, identity))
    }

    fn reference_info(&self, id: Uuid, control: &RequestControl) -> Result<Value, ToolFailure> {
        if let Some(info) = self.virtual_items.info(id) {
            return info;
        }
        let entry = self.elements.get(id)?;
        let (state, window_available) = match self.check_identity(entry, control) {
            Ok(Some(_)) => (ReferenceState::Valid, true),
            Ok(None) if entry.state == ReferenceState::Ambiguous => {
                (ReferenceState::Ambiguous, true)
            }
            Ok(None) => (ReferenceState::Unavailable, true),
            Err(error) if error.code == "uia_window_unavailable" => {
                (ReferenceState::Unavailable, false)
            }
            Err(error) => return Err(error),
        };
        Ok(json!({
            "ref": reference_token(id), "window_id": entry.window.map(WindowId::token),
            "window_available": window_available, "runtime_id": entry.identity.runtime_id,
            "generation": entry.generation, "status": state.name(),
            "can_relocate": window_available && entry.locator.is_some() && entry.identity.criteria().is_some(),
        }))
    }
}

fn unavailable(message: &'static str) -> ToolFailure {
    ToolFailure::new("uia_element_unavailable", message)
}
