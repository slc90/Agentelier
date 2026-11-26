use serde_json::{Map, Value};
use std::borrow::Cow;

const COMPACTION_THRESHOLD: usize = 5_000;
const MAX_NESTED_SCHEMA_DEPTH: usize = 3;
const DEFINITION_KEYS: [&str; 2] = ["$defs", "definitions"];
const COMPOSITION_KEYS: [&str; 3] = ["allOf", "anyOf", "oneOf"];

pub(super) fn compact(schema: &Value) -> Cow<'_, Value> {
    if schema.to_string().len() <= COMPACTION_THRESHOLD {
        return Cow::Borrowed(schema);
    }
    let mut schema = schema.clone();
    visit_schemas(&mut schema, 0, &mut |node, _| {
        node.remove("description");
    });
    if schema.to_string().len() > COMPACTION_THRESHOLD {
        visit_schemas(&mut schema, 0, &mut |node, depth| {
            let local_definition =
                node.get("$ref")
                    .and_then(Value::as_str)
                    .is_some_and(|reference| {
                        reference.starts_with("#/$defs/") || reference.starts_with("#/definitions/")
                    });
            if local_definition && depth > 0 {
                node.clear();
            }
        });
        if schema.get("$ref").is_none()
            && let Some(root) = schema.as_object_mut()
        {
            for key in DEFINITION_KEYS {
                root.remove(key);
            }
        }
    }
    if schema.to_string().len() > COMPACTION_THRESHOLD {
        visit_schemas(&mut schema, 0, &mut |node, depth| {
            if depth >= MAX_NESTED_SCHEMA_DEPTH
                && [
                    "properties",
                    "items",
                    "additionalProperties",
                    "$ref",
                    "oneOf",
                    "anyOf",
                    "allOf",
                ]
                .iter()
                .any(|key| node.contains_key(*key))
            {
                node.clear();
            }
        });
    }
    if schema.to_string().len() > COMPACTION_THRESHOLD {
        visit_schemas(&mut schema, 0, &mut |node, depth| {
            if depth > 0 && COMPOSITION_KEYS.iter().any(|key| node.contains_key(*key)) {
                node.clear();
            }
        });
    }
    Cow::Owned(schema)
}

fn visit_schemas(
    value: &mut Value,
    depth: usize,
    visit: &mut impl FnMut(&mut Map<String, Value>, usize),
) {
    match value {
        Value::Object(node) => {
            visit(node, depth);
            for key in [
                "properties",
                "patternProperties",
                "dependentSchemas",
                "$defs",
                "definitions",
            ] {
                if let Some(children) = node.get_mut(key).and_then(Value::as_object_mut) {
                    for child in children.values_mut() {
                        visit_schemas(child, depth + 1, visit);
                    }
                }
            }
            for key in [
                "items",
                "prefixItems",
                "additionalProperties",
                "unevaluatedProperties",
                "unevaluatedItems",
                "contains",
                "propertyNames",
                "if",
                "then",
                "else",
                "not",
                "allOf",
                "anyOf",
                "oneOf",
            ] {
                if let Some(child) = node.get_mut(key) {
                    visit_schemas(child, depth + 1, visit);
                }
            }
        }
        Value::Array(children) => {
            for child in children {
                visit_schemas(child, depth, visit);
            }
        }
        _ => {}
    }
}
