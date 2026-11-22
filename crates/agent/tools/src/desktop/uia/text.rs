use super::elements::{Element, parse_reference};
use super::patterns::{
    SelectionChange, get_pattern, invalid_pattern_argument, optional_provider as optional,
    pattern_result, provider,
};
use super::{RequestControl, Runtime, Worker, native, provider_error};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    errors::Error as UiaError,
    patterns::{UITextChildPattern, UITextEditPattern, UITextPattern, UITextRange},
    types::{TextAttribute, TextPatternRangeEndpoint, TextUnit},
    variants::Value as VariantValue,
};
use uuid::Uuid;
use windows::Win32::Foundation::E_NOINTERFACE;

const MAX_STORED_RANGES: usize = 256;
const MAX_RESULT_RANGES: usize = 32;
const MAX_TEXT_CHARS: i64 = 4096;

pub(crate) enum TextAction {
    Document,
    Selection,
    Visible,
    Caret,
    Annotation(String),
    Edit,
    Child,
}

pub(crate) enum TextRangeAction {
    Read(usize),
    Find {
        text: String,
        backward: bool,
        ignore_case: bool,
    },
    Move {
        unit: TextUnit,
        amount: i32,
        endpoint: Option<TextPatternRangeEndpoint>,
    },
    Expand(TextUnit),
    Select(SelectionChange),
    Scroll(bool),
    Attribute(TextAttribute),
}

impl TextRangeAction {
    pub(crate) fn read(max_chars: i64) -> Result<Self, ToolFailure> {
        if !(1..=MAX_TEXT_CHARS).contains(&max_chars) {
            return Err(invalid_pattern_argument(format!(
                "文本字符预算须在 1..={MAX_TEXT_CHARS}"
            )));
        }
        Ok(Self::Read(max_chars as usize))
    }

    pub(crate) fn find(
        text: String,
        backward: bool,
        ignore_case: bool,
    ) -> Result<Self, ToolFailure> {
        if text.is_empty() || text.chars().nth(MAX_TEXT_CHARS as usize).is_some() {
            return Err(invalid_pattern_argument("查找文本为空或超出字符预算"));
        }
        Ok(Self::Find {
            text,
            backward,
            ignore_case,
        })
    }

    pub(crate) fn movement(
        unit: &str,
        amount: i64,
        endpoint: Option<&str>,
    ) -> Result<Self, ToolFailure> {
        if !(-MAX_TEXT_CHARS..=MAX_TEXT_CHARS).contains(&amount) {
            return Err(invalid_pattern_argument("文本范围移动量越界"));
        }
        let endpoint = endpoint
            .map(|name| match name {
                "start" => Ok(TextPatternRangeEndpoint::Start),
                "end" => Ok(TextPatternRangeEndpoint::End),
                _ => Err(invalid_pattern_argument("文本端点须为 start 或 end")),
            })
            .transpose()?;
        Ok(Self::Move {
            unit: text_unit(unit)?,
            amount: amount as i32,
            endpoint,
        })
    }

    pub(crate) fn expand(unit: &str) -> Result<Self, ToolFailure> {
        Ok(Self::Expand(text_unit(unit)?))
    }

    pub(crate) fn attribute(name: &str) -> Result<Self, ToolFailure> {
        let attribute = match name.to_ascii_lowercase().as_str() {
            "fontname" => TextAttribute::FontName,
            "fontsize" => TextAttribute::FontSize,
            "fontweight" => TextAttribute::FontWeight,
            "isreadonly" => TextAttribute::IsReadOnly,
            "ishidden" => TextAttribute::IsHidden,
            "isitalic" => TextAttribute::IsItalic,
            "culture" => TextAttribute::Culture,
            "foregroundcolor" => TextAttribute::ForegroundColor,
            "backgroundcolor" => TextAttribute::BackgroundColor,
            "underlinestyle" => TextAttribute::UnderlineStyle,
            "stylename" => TextAttribute::StyleName,
            _ => return Err(invalid_pattern_argument("未知的 UIA 文本属性")),
        };
        Ok(Self::Attribute(attribute))
    }
}

#[derive(Default)]
pub(super) struct TextRanges {
    entries: HashMap<Uuid, RangeEntry>,
}

#[derive(Clone)]
struct RangeEntry {
    range: UITextRange,
    owner: Uuid,
    generation: i64,
}

impl TextRanges {
    fn store(&mut self, entry: RangeEntry) -> Result<String, ToolFailure> {
        if self.entries.len() >= MAX_STORED_RANGES {
            return Err(ToolFailure::new(
                "uia_text_range_limit",
                "UIA 文本范围引用已达上限",
            ));
        }
        let id = Uuid::new_v4();
        self.entries.insert(id, entry);
        Ok(format!("uia-text:{id}"))
    }

    fn store_all(
        &mut self,
        owner: Uuid,
        generation: i64,
        ranges: Vec<UITextRange>,
    ) -> Result<Value, ToolFailure> {
        let count = ranges.len();
        let retained = count.min(MAX_RESULT_RANGES);
        if retained > MAX_STORED_RANGES.saturating_sub(self.entries.len()) {
            return Err(ToolFailure::new(
                "uia_text_range_limit",
                "UIA 文本范围引用已达上限",
            ));
        }
        let ranges = ranges
            .into_iter()
            .take(retained)
            .map(|range| {
                self.store(RangeEntry {
                    range,
                    owner,
                    generation,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"ranges": ranges, "count": count, "truncated": count > retained}))
    }
}

impl Runtime {
    pub(crate) fn text_action(
        &self,
        reference: &str,
        action: TextAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.text_action(id, action, control)
        })
    }

    pub(crate) fn text_range_action(
        &self,
        reference: &str,
        action: TextRangeAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = reference
            .strip_prefix("uia-text:")
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(range_stale)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.text_range_action(id, reference_token(id), action, control)
        })
    }
}

impl Worker {
    fn text_action(
        &mut self,
        id: Uuid,
        action: TextAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let generation = entry.generation;
        let (pattern, operation, data) = match action {
            TextAction::Document => {
                let pattern: UITextPattern = get_pattern(&entry.element, control)?;
                let range = provider(control, || pattern.get_document_range())?;
                let range = self.text_ranges.store(RangeEntry {
                    range,
                    owner: id,
                    generation,
                })?;
                ("Text", "document_range", json!({"range": range}))
            }
            TextAction::Selection | TextAction::Visible => {
                let pattern: UITextPattern = get_pattern(&entry.element, control)?;
                let (operation, ranges) = if matches!(action, TextAction::Selection) {
                    (
                        "selection",
                        optional(control, || pattern.get_selection())?.unwrap_or_default(),
                    )
                } else {
                    (
                        "visible_ranges",
                        optional(control, || pattern.get_visible_ranges())?.unwrap_or_default(),
                    )
                };
                (
                    "Text",
                    operation,
                    self.text_ranges.store_all(id, generation, ranges)?,
                )
            }
            TextAction::Caret => {
                let pattern: UITextPattern = get_pattern(&entry.element, control)?;
                let (active, range) = text2(control, || pattern.get_caret_range())?;
                let range = self.text_ranges.store(RangeEntry {
                    range,
                    owner: id,
                    generation,
                })?;
                (
                    "Text2",
                    "caret_range",
                    json!({"active": active, "range": range}),
                )
            }
            TextAction::Annotation(annotation) => {
                let annotation = self.resolve_element(parse_reference(&annotation)?, control)?;
                if entry.window != annotation.window {
                    return Err(invalid_pattern_argument("注释元素属于其他窗口"));
                }
                let pattern: UITextPattern = get_pattern(&entry.element, control)?;
                let range = text2(control, || {
                    pattern.get_range_from_annotation(&annotation.element)
                })?;
                let range = self.text_ranges.store(RangeEntry {
                    range,
                    owner: id,
                    generation,
                })?;
                ("Text2", "annotation_range", json!({"range": range}))
            }
            TextAction::Edit => {
                let pattern: UITextEditPattern = get_pattern(&entry.element, control)?;
                let composition = optional(control, || pattern.get_active_composition())?;
                let conversion = optional(control, || pattern.get_conversion_target())?;
                let wrap = |range| RangeEntry {
                    range,
                    owner: id,
                    generation,
                };
                let composition = composition
                    .map(|range| self.text_ranges.store(wrap(range)))
                    .transpose()?;
                let conversion = conversion
                    .map(|range| self.text_ranges.store(wrap(range)))
                    .transpose()?;
                (
                    "TextEdit",
                    "edit_state",
                    json!({"active_composition": composition, "conversion_target": conversion}),
                )
            }
            TextAction::Child => self.text_child(id, &entry, control)?,
        };
        Ok(pattern_result(id, pattern, operation, data))
    }

    fn text_child(
        &mut self,
        id: Uuid,
        entry: &Element,
        control: &RequestControl,
    ) -> Result<(&'static str, &'static str, Value), ToolFailure> {
        let pattern: UITextChildPattern = get_pattern(&entry.element, control)?;
        let container = provider(control, || pattern.get_text_container())?;
        let range = provider(control, || pattern.get_text_range())?;
        let mut summary = self.register_pattern_elements(id, entry, vec![container], control)?;
        let container = summary.pop().ok_or_else(range_stale)?;
        let range = self.text_ranges.store(RangeEntry {
            range,
            owner: id,
            generation: entry.generation,
        })?;
        Ok((
            "TextChild",
            "child_range",
            json!({"range": range, "container": container}),
        ))
    }

    fn text_range_action(
        &mut self,
        id: Uuid,
        range_ref: String,
        action: TextRangeAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self
            .text_ranges
            .entries
            .get(&id)
            .cloned()
            .ok_or_else(range_stale)?;
        let owner = self.resolve_element(entry.owner, control)?;
        if owner.generation != entry.generation {
            self.text_ranges.entries.remove(&id);
            return Err(range_stale());
        }
        let range = &entry.range;
        let (operation, mut data) = match action {
            TextRangeAction::Read(limit) => {
                let text = provider(control, || range.get_text((limit * 2 + 1) as i32))?;
                let mut chars = text.chars();
                let text: String = chars.by_ref().take(limit).collect();
                (
                    "get_text",
                    json!({"text": text, "truncated": chars.next().is_some()}),
                )
            }
            TextRangeAction::Find {
                text,
                backward,
                ignore_case,
            } => {
                let range = optional(control, || range.find_text(&text, backward, ignore_case))?
                    .ok_or_else(|| {
                        ToolFailure::new("uia_text_not_found", "文本范围中没有匹配内容")
                    })?;
                let found = self.text_ranges.store(RangeEntry {
                    range,
                    ..entry.clone()
                })?;
                ("find_text", json!({"match": found}))
            }
            TextRangeAction::Move {
                unit,
                amount,
                endpoint,
            } => {
                let moved = if let Some(endpoint) = endpoint {
                    provider(control, || {
                        range.move_endpoint_by_unit(endpoint, unit, amount)
                    })?
                } else {
                    provider(control, || range.move_text(unit, amount))?
                };
                (
                    "move",
                    json!({"moved": moved, "unit": format!("{unit:?}"), "endpoint": endpoint.map(|e| format!("{e:?}"))}),
                )
            }
            TextRangeAction::Expand(unit) => {
                provider(control, || range.expand_to_enclosing_unit(unit))?;
                ("expand", json!({"unit": format!("{unit:?}")}))
            }
            TextRangeAction::Select(change) => {
                let operation = match change {
                    SelectionChange::Select => {
                        provider(control, || range.select())?;
                        "select"
                    }
                    SelectionChange::Add => {
                        provider(control, || range.add_to_selection())?;
                        "add_to_selection"
                    }
                    SelectionChange::Remove => {
                        provider(control, || range.remove_from_selection())?;
                        "remove_from_selection"
                    }
                };
                (operation, json!({}))
            }
            TextRangeAction::Scroll(align) => {
                provider(control, || range.scroll_into_view(align))?;
                ("scroll_into_view", json!({"align_to_top": align}))
            }
            TextRangeAction::Attribute(attribute) => {
                let value = provider(control, || range.get_attribute_value(attribute))?;
                let (status, value) = if native::property_not_supported(&self.automation, &value)? {
                    ("unsupported", Value::Null)
                } else {
                    text_attribute(value.get_value().map_err(provider_error)?)?
                };
                (
                    "attribute",
                    json!({"attribute": format!("{attribute:?}"), "status": status, "value": value}),
                )
            }
        };
        data["range"] = Value::String(range_ref);
        Ok(pattern_result(entry.owner, "TextRange", operation, data))
    }
}

fn text2<T>(
    control: &RequestControl,
    operation: impl FnOnce() -> Result<T, UiaError>,
) -> Result<T, ToolFailure> {
    control.check()?;
    let result = operation();
    control.check()?;
    result.map_err(|error| {
        if error.result() == Some(E_NOINTERFACE) {
            ToolFailure::new("uia_pattern_not_supported", "目标元素不支持 Text2 Pattern")
        } else {
            provider_error(error)
        }
    })
}

fn text_attribute(value: VariantValue) -> Result<(&'static str, Value), ToolFailure> {
    let value = match value {
        VariantValue::EMPTY | VariantValue::NULL => return Ok(("missing", Value::Null)),
        VariantValue::UNKNOWN(_) => return Ok(("mixed", Value::Null)),
        VariantValue::BOOL(value) => json!(value),
        VariantValue::I4(value) | VariantValue::INT(value) => json!(value),
        VariantValue::R8(value) => json!(value),
        VariantValue::STRING(value) => json!(
            value
                .chars()
                .take(MAX_TEXT_CHARS as usize)
                .collect::<String>()
        ),
        _ => {
            return Err(ToolFailure::new(
                "uia_property_type",
                "无法以普通数据表示此文本属性",
            ));
        }
    };
    Ok(("available", value))
}

fn text_unit(value: &str) -> Result<TextUnit, ToolFailure> {
    match value {
        "character" => Ok(TextUnit::Character),
        "format" => Ok(TextUnit::Format),
        "word" => Ok(TextUnit::Word),
        "line" => Ok(TextUnit::Line),
        "paragraph" => Ok(TextUnit::Paragraph),
        "page" => Ok(TextUnit::Page),
        "document" => Ok(TextUnit::Document),
        _ => Err(invalid_pattern_argument("未知的 UIA 文本单位")),
    }
}

fn range_stale() -> ToolFailure {
    ToolFailure::new("uia_text_range_stale", "UIA 文本范围已失效或不属于当前会话")
}

fn reference_token(id: Uuid) -> String {
    format!("uia-text:{id}")
}
