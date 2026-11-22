use super::elements::{Element, parse_reference};
use super::patterns::{
    get_pattern, invalid_pattern_argument, optional_provider, pattern_result, provider,
};
use super::{RequestControl, Runtime, Worker, native, properties::format_property};
use crate::ToolFailure;
use serde_json::{Map, Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    patterns::{
        UIAnnotationPattern, UILegacyIAccessiblePattern, UIMultipleViewPattern, UIStylesPattern,
    },
    types::UIProperty,
};
use uuid::Uuid;

const MAX_VIEWS: usize = 64;
const MAX_VIEW_NAME_CHARS: usize = 256;
const MSAA_SELECTION_FLAGS: i64 = 0x1f;

const LEGACY_ATTRIBUTES: &[(&str, UIProperty)] = &[
    ("child_id", UIProperty::LegacyIAccessibleChildId),
    ("name", UIProperty::LegacyIAccessibleName),
    ("value", UIProperty::LegacyIAccessibleValue),
    ("description", UIProperty::LegacyIAccessibleDescription),
    ("role", UIProperty::LegacyIAccessibleRole),
    ("state", UIProperty::LegacyIAccessibleState),
    ("help", UIProperty::LegacyIAccessibleHelp),
    (
        "keyboard_shortcut",
        UIProperty::LegacyIAccessibleKeyboardShortcut,
    ),
    ("default_action", UIProperty::LegacyIAccessibleDefaultAction),
];
const ANNOTATION_ATTRIBUTES: &[(&str, UIProperty)] = &[
    ("type_id", UIProperty::AnnotationAnnotationTypeId),
    ("type_name", UIProperty::AnnotationAnnotationTypeName),
    ("author", UIProperty::AnnotationAuthor),
    ("date_time", UIProperty::AnnotationDateTime),
];
const STYLE_ATTRIBUTES: &[(&str, UIProperty)] = &[
    ("style_id", UIProperty::StylesStyleId),
    ("style_name", UIProperty::StylesStyleName),
    ("fill_color", UIProperty::StylesFillColor),
    ("fill_pattern_style", UIProperty::StylesFillPatternStyle),
    ("fill_pattern_color", UIProperty::StylesFillPatternColor),
    ("shape", UIProperty::StylesShape),
    ("extended_properties", UIProperty::StylesExtendedProperties),
];

pub(crate) enum MetadataAction {
    Legacy(LegacyAction),
    Views(Option<i32>),
    Annotation,
    Styles,
}

pub(crate) enum LegacyAction {
    Info,
    DefaultAction,
    SetValue(String),
    Select(i32),
}

impl MetadataAction {
    pub(crate) fn set_view(view: i64) -> Result<Self, ToolFailure> {
        let view = i32::try_from(view)
            .map_err(|_| invalid_pattern_argument("视图 ID 超出 Provider 允许的数据类型"))?;
        Ok(Self::Views(Some(view)))
    }

    pub(crate) fn select(flags: i64) -> Result<Self, ToolFailure> {
        if !(1..=MSAA_SELECTION_FLAGS).contains(&flags) {
            return Err(invalid_pattern_argument("MSAA 选择标记包含无效位"));
        }
        Ok(Self::Legacy(LegacyAction::Select(flags as i32)))
    }
}

impl Runtime {
    pub(crate) fn metadata_pattern(
        &self,
        reference: &str,
        action: MetadataAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.metadata_pattern(id, action, control)
        })
    }
}

impl Worker {
    fn metadata_pattern(
        &mut self,
        id: Uuid,
        action: MetadataAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let (pattern, operation, data) = match action {
            MetadataAction::Legacy(action) => self.legacy(id, &entry, action, control)?,
            MetadataAction::Views(view) => self.views(&entry, view, control)?,
            MetadataAction::Annotation => {
                let pattern: UIAnnotationPattern = get_pattern(&entry.element, control)?;
                let mut data = self.metadata_attributes(&entry, ANNOTATION_ATTRIBUTES, control)?;
                let target = optional_provider(control, || pattern.get_target())?;
                let target = if let Some(target) = target {
                    self.register_pattern_elements(id, &entry, vec![target], control)?
                        .pop()
                } else {
                    None
                };
                data.insert("target".into(), json!(target));
                ("Annotation", "info", Value::Object(data))
            }
            MetadataAction::Styles => {
                let _: UIStylesPattern = get_pattern(&entry.element, control)?;
                (
                    "Styles",
                    "info",
                    Value::Object(self.metadata_attributes(&entry, STYLE_ATTRIBUTES, control)?),
                )
            }
        };
        Ok(pattern_result(id, pattern, operation, data))
    }

    fn legacy(
        &mut self,
        id: Uuid,
        entry: &Element,
        action: LegacyAction,
        control: &RequestControl,
    ) -> Result<(&'static str, &'static str, Value), ToolFailure> {
        let pattern: UILegacyIAccessiblePattern = get_pattern(&entry.element, control)?;
        let operation = match action {
            LegacyAction::Info => "info",
            LegacyAction::DefaultAction => {
                provider(control, || pattern.do_default_action())?;
                "default_action"
            }
            LegacyAction::SetValue(value) => {
                provider(control, || pattern.set_value(&value))?;
                "set_value"
            }
            LegacyAction::Select(flags) => {
                provider(control, || pattern.select(flags))?;
                "select"
            }
        };
        let mut data = match self.metadata_attributes(entry, LEGACY_ATTRIBUTES, control) {
            Ok(data) => data,
            Err(error) if operation != "info" && error.code == "uia_element_unavailable" => {
                return Ok((
                    "LegacyIAccessible",
                    operation,
                    json!({"element_available": false}),
                ));
            }
            Err(error) => return Err(error),
        };
        let selected = optional_provider(control, || pattern.get_selection())?.unwrap_or_default();
        let count = selected.len();
        let selection = self.register_pattern_elements(id, entry, selected, control)?;
        data.insert("element_available".into(), json!(true));
        data.insert("selection_truncated".into(), json!(count > selection.len()));
        data.insert("selection".into(), json!(selection));
        Ok(("LegacyIAccessible", operation, Value::Object(data)))
    }

    fn views(
        &self,
        entry: &Element,
        view: Option<i32>,
        control: &RequestControl,
    ) -> Result<(&'static str, &'static str, Value), ToolFailure> {
        let pattern: UIMultipleViewPattern = get_pattern(&entry.element, control)?;
        let available = provider(control, || pattern.get_supported_views())?;
        if let Some(view) = view {
            if !available.contains(&view) {
                return Err(invalid_pattern_argument(
                    "视图 ID 不在 Provider 报告的可用集合中",
                ));
            }
            provider(control, || pattern.set_current_view(view))?;
        }
        let current = provider(control, || pattern.get_current_view())?;
        let count = available.len();
        let views = available.iter().take(MAX_VIEWS).map(|id| {
            let name = provider(control, || pattern.get_view_name(*id))?;
            Ok(json!({"id":id,"name":name.chars().take(MAX_VIEW_NAME_CHARS).collect::<String>()}))
        }).collect::<Result<Vec<_>, ToolFailure>>()?;
        Ok((
            "MultipleView",
            if view.is_some() { "set_view" } else { "info" },
            json!({"current":current, "requested":view, "views":views, "truncated":count > views.len()}),
        ))
    }

    fn metadata_attributes(
        &self,
        entry: &Element,
        fields: &[(&str, UIProperty)],
        control: &RequestControl,
    ) -> Result<Map<String, Value>, ToolFailure> {
        fields
            .iter()
            .map(|(name, property)| {
                control.check()?;
                let value = native::property(&self.automation, &entry.element, *property, false)?
                    .map_or(Ok(Value::Null), |value| format_property(*property, &value))?;
                Ok(((*name).into(), value))
            })
            .collect()
    }
}
