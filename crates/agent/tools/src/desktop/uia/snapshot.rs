use super::elements::{Element, parse_reference, reference_token, validate_window};
use super::events::{EventSpec, Listener, Notice};
use super::properties::{Property, element_cache, format_property, parse_properties};
use super::references::Locator;
use super::tree::{MAX_TREE_DEPTH, TreeView, advance};
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::{ToolFailure, desktop::WindowId};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation, UIElement,
    events::UIEventType,
    types::{StructureChangeType, TreeScope, UIProperty},
};
use uuid::Uuid;

const MAX_SNAPSHOTS: usize = 8;
const MAX_NODES: i64 = 64;
const QUEUE_CAPACITY: usize = 256;

#[derive(Default)]
pub(super) struct SnapshotState {
    entries: HashMap<Uuid, Snapshot>,
}

pub(crate) struct SnapshotResult {
    pub data: Value,
    pub generation: i64,
}

pub(crate) struct SnapshotRequest {
    root: Uuid,
    view: TreeView,
    properties: Vec<Property>,
    limit: usize,
}

struct Snapshot {
    request: SnapshotRequest,
    window: WindowId,
    generation: i64,
    truncated: bool,
    nodes: Vec<Node>,
    properties: HashMap<Uuid, Listener>,
    observers: Vec<Listener>,
    signal: Signal,
    notices: Receiver<Impact>,
    loss_epoch: u64,
    dirty: Dirty,
}

#[derive(Clone)]
struct Node {
    id: Uuid,
    parent: Option<Uuid>,
    depth: usize,
    runtime_id: Vec<i32>,
    fields: Map<String, Value>,
}

#[derive(Clone)]
struct Signal {
    sender: SyncSender<Impact>,
    overflow: Arc<AtomicBool>,
}

enum Impact {
    Property(Uuid, UIProperty),
    Structure(StructureChangeType, Option<Vec<i32>>),
    Focus,
    Text,
}

#[derive(Default)]
struct Dirty {
    full: bool,
    branches: HashSet<Uuid>,
    fields: HashMap<Uuid, Vec<UIProperty>>,
    focus: bool,
    text: bool,
}

enum Scope {
    Auto,
    Full,
    Branch(Uuid),
}

#[derive(Default)]
struct Pending {
    references: Vec<(Uuid, Element)>,
    properties: HashMap<Uuid, Listener>,
}

impl SnapshotRequest {
    pub(crate) fn new(
        root: &str,
        view: &str,
        properties: &[String],
        limit: i64,
    ) -> Result<Self, ToolFailure> {
        if !(1..=MAX_NODES).contains(&limit) {
            return Err(invalid("快照节点预算须为 1–64"));
        }
        Ok(Self {
            root: parse_reference(root)?,
            view: view.parse()?,
            properties: parse_properties(properties)?,
            limit: limit as usize,
        })
    }
}

impl Runtime {
    pub(crate) fn snapshot(
        &self,
        request: SnapshotRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<SnapshotResult, ToolFailure> {
        self.call(cancel, deadline, move |worker, control| {
            if worker.snapshots.entries.len() >= MAX_SNAPSHOTS {
                return Err(ToolFailure::new(
                    "uia_snapshot_limit",
                    "会话的 UIA 快照已达上限",
                ));
            }
            let root = worker.resolve_element(request.root, control)?;
            let window = root
                .window
                .filter(|_| root.is_root)
                .ok_or_else(|| invalid("创建快照需要窗口根元素引用"))?;
            let id = Uuid::new_v4();
            let mut snapshot = Snapshot::new(request, window, worker.events.loss_epoch());
            let result = snapshot
                .observe(worker, &root.element, control)
                .and_then(|()| snapshot.refresh(worker, id, Scope::Full, control));
            if result.is_ok() {
                worker.snapshots.entries.insert(id, snapshot);
            } else {
                snapshot.close(worker);
            }
            result
        })
    }

    pub(crate) fn snapshot_refresh(
        &self,
        id: &str,
        scope: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<SnapshotResult, ToolFailure> {
        let id = snapshot_id(id)?;
        let scope = match scope {
            "auto" => Scope::Auto,
            "full" => Scope::Full,
            reference => Scope::Branch(parse_reference(reference)?),
        };
        self.call(cancel, deadline, move |worker, control| {
            let mut snapshot = worker
                .snapshots
                .entries
                .remove(&id)
                .ok_or_else(unavailable)?;
            let result = snapshot.refresh(worker, id, scope, control);
            worker.snapshots.entries.insert(id, snapshot);
            result
        })
    }

    pub(crate) fn snapshot_status(
        &self,
        id: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<SnapshotResult, ToolFailure> {
        let id = snapshot_id(id)?;
        self.call(cancel, deadline, move |worker, _| {
            let snapshot = worker
                .snapshots
                .entries
                .get_mut(&id)
                .ok_or_else(unavailable)?;
            let window_available = validate_window(snapshot.window).is_ok();
            snapshot.drain(worker.events.loss_epoch());
            snapshot.dirty.full |= !window_available;
            if snapshot.dirty.full || !snapshot.dirty.branches.is_empty() {
                worker.tree.invalidate_window(snapshot.window);
            }
            Ok(SnapshotResult {
                data: snapshot.summary(id, window_available),
                generation: snapshot.generation,
            })
        })
    }
}

impl Snapshot {
    fn new(request: SnapshotRequest, window: WindowId, loss_epoch: u64) -> Self {
        let (sender, notices) = sync_channel(QUEUE_CAPACITY);
        Self {
            request,
            window,
            generation: 0,
            truncated: false,
            nodes: Vec::new(),
            observers: Vec::new(),
            properties: HashMap::new(),
            notices,
            signal: Signal {
                sender,
                overflow: Arc::new(AtomicBool::new(false)),
            },
            loss_epoch,
            dirty: Dirty::default(),
        }
    }

    fn observe(
        &mut self,
        worker: &mut Worker,
        root: &UIElement,
        control: &RequestControl,
    ) -> Result<(), ToolFailure> {
        worker.events.reap(&worker.automation)?;
        let signal = self.signal.clone();
        self.observers.push(Listener::register(
            &worker.automation,
            root,
            TreeScope::Subtree,
            &EventSpec::Structure,
            move |notice| {
                if let Notice::Structure(change, id) = notice {
                    signal.send(Impact::Structure(change, id));
                }
            },
        )?);
        for (property, spec, impact) in [
            (
                UIProperty::HasKeyboardFocus,
                EventSpec::Focus,
                Impact::Focus,
            ),
            (
                UIProperty::ValueValue,
                EventSpec::Automation(UIEventType::Text_TextChanged),
                Impact::Text,
            ),
        ] {
            if self
                .request
                .properties
                .iter()
                .any(|field| field.id == property)
            {
                control.check()?;
                let signal = self.signal.clone();
                let is_focus = matches!(impact, Impact::Focus);
                self.observers.push(Listener::register(
                    &worker.automation,
                    root,
                    TreeScope::Subtree,
                    &spec,
                    move |_| {
                        signal.send(if is_focus {
                            Impact::Focus
                        } else {
                            Impact::Text
                        });
                    },
                )?);
            }
        }
        Ok(())
    }

    fn drain(&mut self, loss_epoch: u64) {
        let local_overflow = self.signal.overflow.swap(false, Ordering::AcqRel);
        self.dirty.full |= self.loss_epoch != loss_epoch || local_overflow;
        self.loss_epoch = loss_epoch;
        let mut count = 0;
        for impact in self.notices.try_iter().take(QUEUE_CAPACITY) {
            count += 1;
            if self.dirty.full {
                continue;
            }
            match impact {
                Impact::Property(id, property) => {
                    if self.nodes.iter().any(|node| node.id == id) {
                        let fields = self.dirty.fields.entry(id).or_default();
                        if !fields.contains(&property) {
                            fields.push(property);
                        }
                    }
                }
                Impact::Structure(change, id) => {
                    let parent = id
                        .as_ref()
                        .filter(|_| change == StructureChangeType::ChildRemoved)
                        .and_then(|id| self.nodes.iter().find(|node| &node.runtime_id == id))
                        .and_then(|node| node.parent);
                    if let Some(parent) = parent {
                        self.dirty.branches.insert(parent);
                    } else {
                        self.dirty.full = true;
                    }
                }
                Impact::Focus => self.dirty.focus = true,
                Impact::Text => self.dirty.text = true,
            }
        }
        self.dirty.full |= count == QUEUE_CAPACITY;
    }

    fn refresh(
        &mut self,
        worker: &mut Worker,
        id: Uuid,
        scope: Scope,
        control: &RequestControl,
    ) -> Result<SnapshotResult, ToolFailure> {
        validate_window(self.window)?;
        self.drain(worker.events.loss_epoch());
        let requested = match scope {
            Scope::Branch(id) if self.nodes.iter().any(|node| node.id == id) => Some(id),
            Scope::Branch(_) => return Err(invalid("指定刷新子树不属于此快照")),
            Scope::Auto | Scope::Full => None,
        };
        let mut branches = self.dirty.branches.clone();
        branches.extend(requested);
        let full = matches!(scope, Scope::Full)
            || self.dirty.full
            || branches.len() > 1
            || (!branches.is_empty() && self.truncated)
            || branches.contains(&self.request.root);
        if !full && !self.dirty.any() && branches.is_empty() {
            return Ok(self.result(id, "unchanged"));
        }
        if full || !branches.is_empty() {
            worker.tree.invalidate_window(self.window);
        }
        worker.events.reap(&worker.automation)?;
        let mut pending = Pending::default();
        let prepared = self
            .prepare(
                worker,
                full,
                branches.into_iter().next(),
                &mut pending,
                control,
            )
            .and_then(|(nodes, truncated, full)| {
                validate_window(self.window)?;
                control.check()?;
                let generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| ToolFailure::new("uia_snapshot_limit", "UIA 快照代次已耗尽"))?;
                Ok((nodes, truncated, full, generation))
            });
        let (nodes, truncated, full, generation) = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.dirty.full = true;
                pending.close(worker);
                return Err(error);
            }
        };
        for (id, entry) in pending.references {
            worker.elements.insert(id, entry);
        }
        self.properties.extend(pending.properties);
        let obsolete: Vec<_> = self
            .properties
            .keys()
            .filter(|id| !nodes.iter().any(|node| node.id == **id))
            .copied()
            .collect();
        for id in obsolete {
            if let Some(listener) = self.properties.remove(&id) {
                retire(worker, listener);
            }
        }
        self.nodes = nodes;
        self.truncated = truncated;
        self.generation = generation;
        self.dirty = Dirty::default();
        self.drain(worker.events.loss_epoch());
        Ok(self.result(id, if full { "full" } else { "incremental" }))
    }

    fn prepare(
        &self,
        worker: &mut Worker,
        full: bool,
        branch: Option<Uuid>,
        pending: &mut Pending,
        control: &RequestControl,
    ) -> Result<(Vec<Node>, bool, bool), ToolFailure> {
        if full {
            let (nodes, truncated) = self.read_branch(
                worker,
                self.request.root,
                self.request.limit,
                pending,
                control,
            )?;
            return Ok((nodes, truncated, true));
        }
        let mut nodes = self.nodes.clone();
        if let Some(branch) = branch {
            let start = nodes
                .iter()
                .position(|node| node.id == branch)
                .ok_or_else(unavailable)?;
            let depth = nodes[start].depth;
            let end = nodes[start + 1..]
                .iter()
                .position(|node| node.depth <= depth)
                .map_or(nodes.len(), |index| start + 1 + index);
            let remaining = self.request.limit - (nodes.len() - (end - start));
            let (updated, truncated) =
                self.read_branch(worker, branch, remaining, pending, control)?;
            let moved = updated.iter().any(|node| {
                nodes[..start]
                    .iter()
                    .chain(nodes[end..].iter())
                    .any(|other| other.id == node.id)
            });
            if truncated || moved {
                pending.close(worker);
                let (nodes, truncated) = self.read_branch(
                    worker,
                    self.request.root,
                    self.request.limit,
                    pending,
                    control,
                )?;
                return Ok((nodes, truncated, true));
            }
            nodes.splice(start..end, updated);
        }
        self.update_fields(worker, &mut nodes, control)?;
        Ok((nodes, self.truncated, false))
    }

    fn read_branch(
        &self,
        worker: &mut Worker,
        pivot: Uuid,
        limit: usize,
        pending: &mut Pending,
        control: &RequestControl,
    ) -> Result<(Vec<Node>, bool), ToolFailure> {
        let root = worker.resolve_element(pivot, control)?;
        let initial = self.nodes.iter().find(|node| node.id == pivot);
        if initial.is_some_and(|node| node.runtime_id != root.identity.runtime_id) {
            return Err(ToolFailure::new(
                "uia_snapshot_stale",
                "节点已重新定位，须完整刷新窗口快照",
            ));
        }
        let depth = initial.map_or(0, |node| node.depth);
        let outer_parent = initial.and_then(|node| node.parent);
        let walker = self.request.view.walker(&worker.automation)?;
        let request = element_cache(&worker.automation, self.request.view)?;
        for field in &self.request.properties {
            request.add_property(field.id).map_err(provider_error)?;
        }
        let property_event = EventSpec::Property(
            self.request
                .properties
                .iter()
                .map(|field| field.id)
                .collect(),
        );
        let locator = Locator::new(
            self.request.root,
            self.request.view,
            MAX_TREE_DEPTH as usize,
        );
        let mut nodes = Vec::new();
        let mut parents = Vec::new();
        let mut next = Some(root.element.clone());
        let mut depth_limited = false;
        while nodes.len() < limit {
            let Some(element) = next.take() else { break };
            control.check()?;
            let entry = if nodes.is_empty() {
                root.clone()
            } else {
                Element::capture(
                    &worker.automation,
                    element.clone(),
                    Some(self.window),
                    false,
                    Some(locator),
                    control,
                )?
            };
            let runtime_id = entry.identity.runtime_id.clone();
            let id = if nodes.is_empty() {
                pivot
            } else if let Some(id) = self.reuse_ref(worker, &entry, &nodes) {
                id
            } else {
                let id = Uuid::new_v4();
                pending.references.push((id, entry));
                id
            };
            if !self.properties.contains_key(&id) && !pending.properties.contains_key(&id) {
                let signal = self.signal.clone();
                let listener = Listener::register(
                    &worker.automation,
                    &element,
                    TreeScope::Element,
                    &property_event,
                    move |notice| {
                        if let Notice::Property(property, _) = notice {
                            signal.send(Impact::Property(id, property));
                        }
                    },
                )?;
                pending.properties.insert(id, listener);
            }
            control.check()?;
            let cached = element
                .build_updated_cache(&request)
                .map_err(provider_error)?;
            let fields = self
                .request
                .properties
                .iter()
                .map(|property| {
                    let value = read_field(&worker.automation, &cached, *property, true, control)?;
                    Ok((property.name.into(), value))
                })
                .collect::<Result<_, ToolFailure>>()?;
            nodes.push(Node {
                id,
                parent: parents.last().map(|(_, id)| *id).or(outer_parent),
                depth: depth + parents.len(),
                runtime_id,
                fields,
            });
            let available_depth = MAX_TREE_DEPTH as usize - depth;
            depth_limited |= parents.len() == available_depth;
            next = advance(
                &walker,
                &mut parents,
                (element, id),
                available_depth,
                control,
                |(element, _)| element,
            )?;
        }
        Ok((nodes, next.is_some() || depth_limited))
    }

    fn reuse_ref(&self, worker: &Worker, current: &Element, nodes: &[Node]) -> Option<Uuid> {
        self.nodes
            .iter()
            .find(|node| {
                node.runtime_id == current.identity.runtime_id
                    && !nodes.iter().any(|used| used.id == node.id)
                    && worker
                        .elements
                        .get(node.id)
                        .is_ok_and(|entry| entry.identity.same_element(&current.identity))
            })
            .map(|node| node.id)
    }

    fn update_fields(
        &self,
        worker: &mut Worker,
        nodes: &mut [Node],
        control: &RequestControl,
    ) -> Result<(), ToolFailure> {
        let focused = if self.dirty.focus {
            control.check()?;
            Some(
                worker
                    .automation
                    .get_focused_element()
                    .map_err(provider_error)?
                    .get_runtime_id()
                    .map_err(provider_error)?,
            )
        } else {
            None
        };
        for node in nodes {
            let fields: Vec<_> = self
                .request
                .properties
                .iter()
                .copied()
                .filter(|property| {
                    self.dirty
                        .fields
                        .get(&node.id)
                        .is_some_and(|fields| fields.contains(&property.id))
                        || (self.dirty.text
                            && property.id == UIProperty::ValueValue
                            && node.fields[property.name].is_string())
                })
                .collect();
            if !fields.is_empty() {
                let entry = worker.resolve_element(node.id, control)?;
                if entry.identity.runtime_id != node.runtime_id {
                    return Err(ToolFailure::new(
                        "uia_snapshot_stale",
                        "节点已重新定位，须完整刷新窗口快照",
                    ));
                }
                for property in fields {
                    let value =
                        read_field(&worker.automation, &entry.element, property, false, control)?;
                    node.fields.insert(property.name.into(), value);
                }
            }
            if let Some(focused) = &focused
                && let Some(field) = node.fields.get_mut("HasKeyboardFocus")
                && field.is_boolean()
            {
                *field = json!(*focused == node.runtime_id);
            }
        }
        Ok(())
    }

    fn summary(&self, id: Uuid, window_available: bool) -> Value {
        json!({
            "snapshot_id":id.to_string(), "window_id":self.window.token(), "root":reference_token(self.request.root),
            "view":self.request.view.name(),
            "dirty":self.dirty.any(), "rebuild_required":self.dirty.full, "dirty_nodes":self.dirty.fields.len(),
            "dirty_branches":self.dirty.branches.len(), "focus_dirty":self.dirty.focus, "text_dirty":self.dirty.text,
            "truncated":self.truncated, "count":self.nodes.len(), "window_available":window_available,
        })
    }

    fn result(&self, id: Uuid, refresh: &str) -> SnapshotResult {
        let mut data = self.summary(id, true);
        data["refresh"] = json!(refresh);
        data["nodes"] = json!(
            self.nodes
                .iter()
                .map(|node| json!({
                    "ref":reference_token(node.id), "parent":node.parent.map(reference_token),
                    "depth":node.depth, "properties":node.fields,
                }))
                .collect::<Vec<_>>()
        );
        SnapshotResult {
            data,
            generation: self.generation,
        }
    }

    fn close(mut self, worker: &mut Worker) {
        for listener in self
            .observers
            .drain(..)
            .chain(self.properties.drain().map(|(_, listener)| listener))
        {
            retire(worker, listener);
        }
    }
}

impl Pending {
    fn close(&mut self, worker: &mut Worker) {
        self.references.clear();
        for (_, listener) in self.properties.drain() {
            retire(worker, listener);
        }
    }
}

impl Signal {
    fn send(&self, impact: Impact) {
        if matches!(self.sender.try_send(impact), Err(TrySendError::Full(_))) {
            self.overflow.store(true, Ordering::Release);
        }
    }
}

impl Dirty {
    fn any(&self) -> bool {
        self.full || self.focus || self.text || !self.branches.is_empty() || !self.fields.is_empty()
    }
}

impl Worker {
    pub(super) fn close_snapshots(&mut self) {
        let snapshots = std::mem::take(&mut self.snapshots);
        for snapshot in snapshots.entries.into_values() {
            snapshot.close(self);
        }
    }
}

fn read_field(
    automation: &UIAutomation,
    element: &UIElement,
    property: Property,
    cached: bool,
    control: &RequestControl,
) -> Result<Value, ToolFailure> {
    control.check()?;
    native::property(automation, element, property.id, cached)?.map_or(Ok(Value::Null), |value| {
        format_property(property.id, &value)
    })
}

fn retire(worker: &mut Worker, listener: Listener) {
    if let Err(error) = worker.events.retire(&worker.automation, listener) {
        tracing::debug!(%error, "UIA 快照监听移除失败，将在后续请求中重试");
    }
}

fn invalid(message: &str) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}

fn snapshot_id(id: &str) -> Result<Uuid, ToolFailure> {
    Uuid::parse_str(id).map_err(|_| invalid("UIA 快照 ID 格式无效"))
}

fn unavailable() -> ToolFailure {
    ToolFailure::new(
        "uia_snapshot_unavailable",
        "此会话没有该 UIA 快照或快照节点",
    )
}
