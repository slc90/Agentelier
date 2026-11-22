use super::elements::parse_reference;
use super::events::{EventSpec, Listener};
use super::find::FindRequest;
use super::properties::{Property, format_property, parse_properties};
use super::tree::{TreeView, advance};
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::ToolFailure;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uiautomation::{UIAutomation, UIElement, types::TreeScope};
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_WAIT_MS: i64 = 30_000;
const STRUCTURE_NODES: usize = 128;
const WAIT_DEPTH: usize = 16;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Condition {
    Appears { selector: Value },
    Disappears { selector: Value },
    Property { name: String, equals: Value },
    Enabled,
    Focused,
    FocusChanged,
    StructureChanged,
}

pub(crate) struct WaitRequest {
    reference: Uuid,
    condition: Check,
    timeout_ms: u64,
}

enum Check {
    Search { request: FindRequest, present: bool },
    Property { property: Property, expected: Value },
    Focus,
    Structure,
}

enum Baseline {
    None,
    Focus(Vec<i32>),
    Structure(Vec<(usize, Vec<i32>)>),
}

impl WaitRequest {
    pub(crate) fn new(
        reference: &str,
        condition: Value,
        timeout_ms: i64,
    ) -> Result<Self, ToolFailure> {
        if !(1..=MAX_WAIT_MS).contains(&timeout_ms) {
            return Err(invalid("等待超时须为 1–30000 毫秒"));
        }
        let condition: Condition =
            serde_json::from_value(condition).map_err(|error| invalid(error.to_string()))?;
        let condition = match condition {
            Condition::Appears { selector } => Self::search(reference, selector, true)?,
            Condition::Disappears { selector } => Self::search(reference, selector, false)?,
            Condition::Property { name, equals } => Self::property(name, equals)?,
            Condition::Enabled => Self::property("IsEnabled".into(), json!(true))?,
            Condition::Focused => Self::property("HasKeyboardFocus".into(), json!(true))?,
            Condition::FocusChanged => Check::Focus,
            Condition::StructureChanged => Check::Structure,
        };
        Ok(Self {
            reference: parse_reference(reference)?,
            condition,
            timeout_ms: timeout_ms as u64,
        })
    }

    fn search(reference: &str, selector: Value, present: bool) -> Result<Check, ToolFailure> {
        let request = FindRequest::new(
            reference,
            selector,
            "control",
            "subtree",
            WAIT_DEPTH as i64,
            512,
            1,
        )?;
        Ok(Check::Search { request, present })
    }

    fn property(name: String, expected: Value) -> Result<Check, ToolFailure> {
        if expected
            .as_str()
            .is_some_and(|text| text.chars().nth(512).is_some())
        {
            return Err(invalid("等待属性的文本值最多 512 个字符"));
        }
        let property = parse_properties(&[name])?
            .into_iter()
            .next()
            .ok_or_else(|| invalid("等待需要属性名称"))?;
        Ok(Check::Property { property, expected })
    }
}

impl Runtime {
    pub(crate) fn wait(
        &self,
        request: WaitRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let requested = Instant::now() + Duration::from_millis(request.timeout_ms);
        self.call(cancel, deadline.min(requested), move |worker, control| {
            worker.wait(request, control)
        })
        .map_err(|error| match error.code {
            "timeout" => ToolFailure::new("uia_timeout", "等待 UIA 条件超时"),
            "cancelled" => ToolFailure::new("uia_cancelled", "等待 UIA 条件已取消"),
            _ => error,
        })
    }
}

impl Worker {
    fn wait(
        &mut self,
        request: WaitRequest,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let target = self.resolve_element(request.reference, control)?.element;
        self.events.reap(&self.automation)?;
        let (sender, wake) = sync_channel(1);
        let (scope, event) = request.condition.event();
        let listener =
            Listener::register(&self.automation, &target, scope, &event, move |notice| {
                drop(sender.try_send(notice));
            })?;
        let result = (|| {
            let mut baseline = Baseline::None;
            loop {
                if let Some(value) = self.check_wait(&request, &mut baseline, control)? {
                    return Ok(value);
                }
                control.check()?;
                let interval =
                    POLL_INTERVAL.min(control.deadline.saturating_duration_since(Instant::now()));
                match wake.recv_timeout(interval) {
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err(super::worker_failed()),
                }
            }
        })();
        match self.events.retire(&self.automation, listener) {
            Ok(()) => result,
            Err(error) => {
                tracing::debug!(%error, "临时 UIA 等待监听移除失败，将在后续请求中重试");
                result.and(Err(error))
            }
        }
    }

    fn check_wait(
        &mut self,
        request: &WaitRequest,
        baseline: &mut Baseline,
        control: &RequestControl,
    ) -> Result<Option<Value>, ToolFailure> {
        let target = self.resolve_element(request.reference, control)?;
        match &request.condition {
            Check::Search { request, present } => {
                let actual = request.scan(&self.automation, &target.element, control)?;
                if *present && !actual.candidates.is_empty() {
                    let matches = self.publish_matches(request, target, actual, control)?;
                    Ok(Some(json!({"kind":"appears","matches":matches})))
                } else if !present && actual.candidates.is_empty() && !actual.truncated {
                    Ok(Some(
                        json!({"kind":"disappears","present":false,"inspected":actual.inspected}),
                    ))
                } else {
                    Ok(None)
                }
            }
            Check::Property { property, expected } => {
                let raw = native::property(&self.automation, &target.element, property.id, false)?;
                if let Some(raw) = raw.as_ref().filter(|raw| raw.is_string())
                    && expected.as_str() != Some(raw.get_string().map_err(provider_error)?.as_str())
                {
                    return Ok(None);
                }
                let value = raw.map_or(Ok(Value::Null), |value| {
                    format_property(property.id, &value)
                })?;
                Ok((&value == expected)
                    .then(|| json!({"kind":"property","property":property.name,"value":value})))
            }
            Check::Focus => {
                let focused = self
                    .automation
                    .get_focused_element()
                    .map_err(provider_error)?;
                let current = focused.get_runtime_id().map_err(provider_error)?;
                if let Baseline::Focus(initial) = baseline {
                    return Ok((current != *initial)
                        .then(|| json!({"kind":"focus_changed","focused_runtime_id":current})));
                }
                *baseline = Baseline::Focus(current);
                Ok(None)
            }
            Check::Structure => {
                let current = structure(&self.automation, target.element, control)?;
                if let Baseline::Structure(initial) = baseline {
                    return Ok((current != *initial).then(
                        || json!({"kind":"structure_changed","nodes_inspected":current.len()}),
                    ));
                }
                *baseline = Baseline::Structure(current);
                Ok(None)
            }
        }
    }
}

impl Check {
    fn event(&self) -> (TreeScope, EventSpec) {
        match self {
            Self::Search { .. } | Self::Structure => (TreeScope::Subtree, EventSpec::Structure),
            Self::Property { property, .. } => {
                (TreeScope::Element, EventSpec::Property(vec![property.id]))
            }
            Self::Focus => (TreeScope::Element, EventSpec::Focus),
        }
    }
}

fn structure(
    automation: &UIAutomation,
    root: UIElement,
    control: &RequestControl,
) -> Result<Vec<(usize, Vec<i32>)>, ToolFailure> {
    let walker = TreeView::Control.walker(automation)?;
    let mut next = Some(root);
    let mut parents = Vec::new();
    let mut nodes = Vec::new();
    for _ in 0..STRUCTURE_NODES {
        let Some(element) = next else { break };
        control.check()?;
        nodes.push((
            parents.len(),
            element.get_runtime_id().map_err(provider_error)?,
        ));
        next = advance(
            &walker,
            &mut parents,
            element,
            WAIT_DEPTH,
            control,
            |element| element,
        )?;
    }
    Ok(nodes)
}

fn invalid(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}
