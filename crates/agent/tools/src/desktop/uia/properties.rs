use super::elements::{Element, parse_reference, reference_token, validate_window};
use super::references::Locator;
use super::tree::{TreeView, neighbor};
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::ToolFailure;
use serde_json::{Map, Value as Json, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIAutomation, UIElement,
    core::UICacheRequest,
    patterns::UIPatternType,
    types::{ControlType, ElementMode, TreeScope, UIProperty},
    variants::{Value, Variant},
};
use uuid::Uuid;
use windows::{Win32::UI::Accessibility::IUIAutomationElement, core::Interface};

const MAX_CACHE_TARGETS: usize = 32;
const MAX_CACHE_DEPTH: usize = 32;
const MAX_PROPERTY_CHARS: usize = 512;
const MAX_ARRAY_ITEMS: usize = 64;

#[derive(Clone, Copy)]
pub(super) struct Property {
    pub(super) name: &'static str,
    pub(super) id: UIProperty,
}

macro_rules! property_catalog {
    ($($id:ident),+ $(,)?) => {
        [$(Property { name: stringify!($id), id: UIProperty::$id }),+]
    };
}

const COMMON_PROPERTIES: [Property; 11] = property_catalog![
    Name,
    AutomationId,
    ControlType,
    ClassName,
    BoundingRectangle,
    IsEnabled,
    IsOffscreen,
    HasKeyboardFocus,
    IsKeyboardFocusable,
    FrameworkId,
    RuntimeId,
];
const EXTENDED_PROPERTIES: [Property; 17] = property_catalog![
    ProcessId,
    NativeWindowHandle,
    LocalizedControlType,
    HelpText,
    AccessKey,
    AcceleratorKey,
    IsPassword,
    IsControlElement,
    IsContentElement,
    ItemStatus,
    ItemType,
    LabeledBy,
    ProviderDescription,
    HeadingLevel,
    IsDialog,
    IsRequiredForForm,
    ValueValue,
];

#[derive(Clone, Copy)]
struct Pattern {
    name: &'static str,
    id: UIPatternType,
    available: UIProperty,
}

macro_rules! pattern_catalog {
    ($($id:ident => $available:ident),+ $(,)?) => {
        [$(Pattern { name: stringify!($id), id: UIPatternType::$id, available: UIProperty::$available }),+]
    };
}

const PATTERNS: [Pattern; 34] = pattern_catalog![
    Invoke => IsInvokePatternAvailable,
    Selection => IsSelectionPatternAvailable,
    Value => IsValuePatternAvailable,
    RangeValue => IsRangeValuePatternAvailable,
    Scroll => IsScrollPatternAvailable,
    ExpandCollapse => IsExpandCollapsePatternAvailable,
    Grid => IsGridPatternAvailable,
    GridItem => IsGridItemPatternAvailable,
    MultipleView => IsMultipleViewPatternAvailable,
    Window => IsWindowPatternAvailable,
    SelectionItem => IsSelectionItemPatternAvailable,
    Dock => IsDockPatternAvailable,
    Table => IsTablePatternAvailable,
    TableItem => IsTableItemPatternAvailable,
    Text => IsTextPatternAvailable,
    Toggle => IsTogglePatternAvailable,
    Transform => IsTransformPatternAvailable,
    ScrollItem => IsScrollItemPatternAvailable,
    LegacyIAccessible => IsLegacyIAccessiblePatternAvailable,
    ItemContainer => IsItemContainerPatternAvailable,
    VirtualizedItem => IsVirtualizedItemPatternAvailable,
    SynchronizedInput => IsSynchronizedInputPatternAvailable,
    ObjectModel => IsObjectModelPatternAvailable,
    Annotation => IsAnnotationPatternAvailable,
    Text2 => IsTextPattern2Available,
    Styles => IsStylesPatternAvailable,
    Spreadsheet => IsSpreadsheetPatternAvailable,
    SpreadsheetItem => IsSpreadsheetItemPatternAvailable,
    Transform2 => IsTransformPattern2Available,
    TextChild => IsTextChildPatternAvailable,
    Drag => IsDragPatternAvailable,
    DropTarget => IsDropTargetPatternAvailable,
    TextEdit => IsTextEditPatternAvailable,
    CustomNavigation => IsCustomNavigationPatternAvailable,
];

#[derive(Clone)]
pub(super) struct CachedElement {
    pub(super) element: UIElement,
    prepared: Vec<Property>,
}

pub(crate) struct CacheRequest {
    view: TreeView,
    scope: CacheScope,
    properties: Vec<Property>,
    patterns: Vec<Pattern>,
}

#[derive(Clone, Copy)]
enum CacheScope {
    Element,
    Children,
    Descendants,
    Subtree,
}

impl CacheRequest {
    pub(crate) fn new(
        properties: &[String],
        patterns: &[String],
        view: &str,
        scope: &str,
    ) -> Result<Self, ToolFailure> {
        let scope = match scope {
            "element" => CacheScope::Element,
            "children" => CacheScope::Children,
            "descendants" => CacheScope::Descendants,
            "subtree" => CacheScope::Subtree,
            _ => {
                return Err(invalid_property_argument(
                    "缓存范围须为 element、children、descendants 或 subtree",
                ));
            }
        };
        Ok(Self {
            view: view.parse()?,
            scope,
            properties: parse_properties(properties)?,
            patterns: parse_patterns(patterns)?,
        })
    }

    fn build(&self, automation: &UIAutomation) -> Result<UICacheRequest, ToolFailure> {
        let request = element_cache(automation, self.view)?;
        for prop in &self.properties {
            request.add_property(prop.id).map_err(provider_error)?;
        }
        for pattern in &self.patterns {
            request.add_pattern(pattern.id).map_err(provider_error)?;
        }
        Ok(request)
    }
}

impl CacheScope {
    fn name(self) -> &'static str {
        match self {
            Self::Element => "element",
            Self::Children => "children",
            Self::Descendants => "descendants",
            Self::Subtree => "subtree",
        }
    }

    fn include_root(self) -> bool {
        matches!(self, Self::Element | Self::Subtree)
    }

    fn visits_children(self, depth: usize) -> bool {
        match self {
            Self::Element => false,
            Self::Children => depth == 0,
            Self::Descendants | Self::Subtree => true,
        }
    }
}

impl Runtime {
    pub(crate) fn properties(
        &self,
        reference: &str,
        names: &[String],
        cached: bool,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Json, ToolFailure> {
        let id = parse_reference(reference)?;
        let properties = parse_properties(names)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.read_properties(id, &properties, cached, control)
        })
    }

    pub(crate) fn prefetch(
        &self,
        reference: &str,
        request: CacheRequest,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Json, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.prefetch(id, &request, control)
        })
    }

    pub(crate) fn patterns(
        &self,
        reference: &str,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Json, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.read_patterns(id, control)
        })
    }
}

impl Worker {
    fn read_properties(
        &mut self,
        id: Uuid,
        properties: &[Property],
        cached: bool,
        control: &RequestControl,
    ) -> Result<Json, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let element = if cached {
            let cache = entry
                .cache
                .as_ref()
                .ok_or_else(|| ToolFailure::new("uia_cache_unavailable", "该元素尚未预取缓存"))?;
            if let Some(property) = properties
                .iter()
                .find(|p| !cache.prepared.iter().any(|cached| cached.id == p.id))
            {
                return Err(ToolFailure::new(
                    "uia_cache_unavailable",
                    format!("属性 {} 未预取", property.name),
                ));
            }
            &cache.element
        } else {
            &entry.element
        };
        let fields = properties
            .iter()
            .map(|property| {
                control.check()?;
                let value = native::property(&self.automation, element, property.id, cached)?
                    .map_or(Ok(Json::Null), |value| format_property(property.id, &value))?;
                Ok((property.name.into(), value))
            })
            .collect::<Result<Map<String, Json>, ToolFailure>>()?;
        if let Some(window) = entry.window {
            validate_window(window)?;
        }
        Ok(json!({
            "ref": reference_token(id),
            "source": if cached { "cache" } else { "live" },
            "properties": fields,
        }))
    }

    fn prefetch(
        &mut self,
        id: Uuid,
        request: &CacheRequest,
        control: &RequestControl,
    ) -> Result<Json, ToolFailure> {
        let root = self.resolve_element(id, control)?;
        let window = root.window;
        let handle = request.build(&self.automation)?;
        let (targets, truncated) =
            cache_targets(&self.automation, &root.element, request, control)?;
        let include_root = request.scope.include_root();
        self.elements
            .reserve(targets.len().saturating_sub(usize::from(include_root)))?;
        let mut prepared = Vec::with_capacity(targets.len());
        for target in targets {
            control.check()?;
            let element = target
                .build_updated_cache(&handle)
                .map_err(provider_error)?;
            prepared.push((
                target,
                CachedElement {
                    element,
                    prepared: request.properties.clone(),
                },
            ));
        }
        control.check()?;
        if let Some(window) = window {
            validate_window(window)?;
        }
        let mut refs = Vec::with_capacity(prepared.len());
        for (index, (target, cache)) in prepared.into_iter().enumerate() {
            if include_root && index == 0 {
                self.elements.get_mut(id)?.cache = Some(cache);
                refs.push(reference_token(id));
            } else {
                let target_id = Uuid::new_v4();
                let depth = if matches!(request.scope, CacheScope::Children) {
                    1
                } else {
                    MAX_CACHE_DEPTH
                };
                let mut entry = Element::capture(
                    &self.automation,
                    target,
                    window,
                    false,
                    Some(Locator::new(id, request.view, depth)),
                    control,
                )?;
                entry.cache = Some(cache);
                self.elements.insert(target_id, entry)?;
                refs.push(reference_token(target_id));
            }
        }
        Ok(json!({
            "ref": reference_token(id), "source": "cache", "scope": request.scope.name(),
            "cached_refs": refs, "truncated": truncated,
            "properties": request.properties.iter().map(|prop| prop.name).collect::<Vec<_>>(),
            "patterns": request.patterns.iter().map(|pattern| pattern.name).collect::<Vec<_>>(),
        }))
    }

    pub(super) fn read_patterns(
        &mut self,
        id: Uuid,
        control: &RequestControl,
    ) -> Result<Json, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let request = element_cache(&self.automation, TreeView::Raw)?;
        for pattern in PATTERNS {
            request
                .add_property(pattern.available)
                .map_err(provider_error)?;
        }
        control.check()?;
        let cached = entry
            .element
            .build_updated_cache(&request)
            .map_err(provider_error)?;
        let mut available = Vec::new();
        for pattern in PATTERNS {
            control.check()?;
            let Some(value) = native::property(&self.automation, &cached, pattern.available, true)?
            else {
                continue;
            };
            match value.get_value().map_err(provider_error)? {
                Value::BOOL(true) => available.push(pattern.name),
                Value::BOOL(false) => {}
                _ => {
                    return Err(ToolFailure::new(
                        "uia_property_type",
                        "UIA Pattern 可用性属性类型无效",
                    ));
                }
            }
        }
        if let Some(window) = entry.window {
            validate_window(window)?;
        }
        Ok(json!({"ref": reference_token(id), "source": "live", "patterns": available}))
    }
}

fn element_cache(automation: &UIAutomation, view: TreeView) -> Result<UICacheRequest, ToolFailure> {
    let request = automation.create_cache_request().map_err(provider_error)?;
    request
        .set_tree_scope(TreeScope::Element)
        .map_err(provider_error)?;
    request
        .set_element_mode(ElementMode::Full)
        .map_err(provider_error)?;
    request
        .set_tree_filter(view.filter(automation)?)
        .map_err(provider_error)?;
    Ok(request)
}

fn cache_targets(
    automation: &UIAutomation,
    root: &UIElement,
    request: &CacheRequest,
    control: &RequestControl,
) -> Result<(Vec<UIElement>, bool), ToolFailure> {
    if matches!(request.scope, CacheScope::Element) {
        return Ok((vec![root.clone()], false));
    }
    let walker = request.view.walker(automation)?;
    let skip = usize::from(!request.scope.include_root());
    let limit = MAX_CACHE_TARGETS + skip;
    let mut nodes = vec![(root.clone(), 0)];
    let mut position = 0;
    let mut depth_truncated = false;
    while position < nodes.len() && nodes.len() <= limit {
        let (node, depth) = nodes[position].clone();
        position += 1;
        if !request.scope.visits_children(depth) {
            continue;
        }
        control.check()?;
        let mut child = neighbor(walker.get_first_child(&node))?;
        if depth >= MAX_CACHE_DEPTH {
            depth_truncated |= child.is_some();
            continue;
        }
        while let Some(element) = child {
            nodes.push((element.clone(), depth + 1));
            if nodes.len() > limit {
                break;
            }
            control.check()?;
            child = neighbor(walker.get_next_sibling(&element))?;
        }
    }
    let truncated = depth_truncated || nodes.len() > limit;
    Ok((
        nodes
            .into_iter()
            .skip(skip)
            .take(MAX_CACHE_TARGETS)
            .map(|(node, _)| node)
            .collect(),
        truncated,
    ))
}

pub(super) fn parse_properties(names: &[String]) -> Result<Vec<Property>, ToolFailure> {
    if names.is_empty() {
        return Ok(COMMON_PROPERTIES.to_vec());
    }
    if names.len() > COMMON_PROPERTIES.len() + EXTENDED_PROPERTIES.len() {
        return Err(invalid_property_argument("请求的属性数量超过上限"));
    }
    let mut properties = Vec::with_capacity(names.len());
    for name in names {
        let property = COMMON_PROPERTIES
            .iter()
            .chain(EXTENDED_PROPERTIES.iter())
            .find(|property| property.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| invalid_property_argument(format!("未知的 UIA 属性 {name}")))?;
        if !properties
            .iter()
            .any(|existing: &Property| existing.id == property.id)
        {
            properties.push(*property);
        }
    }
    Ok(properties)
}

fn parse_patterns(names: &[String]) -> Result<Vec<Pattern>, ToolFailure> {
    if names.len() > PATTERNS.len() {
        return Err(invalid_property_argument("请求的 Pattern 数量超过上限"));
    }
    let mut patterns = Vec::with_capacity(names.len());
    for name in names {
        let pattern = PATTERNS
            .iter()
            .find(|pattern| pattern.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| invalid_property_argument(format!("未知的 UIA Pattern {name}")))?;
        if !patterns
            .iter()
            .any(|existing: &Pattern| existing.id == pattern.id)
        {
            patterns.push(*pattern);
        }
    }
    Ok(patterns)
}

pub(super) fn format_property(
    property: UIProperty,
    variant: &Variant,
) -> Result<Json, ToolFailure> {
    let value = variant.get_value().map_err(provider_error)?;
    let result = match value {
        Value::EMPTY | Value::NULL | Value::VOID => Json::Null,
        Value::BOOL(value) => json!(value),
        Value::STRING(value) => json!(bounded_text(&value)),
        Value::I2(value) => json!(value),
        Value::I4(value) | Value::INT(value) if property == UIProperty::ControlType => {
            let kind = ControlType::try_from(value).map_err(provider_error)?;
            json!(format!("{kind:?}"))
        }
        Value::I4(value) | Value::INT(value) => json!(value),
        Value::I8(value) => json!(value),
        Value::UI2(value) => json!(value),
        Value::UI4(value) | Value::UINT(value) => json!(value),
        Value::UI8(value) => json!(value.to_string()),
        Value::R4(value) => finite(value.into()),
        Value::R8(value) => finite(value),
        Value::ArrayI4(values) => {
            json!(values.into_iter().take(MAX_ARRAY_ITEMS).collect::<Vec<_>>())
        }
        Value::ArrayR8(values) if property == UIProperty::BoundingRectangle => {
            let [left, top, width, height] = values.as_slice() else {
                return Err(ToolFailure::new(
                    "uia_property_type",
                    "UIA 边界需要四个坐标值",
                ));
            };
            json!({"left":finite(*left), "top":finite(*top), "width":finite(*width), "height":finite(*height)})
        }
        Value::UNKNOWN(value) if property == UIProperty::LabeledBy => {
            let label = value
                .cast::<IUIAutomationElement>()
                .map(UIElement::from)
                .map_err(|error| provider_error(error.into()))?;
            json!({"name": bounded_text(&label.get_name().map_err(provider_error)?) })
        }
        _ => {
            return Err(ToolFailure::new(
                "uia_property_type",
                format!("UIA 属性 {property:?} 的值类型无法转换"),
            ));
        }
    };
    Ok(result)
}

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_PROPERTY_CHARS).collect()
}

fn finite(value: f64) -> Json {
    if value.is_finite() {
        json!(value)
    } else {
        json!(value.to_string())
    }
}

fn invalid_property_argument(message: impl Into<String>) -> ToolFailure {
    ToolFailure::new("uia_invalid_argument", message)
}
