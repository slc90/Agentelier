use super::elements::{
    Element, Registry, parse_reference, reference_token, summarize_node, validate_window,
};
use super::events::registration_guard;
use super::references::Locator;
use super::{RequestControl, RootSummary, Runtime, Worker, provider_error};
use crate::{ToolFailure, desktop::WindowId};
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation, UIElement,
    core::{UICondition, UITreeWalker},
    errors::Error as UiaError,
    events::{CustomStructureChangedEventHandlerFn, UIStructureChangeEventHandler},
    types::TreeScope,
};
use uuid::Uuid;

const MAX_CURSORS: usize = 16;
pub(super) const MAX_TREE_DEPTH: i64 = 32;
const MAX_PAGE_NODES: i64 = 64;
const DEFAULT_DEPTH: usize = 2;
const DEFAULT_PAGE_NODES: usize = 16;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) enum TreeView {
    Raw,
    #[default]
    Control,
    Content,
}

impl TreeView {
    pub(super) fn walker(self, automation: &UIAutomation) -> Result<UITreeWalker, ToolFailure> {
        match self {
            Self::Raw => automation.get_raw_view_walker(),
            Self::Control => automation.get_control_view_walker(),
            Self::Content => automation.get_content_view_walker(),
        }
        .map_err(provider_error)
    }

    pub(super) fn filter(self, automation: &UIAutomation) -> Result<UICondition, ToolFailure> {
        match self {
            Self::Raw => automation.create_true_condition(),
            Self::Control => automation.get_control_view_condition(),
            Self::Content => automation.get_content_view_condition(),
        }
        .map_err(provider_error)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Control => "control",
            Self::Content => "content",
        }
    }
}

impl FromStr for TreeView {
    type Err = ToolFailure;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "raw" => Ok(Self::Raw),
            "control" => Ok(Self::Control),
            "content" => Ok(Self::Content),
            _ => Err(invalid_tree_argument("树视图须为 raw、control 或 content")),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TreeRelation {
    Parent,
    FirstChild,
    NextSibling,
    PreviousSibling,
}

impl FromStr for TreeRelation {
    type Err = ToolFailure;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "parent" => Ok(Self::Parent),
            "first_child" => Ok(Self::FirstChild),
            "next_sibling" => Ok(Self::NextSibling),
            "previous_sibling" => Ok(Self::PreviousSibling),
            _ => Err(invalid_tree_argument("未知的 UIA 树导航方向")),
        }
    }
}

pub(crate) struct TreeRequest {
    root: Uuid,
    view: TreeView,
    max_depth: usize,
    max_nodes: usize,
    cursor: Option<Uuid>,
}

impl TreeRequest {
    pub(crate) fn for_root(reference: &str) -> Result<Self, ToolFailure> {
        Ok(Self {
            root: parse_reference(reference)?,
            view: TreeView::default(),
            max_depth: DEFAULT_DEPTH,
            max_nodes: DEFAULT_PAGE_NODES,
            cursor: None,
        })
    }

    pub(crate) fn new(
        reference: &str,
        view: &str,
        max_depth: i64,
        max_nodes: i64,
        cursor: Option<&str>,
    ) -> Result<Self, ToolFailure> {
        if !(0..=MAX_TREE_DEPTH).contains(&max_depth) || !(1..=MAX_PAGE_NODES).contains(&max_nodes)
        {
            return Err(invalid_tree_argument(format!(
                "树深度须在 0..={MAX_TREE_DEPTH}，页节点数须在 1..={MAX_PAGE_NODES}"
            )));
        }
        Ok(Self {
            root: parse_reference(reference)?,
            view: view.parse()?,
            max_depth: max_depth as usize,
            max_nodes: max_nodes as usize,
            cursor: cursor.map(parse_cursor).transpose()?,
        })
    }
}

pub(crate) struct TreePage {
    pub view: TreeView,
    pub root: String,
    pub generation: i64,
    pub cursor: Option<String>,
    pub nodes: Vec<TreeNode>,
    pub issue: Option<TreeIssue>,
}

pub(crate) struct TreeNode {
    pub summary: RootSummary,
    pub parent: Option<String>,
    pub depth: usize,
}

pub(crate) struct TreeIssue {
    pub cause: ToolFailure,
    pub parent: Option<String>,
    pub depth: usize,
}

#[derive(Default)]
pub(super) struct TreeState {
    cursors: VecDeque<Cursor>,
    generation: i64,
}

struct Cursor {
    id: Uuid,
    root: Uuid,
    window: Option<WindowId>,
    view: TreeView,
    max_depth: usize,
    generation: i64,
    watch: StructureWatch,
    parents: Vec<Position>,
    pending: Option<UIElement>,
}

struct Position {
    id: Uuid,
    element: UIElement,
}

struct StructureWatch {
    root: UIElement,
    handler: UIStructureChangeEventHandler,
    changed: Arc<AtomicBool>,
}

impl Runtime {
    pub(crate) fn tree(
        &self,
        request: TreeRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<TreePage, ToolFailure> {
        self.call(cancel, deadline, move |worker, control| {
            worker.read_tree(request, control)
        })
    }

    pub(crate) fn tree_related(
        &self,
        reference: &str,
        view: TreeView,
        relation: TreeRelation,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Option<RootSummary>, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.related(id, view, relation, control)
        })
    }
}

impl Worker {
    fn read_tree(
        &mut self,
        request: TreeRequest,
        control: &RequestControl,
    ) -> Result<TreePage, ToolFailure> {
        let mut cursor = if let Some(id) = request.cursor {
            self.tree.take(id, &request)?
        } else {
            let root = self.resolve_element(request.root, control)?;
            control.check()?;
            self.tree.begin(&self.automation, &request, &root)?
        };
        let page = self.read_page(&mut cursor, request.max_nodes, control);
        if page.as_ref().is_ok_and(|page| page.cursor.is_some()) {
            self.tree.cursors.push_back(cursor);
        } else {
            self.tree.retire(&self.automation, cursor)?;
        }
        page
    }

    fn read_page(
        &mut self,
        cursor: &mut Cursor,
        limit: usize,
        control: &RequestControl,
    ) -> Result<TreePage, ToolFailure> {
        cursor.check()?;
        let walker = cursor.view.walker(&self.automation)?;
        let mut nodes = Vec::with_capacity(limit);
        let issue = cursor
            .read_nodes(
                &self.automation,
                &walker,
                &mut self.elements,
                limit,
                control,
                &mut nodes,
            )
            .err();
        let issue = issue.or_else(|| cursor.check().err().map(|cause| cursor.issue(cause)));
        if nodes.is_empty()
            && let Some(issue) = issue
        {
            return Err(issue.cause);
        }
        let next = (issue.is_none() && cursor.pending.is_some())
            .then(|| format!("uia-tree:{}", cursor.id));
        Ok(TreePage {
            view: cursor.view,
            root: reference_token(cursor.root),
            generation: cursor.generation,
            cursor: next,
            nodes,
            issue,
        })
    }

    fn related(
        &mut self,
        id: Uuid,
        view: TreeView,
        relation: TreeRelation,
        control: &RequestControl,
    ) -> Result<Option<RootSummary>, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        if let Some(window) = entry.window {
            validate_window(window)?;
            if entry.is_root && !matches!(relation, TreeRelation::FirstChild) {
                return Ok(None);
            }
        }
        let walker = view.walker(&self.automation)?;
        control.check()?;
        let related = match relation {
            TreeRelation::Parent => walker.get_parent(&entry.element),
            TreeRelation::FirstChild => walker.get_first_child(&entry.element),
            TreeRelation::NextSibling => walker.get_next_sibling(&entry.element),
            TreeRelation::PreviousSibling => walker.get_previous_sibling(&entry.element),
        };
        let Some(element) = neighbor(related)? else {
            return Ok(None);
        };
        let new_id = Uuid::new_v4();
        let locator = match relation {
            TreeRelation::FirstChild => Some(Locator::new(id, view, 1)),
            TreeRelation::NextSibling | TreeRelation::PreviousSibling => entry.locator,
            TreeRelation::Parent => None,
        };
        let entry = Element::capture(
            &self.automation,
            element,
            entry.window,
            false,
            locator,
            control,
        )?;
        let summary = summarize_node(new_id, &entry, control)?;
        if let Some(window) = entry.window {
            validate_window(window)?;
        }
        self.elements.insert(new_id, entry)?;
        Ok(Some(summary))
    }
}

impl TreeState {
    pub(super) fn invalidate_window(&self, window: WindowId) {
        for cursor in &self.cursors {
            if cursor.window == Some(window) {
                cursor.watch.changed.store(true, Ordering::Release);
            }
        }
    }

    fn begin(
        &mut self,
        automation: &UIAutomation,
        request: &TreeRequest,
        root: &Element,
    ) -> Result<Cursor, ToolFailure> {
        if self.cursors.len() >= MAX_CURSORS
            && let Some(oldest) = self.cursors.pop_front()
        {
            self.retire(automation, oldest)?;
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| ToolFailure::new("uia_tree_generation_exhausted", "UIA 树代次已耗尽"))?;
        Ok(Cursor {
            id: Uuid::new_v4(),
            root: request.root,
            window: root.window,
            view: request.view,
            max_depth: request.max_depth,
            generation: self.generation,
            watch: StructureWatch::new(automation, &root.element)?,
            parents: Vec::new(),
            pending: Some(root.element.clone()),
        })
    }

    fn take(&mut self, id: Uuid, request: &TreeRequest) -> Result<Cursor, ToolFailure> {
        let index = self
            .cursors
            .iter()
            .position(|cursor| cursor.id == id)
            .ok_or_else(cursor_stale)?;
        let cursor = &self.cursors[index];
        if (cursor.root, cursor.view, cursor.max_depth)
            != (request.root, request.view, request.max_depth)
        {
            return Err(invalid_tree_argument("继续读取须使用相同的根、视图和深度"));
        }
        self.cursors.remove(index).ok_or_else(cursor_stale)
    }

    fn retire(&mut self, automation: &UIAutomation, cursor: Cursor) -> Result<(), ToolFailure> {
        if let Err(error) = cursor.watch.remove(automation) {
            cursor.watch.changed.store(true, Ordering::Release);
            self.cursors.push_front(cursor);
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn close(&mut self, automation: &UIAutomation) -> Result<(), ToolFailure> {
        let mut failure = None;
        while let Some(cursor) = self.cursors.pop_front() {
            if let Err(error) = cursor.watch.remove(automation) {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Cursor {
    fn check(&self) -> Result<(), ToolFailure> {
        if self.watch.changed.load(Ordering::Acquire)
            || self
                .window
                .is_some_and(|window| validate_window(window).is_err())
        {
            return Err(cursor_stale());
        }
        Ok(())
    }

    fn read_nodes(
        &mut self,
        automation: &UIAutomation,
        walker: &UITreeWalker,
        registry: &mut Registry,
        limit: usize,
        control: &RequestControl,
        nodes: &mut Vec<TreeNode>,
    ) -> Result<(), TreeIssue> {
        while nodes.len() < limit {
            control
                .check()
                .and_then(|()| self.check())
                .map_err(|cause| self.issue(cause))?;
            let Some(element) = self.pending.take() else {
                break;
            };
            let parent_id = self.parents.last().map(|parent| parent.id);
            let parent = parent_id.map(reference_token);
            let depth = self.parents.len();
            let id = if parent.is_none() {
                self.root
            } else {
                Uuid::new_v4()
            };
            let entry = if let Some(parent_id) = parent_id {
                let owner = registry.get(parent_id).map_err(|cause| self.issue(cause))?;
                Element::capture(
                    automation,
                    element.clone(),
                    owner.window,
                    false,
                    Some(Locator::new(parent_id, self.view, 1)),
                    control,
                )
                .map_err(|cause| self.issue(cause))?
            } else {
                registry.get(id).map_err(|cause| self.issue(cause))?.clone()
            };
            let summary = summarize_node(id, &entry, control).map_err(|cause| self.issue(cause))?;
            if parent.is_some() {
                registry
                    .insert(id, entry)
                    .map_err(|cause| self.issue(cause))?;
            }
            nodes.push(TreeNode {
                summary,
                parent,
                depth,
            });
            self.advance(walker, Position { id, element }, control)
                .map_err(|cause| self.issue(cause))?;
        }
        Ok(())
    }

    fn advance(
        &mut self,
        walker: &UITreeWalker,
        current: Position,
        control: &RequestControl,
    ) -> Result<(), ToolFailure> {
        self.pending = advance(
            walker,
            &mut self.parents,
            current,
            self.max_depth,
            control,
            |position| &position.element,
        )?;
        Ok(())
    }

    fn issue(&self, cause: ToolFailure) -> TreeIssue {
        TreeIssue {
            cause,
            parent: self.parents.last().map(|parent| reference_token(parent.id)),
            depth: self.parents.len(),
        }
    }
}

pub(super) fn advance<T>(
    walker: &UITreeWalker,
    parents: &mut Vec<T>,
    mut current: T,
    max_depth: usize,
    control: &RequestControl,
    element: impl Fn(&T) -> &UIElement,
) -> Result<Option<UIElement>, ToolFailure> {
    control.check()?;
    if parents.len() < max_depth
        && let Some(child) = neighbor(walker.get_first_child(element(&current)))?
    {
        parents.push(current);
        return Ok(Some(child));
    }
    while !parents.is_empty() {
        control.check()?;
        if let Some(next) = neighbor(walker.get_next_sibling(element(&current)))? {
            return Ok(Some(next));
        }
        if let Some(parent) = parents.pop() {
            current = parent;
        }
    }
    Ok(None)
}

impl StructureWatch {
    fn new(automation: &UIAutomation, root: &UIElement) -> Result<Self, ToolFailure> {
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let callback: Box<CustomStructureChangedEventHandlerFn> = Box::new(move |_, _, _| {
            flag.store(true, Ordering::Release);
            Ok(())
        });
        let handler = UIStructureChangeEventHandler::from(callback);
        let _serial = registration_guard()?;
        automation
            .add_structure_changed_event_handler(root, TreeScope::Subtree, None, &handler)
            .map_err(provider_error)?;
        Ok(Self {
            root: root.clone(),
            handler,
            changed,
        })
    }

    fn remove(&self, automation: &UIAutomation) -> Result<(), ToolFailure> {
        let _serial = registration_guard()?;
        automation
            .remove_structure_changed_event_handler(&self.root, &self.handler)
            .map_err(provider_error)
    }
}

pub(super) fn neighbor(
    result: Result<UIElement, UiaError>,
) -> Result<Option<UIElement>, ToolFailure> {
    match result {
        Ok(element) => Ok(Some(element)),
        // windows-rs turns a successful null TreeWalker neighbor into Error::empty().
        Err(error) if error.code() == 0 => Ok(None),
        Err(error) => Err(provider_error(error)),
    }
}

fn parse_cursor(token: &str) -> Result<Uuid, ToolFailure> {
    token
        .strip_prefix("uia-tree:")
        .and_then(|uuid| Uuid::parse_str(uuid).ok())
        .ok_or_else(cursor_stale)
}

fn cursor_stale() -> ToolFailure {
    ToolFailure::new(
        "uia_tree_cursor_stale",
        "UIA 树结构已变更，或继续读取游标已失效",
    )
}

fn invalid_tree_argument(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}
