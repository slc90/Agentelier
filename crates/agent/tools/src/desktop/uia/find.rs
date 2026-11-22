use super::elements::{Element, parse_reference, reference_token, summarize_node, validate_window};
use super::tree::{MAX_TREE_DEPTH, TreeView, advance, neighbor};
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation,
    core::UICondition,
    types::{ControlType, TreeScope, UIProperty},
    variants::Variant,
};
use uuid::Uuid;

const MAX_SEARCH_NODES: i64 = 1024;
const MAX_SEARCH_RESULTS: i64 = 64;
const MAX_CONDITION_DEPTH: usize = 8;
const MAX_CONDITION_TERMS: usize = 32;
const MAX_CONDITION_CHARS: usize = 512;

pub(crate) struct FindRequest {
    root: Uuid,
    view: TreeView,
    condition: Condition,
    include_root: bool,
    max_depth: usize,
    max_nodes: usize,
    max_results: usize,
    first: bool,
}

enum Condition {
    Text(UIProperty, String),
    ControlType(ControlType),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

impl FindRequest {
    pub(crate) fn new(
        reference: &str,
        mut conditions: Value,
        view: &str,
        scope: &str,
        max_depth: i64,
        max_nodes: i64,
        max_results: i64,
    ) -> Result<Self, ToolFailure> {
        if !(0..=MAX_TREE_DEPTH).contains(&max_depth)
            || !(1..=MAX_SEARCH_NODES).contains(&max_nodes)
            || !(1..=MAX_SEARCH_RESULTS).contains(&max_results)
        {
            return Err(invalid_condition(format!(
                "查找深度须在 0..={MAX_TREE_DEPTH}，节点预算 1..={MAX_SEARCH_NODES}，候选预算 1..={MAX_SEARCH_RESULTS}"
            )));
        }
        let (include_root, max_depth) = match scope {
            "element" => (true, 0),
            "children" => (false, max_depth.min(1)),
            "descendants" => (false, max_depth),
            "subtree" => (true, max_depth),
            _ => {
                return Err(invalid_condition(
                    "查找范围须为 element、children、descendants 或 subtree",
                ));
            }
        };
        let object = conditions
            .as_object_mut()
            .ok_or_else(|| invalid_condition("UIA 查找条件须为 Map"))?;
        let first = object
            .remove("first")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| invalid_condition("first 须为布尔值"))
            })
            .transpose()?
            .unwrap_or_default();
        let mut terms = MAX_CONDITION_TERMS;
        Ok(Self {
            root: parse_reference(reference)?,
            view: view.parse()?,
            condition: Condition::parse(conditions, 0, &mut terms)?,
            include_root,
            max_depth: max_depth as usize,
            max_nodes: max_nodes as usize,
            max_results: if first { 1 } else { max_results as usize },
            first,
        })
    }
}

impl Condition {
    fn parse(value: Value, depth: usize, terms: &mut usize) -> Result<Self, ToolFailure> {
        if depth > MAX_CONDITION_DEPTH {
            return Err(invalid_condition("UIA 查找条件嵌套超出上限"));
        }
        let Value::Object(map) = value else {
            return Err(invalid_condition("UIA 查找条件须为 Map"));
        };
        if map.is_empty() || map.len() > *terms {
            return Err(invalid_condition("UIA 查找条件不能为空或超出项数预算"));
        }
        *terms -= map.len();
        let mut parsed = Vec::with_capacity(map.len());
        for (key, value) in map {
            let condition = match key.to_ascii_lowercase().as_str() {
                "name" => Self::Text(UIProperty::Name, condition_text(value)?),
                "automationid" => Self::Text(UIProperty::AutomationId, condition_text(value)?),
                "classname" => Self::Text(UIProperty::ClassName, condition_text(value)?),
                "controltype" => Self::ControlType(control_type(value)?),
                "all" | "any" => {
                    let Value::Array(children) = value else {
                        return Err(invalid_condition("all 和 any 须为非空条件数组"));
                    };
                    if children.is_empty() || children.len() > *terms {
                        return Err(invalid_condition("条件数组不能为空或超出项数预算"));
                    }
                    let children = children
                        .into_iter()
                        .map(|child| Self::parse(child, depth + 1, terms))
                        .collect::<Result<Vec<_>, _>>()?;
                    if key.eq_ignore_ascii_case("all") {
                        Self::All(children)
                    } else {
                        Self::Any(children)
                    }
                }
                "not" => Self::Not(Box::new(Self::parse(value, depth + 1, terms)?)),
                _ => return Err(invalid_condition(format!("未知的 UIA 查找条件 {key}"))),
            };
            parsed.push(condition);
        }
        Ok(Self::All(parsed))
    }

    fn compile(&self, automation: &UIAutomation) -> Result<UICondition, ToolFailure> {
        match self {
            Self::Text(property, text) => {
                automation.create_property_condition(*property, Variant::from(text), None)
            }
            Self::ControlType(kind) => automation.create_property_condition(
                UIProperty::ControlType,
                Variant::from(*kind as i32),
                None,
            ),
            Self::Not(child) => automation.create_not_condition(child.compile(automation)?),
            Self::All(children) | Self::Any(children) => {
                let mut children = children.iter();
                let first = children
                    .next()
                    .ok_or_else(|| invalid_condition("组合条件不能为空"))?;
                return children.try_fold(first.compile(automation)?, |left, child| {
                    let right = child.compile(automation)?;
                    match self {
                        Self::All(_) => automation.create_and_condition(left, right),
                        _ => automation.create_or_condition(left, right),
                    }
                    .map_err(provider_error)
                });
            }
        }
        .map_err(provider_error)
    }
}

impl Runtime {
    pub(crate) fn find(
        &self,
        request: FindRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        self.call(cancel, deadline, move |worker, control| {
            worker.find(request, control)
        })
    }
}

impl Worker {
    fn find(
        &mut self,
        request: FindRequest,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let root = self.elements.get(request.root)?;
        if let Some(window) = root.window {
            validate_window(window)?;
        }
        let window = root.window;
        let condition = request.condition.compile(&self.automation)?;
        let walker = request.view.walker(&self.automation)?;
        let mut ancestors = Vec::new();
        let mut pending = Some(root.element.clone());
        let mut matched = Vec::new();
        let mut inspected = 0;
        let mut truncated = false;
        while let Some(element) = pending.take() {
            control.check()?;
            let at_root = ancestors.is_empty();
            if request.include_root || !at_root {
                inspected += 1;
                if neighbor(element.find_first(TreeScope::Element, &condition))?.is_some() {
                    matched.push((element.clone(), at_root));
                }
            }
            if request.first && !matched.is_empty() {
                break;
            }
            pending = advance(
                &walker,
                &mut ancestors,
                element,
                request.max_depth,
                control,
                |element| element,
            )?;
            if inspected >= request.max_nodes || matched.len() >= request.max_results {
                truncated = pending.is_some();
                break;
            }
        }
        if matched.is_empty() {
            let message = if truncated {
                "在节点预算内未找到匹配的 UIA 元素"
            } else {
                "查找范围内没有匹配的 UIA 元素"
            };
            return Err(ToolFailure::new("uia_element_not_found", message));
        }
        self.elements
            .reserve(matched.iter().filter(|(_, root)| !root).count())?;
        let mut candidates = Vec::with_capacity(matched.len());
        let mut entries = Vec::with_capacity(matched.len());
        for (element, at_root) in matched {
            let id = if at_root {
                request.root
            } else {
                Uuid::new_v4()
            };
            let entry = Element::new(element, window, at_root);
            let summary = summarize_node(id, &entry, control)?;
            let automation_id = native::property(
                &self.automation,
                &entry.element,
                UIProperty::AutomationId,
                false,
            )?
            .map(|value| value.get_string().map_err(provider_error))
            .transpose()?
            .map(|value| value.chars().take(MAX_CONDITION_CHARS).collect::<String>());
            candidates.push(json!({
                "ref": summary.reference, "window_id": summary.window_id, "name": summary.name,
                "class_name": summary.class_name, "control_type": summary.control_type,
                "automation_id": automation_id,
            }));
            if !at_root {
                entries.push((id, entry));
            }
        }
        control.check()?;
        if let Some(window) = window {
            validate_window(window)?;
        }
        for (id, entry) in entries {
            self.elements.insert(id, entry)?;
        }
        Ok(json!({
            "root": reference_token(request.root), "view": request.view.name(), "source": "live",
            "count": candidates.len(), "ambiguous": candidates.len() > 1,
            "inspected": inspected, "truncated": truncated, "candidates": candidates,
        }))
    }
}

fn condition_text(value: Value) -> Result<String, ToolFailure> {
    let Value::String(text) = value else {
        return Err(invalid_condition("文本条件须为字符串"));
    };
    if text.chars().nth(MAX_CONDITION_CHARS).is_some() {
        return Err(invalid_condition("UIA 查找条件文字超过字数限制"));
    }
    Ok(text)
}

fn control_type(value: Value) -> Result<ControlType, ToolFailure> {
    let kind = match value {
        Value::String(name) => (ControlType::Button as i32..=ControlType::AppBar as i32)
            .filter_map(|id| ControlType::try_from(id).ok())
            .find(|kind| format!("{kind:?}").eq_ignore_ascii_case(&name)),
        Value::Number(id) => id
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(|id| ControlType::try_from(id).ok()),
        _ => None,
    };
    kind.ok_or_else(|| invalid_condition("未知的 UIA ControlType"))
}

fn invalid_condition(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}
