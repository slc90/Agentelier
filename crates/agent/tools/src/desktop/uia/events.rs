use super::elements::{parse_reference, reference_token};
use super::properties::{format_property, parse_properties};
use super::tree::{TreeView, neighbor};
use super::{RequestControl, Runtime, provider_error, worker_failed};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation, UIElement,
    events::{
        CustomEventHandlerFn, CustomFocusChangedEventHandlerFn,
        CustomPropertyChangedEventHandlerFn, CustomStructureChangedEventHandlerFn, UIEventHandler,
        UIEventType, UIFocusChangedEventHandler, UIPropertyChangedEventHandler,
        UIStructureChangeEventHandler,
    },
    types::{StructureChangeType, TreeScope, UIProperty},
};
use uuid::Uuid;
use windows::Win32::System::Variant::{
    VT_BOOL, VT_BSTR, VT_EMPTY, VT_I2, VT_I4, VT_I8, VT_INT, VT_NULL, VT_R4, VT_R8, VT_UI2, VT_UI4,
    VT_UI8, VT_UINT, VT_VOID,
};

const QUEUE_CAPACITY: usize = 256;
const MAX_SUBSCRIPTIONS: usize = 32;
const MAX_RUNTIME_ID: usize = 64;
const CANCELLATION_INTERVAL: Duration = Duration::from_millis(25);

// Windows requires the client process to serialize event registration and removal.
static EVENT_REGISTRATION: Mutex<usize> = Mutex::new(0);

pub(super) fn registration_guard() -> Result<MutexGuard<'static, usize>, ToolFailure> {
    EVENT_REGISTRATION.lock().map_err(|_| worker_failed())
}

pub(super) fn worker_connected() -> Result<(), ToolFailure> {
    *registration_guard()? += 1;
    Ok(())
}

pub(super) fn worker_disconnected(automation: &UIAutomation) -> Result<bool, ToolFailure> {
    let mut workers = registration_guard()?;
    *workers -= 1;
    if *workers != 0 {
        return Ok(false);
    }
    // This API clears registrations across the process, including other automation objects.
    automation
        .remove_all_event_handlers()
        .map_err(provider_error)?;
    Ok(true)
}

#[derive(Clone)]
pub(super) enum EventSpec {
    Automation(UIEventType),
    Property(Vec<UIProperty>),
    Structure,
    Focus,
}

pub(crate) struct EventRequest {
    spec: EventSpec,
    scope: Scope,
}

#[derive(Clone, Copy)]
enum Scope {
    Element,
    Children,
    Descendants,
    Subtree,
    Global,
}

impl Scope {
    fn native(self) -> TreeScope {
        match self {
            Self::Element => TreeScope::Element,
            Self::Children => TreeScope::Children,
            Self::Descendants => TreeScope::Descendants,
            Self::Subtree | Self::Global => TreeScope::Subtree,
        }
    }
}

impl EventRequest {
    pub(crate) fn new(spec: &str, scope: &str, properties: &[String]) -> Result<Self, ToolFailure> {
        let spec = match spec {
            "property" => EventSpec::Property(
                parse_properties(properties)?
                    .into_iter()
                    .map(|property| property.id)
                    .collect(),
            ),
            "structure" => EventSpec::Structure,
            "focus" => EventSpec::Focus,
            _ => {
                let event = spec
                    .strip_prefix("automation:")
                    .and_then(|id| id.parse::<i32>().ok())
                    .and_then(|id| UIEventType::try_from(id).ok())
                    .filter(|id| {
                        !matches!(
                            id,
                            UIEventType::StructureChanged
                                | UIEventType::AutomationPropertyChanged
                                | UIEventType::AutomationFocusChanged
                        )
                    })
                    .ok_or_else(|| {
                        invalid("事件须为 property、structure、focus 或 automation:<EventId>")
                    })?;
                EventSpec::Automation(event)
            }
        };
        if !properties.is_empty() && !matches!(spec, EventSpec::Property(_)) {
            return Err(invalid("只有 property 事件接受属性过滤列表"));
        }
        let scope = match scope {
            "element" => Scope::Element,
            "children" => Scope::Children,
            "descendants" => Scope::Descendants,
            "subtree" => Scope::Subtree,
            "global" if matches!(spec, EventSpec::Focus) => Scope::Global,
            _ => {
                return Err(invalid(
                    "事件范围须为 element、children、descendants、subtree，焦点还可用 global",
                ));
            }
        };
        Ok(Self { spec, scope })
    }
}

pub(super) enum Notice {
    Automation(UIEventType),
    Property(UIProperty, Option<Value>),
    Structure(StructureChangeType, Option<Vec<i32>>),
    Focus,
}

pub(super) enum Listener {
    Automation(UIEventType, UIElement, UIEventHandler),
    Property(UIElement, UIPropertyChangedEventHandler),
    Structure(UIElement, UIStructureChangeEventHandler),
    Focus(UIFocusChangedEventHandler),
}

impl Listener {
    pub(super) fn register(
        automation: &UIAutomation,
        target: &UIElement,
        scope: TreeScope,
        spec: &EventSpec,
        notify: impl Fn(Notice) + Send + Sync + 'static,
    ) -> Result<Self, ToolFailure> {
        let _serial = registration_guard()?;
        let listener = match spec {
            EventSpec::Automation(event) => {
                let callback: Box<CustomEventHandlerFn> = Box::new(move |_, event| {
                    notify(Notice::Automation(event));
                    Ok(())
                });
                let handler = UIEventHandler::from(callback);
                automation
                    .add_automation_event_handler(*event, target, scope, None, &handler)
                    .map_err(provider_error)?;
                Self::Automation(*event, target.clone(), handler)
            }
            EventSpec::Property(properties) => {
                let callback: Box<CustomPropertyChangedEventHandlerFn> =
                    Box::new(move |_, id, value| {
                        // Only decode scalar payloads. Other VARIANT types can contain COM references.
                        let value = match value.get_type() {
                            VT_BOOL | VT_BSTR | VT_EMPTY | VT_I2 | VT_I4 | VT_I8 | VT_INT
                            | VT_NULL | VT_R4 | VT_R8 | VT_UI2 | VT_UI4 | VT_UI8 | VT_UINT
                            | VT_VOID => format_property(id, &value).ok(),
                            _ => None,
                        };
                        notify(Notice::Property(id, value));
                        Ok(())
                    });
                let handler = UIPropertyChangedEventHandler::from(callback);
                automation
                    .add_property_changed_event_handler(target, scope, None, &handler, properties)
                    .map_err(provider_error)?;
                Self::Property(target.clone(), handler)
            }
            EventSpec::Structure => {
                let callback: Box<CustomStructureChangedEventHandlerFn> =
                    Box::new(move |_, kind, id| {
                        let id = id.filter(|id| !id.is_empty() && id.len() <= MAX_RUNTIME_ID);
                        notify(Notice::Structure(kind, id.map(<[i32]>::to_vec)));
                        Ok(())
                    });
                let handler = UIStructureChangeEventHandler::from(callback);
                automation
                    .add_structure_changed_event_handler(target, scope, None, &handler)
                    .map_err(provider_error)?;
                Self::Structure(target.clone(), handler)
            }
            EventSpec::Focus => {
                let callback: Box<CustomFocusChangedEventHandlerFn> = Box::new(move |_| {
                    notify(Notice::Focus);
                    Ok(())
                });
                let handler = UIFocusChangedEventHandler::from(callback);
                automation
                    .add_focus_changed_event_handler(None, &handler)
                    .map_err(provider_error)?;
                Self::Focus(handler)
            }
        };
        Ok(listener)
    }

    pub(super) fn remove(&self, automation: &UIAutomation) -> Result<(), ToolFailure> {
        let _serial = registration_guard()?;
        match self {
            Self::Automation(event, target, handler) => {
                automation.remove_automation_event_handler(*event, target, handler)
            }
            Self::Property(target, handler) => {
                automation.remove_property_changed_event_handler(target, handler)
            }
            Self::Structure(target, handler) => {
                automation.remove_structure_changed_event_handler(target, handler)
            }
            Self::Focus(handler) => automation.remove_focus_changed_event_handler(handler),
        }
        .map_err(provider_error)
    }
}

pub(super) struct EventState {
    sender: SyncSender<(Uuid, Notice)>,
    queue: Receiver<(Uuid, Notice)>,
    subscriptions: HashMap<Uuid, Subscription>,
}

struct Subscription {
    listener: Listener,
    target: UIElement,
    reference: Uuid,
    request: EventRequest,
    overflowed: Arc<AtomicBool>,
}

impl Default for EventState {
    fn default() -> Self {
        let (sender, queue) = sync_channel(QUEUE_CAPACITY);
        Self {
            sender,
            queue,
            subscriptions: HashMap::new(),
        }
    }
}

impl Runtime {
    pub(crate) fn subscribe(
        &self,
        reference: &str,
        request: EventRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let reference = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            let target = worker.resolve_element(reference, control)?.element;
            let id = worker
                .events
                .subscribe(&worker.automation, target, reference, request)?;
            if let Err(error) = control.check() {
                worker.events.unsubscribe(&worker.automation, id)?;
                return Err(error);
            }
            Ok(json!({"subscription_id": id.to_string(), "ref": reference_token(reference)}))
        })
    }

    pub(crate) fn unsubscribe(
        &self,
        subscription: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = Uuid::parse_str(subscription).map_err(|_| invalid("订阅 ID 格式无效"))?;
        self.call(cancel, deadline, move |worker, _| {
            worker.events.unsubscribe(&worker.automation, id)?;
            Ok(json!({"subscription_id": id.to_string(), "removed": true}))
        })
    }

    pub(crate) fn events_next(
        &self,
        timeout_ms: i64,
        max_events: i64,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        if !(0..=30_000).contains(&timeout_ms) || !(1..=64).contains(&max_events) {
            return Err(invalid("事件超时须为 0–30000 毫秒，批量须为 1–64"));
        }
        self.call(cancel, deadline, move |worker, control| {
            worker.events.next(
                &worker.automation,
                Duration::from_millis(timeout_ms as u64),
                max_events as usize,
                control,
            )
        })
    }
}

impl EventState {
    fn subscribe(
        &mut self,
        automation: &UIAutomation,
        target: UIElement,
        reference: Uuid,
        request: EventRequest,
    ) -> Result<Uuid, ToolFailure> {
        if self.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(ToolFailure::new(
                "uia_event_limit",
                "会话的 UIA 事件订阅已达上限",
            ));
        }
        let id = Uuid::new_v4();
        let sender = self.sender.clone();
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflow_flag = overflowed.clone();
        let listener = Listener::register(
            automation,
            &target,
            request.scope.native(),
            &request.spec,
            move |notice| {
                if matches!(sender.try_send((id, notice)), Err(TrySendError::Full(_))) {
                    overflow_flag.store(true, Ordering::Release);
                }
            },
        )?;
        self.subscriptions.insert(
            id,
            Subscription {
                listener,
                target,
                reference,
                request,
                overflowed,
            },
        );
        tracing::debug!(subscription = %id, "UIA 事件订阅已注册");
        Ok(id)
    }

    fn unsubscribe(&mut self, automation: &UIAutomation, id: Uuid) -> Result<(), ToolFailure> {
        let entry = self.subscriptions.get(&id).ok_or_else(|| {
            ToolFailure::new("uia_subscription_unavailable", "此会话没有该 UIA 订阅")
        })?;
        entry.listener.remove(automation)?;
        self.subscriptions.remove(&id);
        tracing::debug!(subscription = %id, "UIA 事件订阅已移除");
        Ok(())
    }

    pub(super) fn close(&mut self, automation: &UIAutomation) -> Result<(), ToolFailure> {
        let ids: Vec<_> = self.subscriptions.keys().copied().collect();
        let mut failure = None;
        for id in ids {
            if let Err(error) = self.unsubscribe(automation, id) {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn next(
        &mut self,
        automation: &UIAutomation,
        timeout: Duration,
        max_events: usize,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let end = (Instant::now() + timeout).min(control.deadline);
        let mut events = Vec::new();
        let mut scanned = 0;
        while scanned < QUEUE_CAPACITY {
            control.check()?;
            let overflowed = self
                .subscriptions
                .values()
                .any(|s| s.overflowed.load(Ordering::Acquire));
            let notice = match self.queue.try_recv() {
                Ok(notice) => notice,
                Err(_) if !events.is_empty() || overflowed || Instant::now() >= end => break,
                Err(_) => {
                    let duration = end
                        .saturating_duration_since(Instant::now())
                        .min(CANCELLATION_INTERVAL);
                    match self.queue.recv_timeout(duration) {
                        Ok(notice) => notice,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return Err(worker_failed()),
                    }
                }
            };
            scanned += 1;
            let (id, notice) = notice;
            let Some(subscription) = self.subscriptions.get(&id) else {
                continue;
            };
            if let Some(event) = subscription.project(automation, id, notice, control)? {
                events.push(event);
                if events.len() == max_events {
                    break;
                }
            }
        }
        let mut overflowed: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, s)| s.overflowed.swap(false, Ordering::AcqRel))
            .map(|(id, _)| id.to_string())
            .collect();
        overflowed.sort_unstable();
        Ok(json!({"events": events, "overflowed": overflowed}))
    }
}

impl Subscription {
    fn project(
        &self,
        automation: &UIAutomation,
        id: Uuid,
        notice: Notice,
        control: &RequestControl,
    ) -> Result<Option<Value>, ToolFailure> {
        let (kind, event_id, data) = match notice {
            Notice::Automation(event) => {
                ("automation", event, json!({"name": format!("{event:?}")}))
            }
            Notice::Property(property, value) => (
                "property",
                UIEventType::AutomationPropertyChanged,
                json!({
                    "property_id": property as i32, "name": format!("{property:?}"),
                    "value_available": value.is_some(), "new_value": value,
                }),
            ),
            Notice::Structure(change, id) => (
                "structure",
                UIEventType::StructureChanged,
                json!({
                    "change_type": format!("{change:?}"), "runtime_id": id,
                }),
            ),
            Notice::Focus => {
                let focused = automation.get_focused_element().map_err(provider_error)?;
                if !focus_in_scope(
                    automation,
                    &self.target,
                    &focused,
                    self.request.scope,
                    control,
                )? {
                    return Ok(None);
                }
                let runtime_id = focused.get_runtime_id().map_err(provider_error)?;
                (
                    "focus",
                    UIEventType::AutomationFocusChanged,
                    json!({"focused_runtime_id": (runtime_id.len() <= MAX_RUNTIME_ID).then_some(runtime_id)}),
                )
            }
        };
        Ok(Some(json!({
            "kind": kind, "event_id": event_id as i32, "subscription_id": id.to_string(),
            "ref": reference_token(self.reference), "data": data,
        })))
    }
}

fn focus_in_scope(
    automation: &UIAutomation,
    target: &UIElement,
    focused: &UIElement,
    scope: Scope,
    control: &RequestControl,
) -> Result<bool, ToolFailure> {
    if matches!(scope, Scope::Global) {
        return Ok(true);
    }
    let is_target = automation
        .compare_elements(target, focused)
        .map_err(provider_error)?;
    if is_target || matches!(scope, Scope::Element) {
        return Ok(is_target && matches!(scope, Scope::Element | Scope::Subtree));
    }
    let walker = TreeView::Raw.walker(automation)?;
    let max_depth = if matches!(scope, Scope::Children) {
        1
    } else {
        64
    };
    let mut current = focused.clone();
    for _ in 0..max_depth {
        control.check()?;
        let Some(parent) = neighbor(walker.get_parent(&current))? else {
            return Ok(false);
        };
        if automation
            .compare_elements(target, &parent)
            .map_err(provider_error)?
        {
            return Ok(true);
        }
        current = parent;
    }
    Ok(false)
}

fn invalid(message: &str) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}
