use super::{ScriptOutput, ScriptSession, ScriptTarget};
use crate::{
    ToolDefinition, ToolMode, ToolPayload, ToolResult, ToolSource, ToolStatus, process::arguments,
};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const DESCRIPTION: &str = concat!(
    "Execute a Rhai script against the local Windows desktop. ",
    "Variables and functions persist within the live conversation. Use print(value) or return a value. ",
    "windows() returns id, title, bounds, minimized and foreground. Use window(id) to refresh and activate(id) to focus. ",
    "capture_desktop() and capture_window(id) return image dimensions and capture bounds. When available, the last capture is attached to the result. ",
    "Use screen_at(x,y) or window_at(x,y) to convert pixels of the last capture. Window-local coordinates start at its visible frame. ",
    "Use mouse_position(), mouse_move(x,y) or mouse_move(id,x,y), mouse_down(button), mouse_up(button), click(button), ",
    "double_click(button), scroll(axis,detents), drag(button,[x1,y1],[x2,y2]), and trajectory(button,[[x,y,dt],...]). ",
    "For window-local drag or trajectory, add id as the first argument. Buttons are left, right, middle. ",
    "Scroll axes are vertical and horizontal. Each trajectory dt is milliseconds since the previous point, with zero at the first point. ",
    "Use key_down(name), key_up(name), press(name), shortcut([name,...]), type_text(text), and wait(ms). ",
    "Every uia_* function returns a map {ok,data,error,source,generation}. Read refs and other results via .data, failures via .error.code and .error.message. Pattern values are in .data.result. Snapshot generation is the top-level .generation. Use uia_status().data to probe the UIA connection. ",
    "Use uia_desktop_root() or uia_window_root(id) to get a UI Automation root summary with a session-local ref. ",
    "Use uia_root_info(ref) to re-read it. Window ids come from windows(). ",
    "uia_tree(ref) reads a bounded Control View page. Use uia_tree(ref,view,max_depth,max_nodes,cursor), with raw/control/content, depth 0..32, node limit 1..64, and cursor () for the first page. ",
    "Reuse the same root, view and depth with the returned cursor. uia_tree_related(ref,view,relation) reads parent, first_child, next_sibling, or previous_sibling. ",
    "uia_properties(ref,names) reads live properties; pass [] for common properties. uia_patterns(ref) lists provider-supported patterns. ",
    "uia_prefetch(ref,property_names,pattern_names,view,scope) prepares caches for element, children, descendants, or subtree, returning cached_refs. Use uia_cached_properties(ref,names) to read them. ",
    "uia_find(ref,conditions,view,scope,depth,node_limit,result_limit) matches Name, AutomationId, ClassName and ControlType. Combine keys with AND, use all/any arrays or not with a nested map, and set first:true only for an explicitly requested first match. ",
    "Keep element refs in Rhai variables across calls. uia_ref_info(ref) reads status and generation; uia_refresh_element(ref) verifies or uniquely rebinds a stale ref in its original scope. ",
    "Use uia_invoke(ref), uia_value_get(ref), uia_value_set(ref,text), uia_toggle(ref,change), uia_range_value_get(ref), and uia_range_value_set(ref,number). ",
    "Use uia_selection(ref) for the container, uia_select(ref,select/add/remove) for an item, and uia_expand_collapse(ref,state/expand/collapse). Patterns must be supported by the target provider. ",
    "uia_scroll(ref) reads scroll state; uia_scroll(ref,horizontal,vertical) accepts none, small_increment, small_decrement, large_increment, or large_decrement. uia_scroll_item(ref) brings an item into view. ",
    "uia_find_item(container,property,value[,after_ref]) searches ItemContainer by Name, AutomationId, or ClassName. Use uia_realize(ref) for a virtualized result. ",
    "For Text, use uia_text_document_range(ref), uia_text_selection(ref), uia_text_visible_ranges(ref), then uia_text_get(range,max_chars). uia_text_find(range,text,backward,ignore_case) returns a matching range. ",
    "Use uia_text_range_move(range,unit,count[,endpoint]), uia_text_range_expand(range,unit), uia_text_range_select(range), uia_text_range_scroll(range,align_to_top), and uia_text_range_attribute(range,name). ",
    "Read Text2 caret and annotations via uia_text_caret(ref), uia_text_annotation_range(ref,annotation), edit state via uia_text_edit_state(ref), and a TextChild range via uia_text_child_range(ref). ",
    "Use uia_grid_info(ref), uia_grid_cell(ref,row,column) with zero-based indices, and uia_grid_item_info(ref). uia_table_info(ref), uia_table_headers(ref) and uia_table_item_info(ref) report structure and headers. ",
    "Use uia_spreadsheet_info(ref[,cell_name]) for named cells and uia_spreadsheet_item_info(ref) for formula, value and annotations when supported. ",
    "Use uia_window_state(ref) and uia_window_action(ref,action) with restore, maximize, minimize, close, or wait_for_input_idle. An explicit idle timeout is in milliseconds. ",
    "uia_transform_capabilities(ref) reports move, resize, rotate, and zoom support. Use uia_transform(ref,move/resize/rotate/zoom/zoom_by_unit,args) with UIA screen coordinates, dimensions, degrees, or zoom percent. uia_dock_position(ref[,position]) reads or sets dock state. ",
    "uia_drag_info(ref) and uia_drop_target_info(ref) report provider data. uia_custom_navigation(ref,direction) navigates via that provider. uia_synchronized_input(ref,start/status/cancel[,input_type]) manages a session-owned native input notification. ",
    "uia_legacy_accessible(ref[,default_action]) reads MSAA data or invokes its default action. Use (ref,\"set_value\",value) or (ref,\"select\",flags) for other actions. uia_multiple_view(ref[,view_id]) reads or switches views. uia_annotation(ref) and uia_styles(ref) read metadata. ",
    "uia_object_model(ref) reports whether the provider exposes an underlying object model. ",
    "uia_subscribe(ref,event,scope,properties) accepts property, structure, focus, or automation:<numeric event ID>, with element/children/descendants/subtree scope (also global for focus). Only property accepts property names, e.g. ValueValue. uia_unsubscribe(id) removes a subscription. uia_events_next(timeout_ms,max_events) drains events and reports overflowed IDs. ",
    "uia_wait(ref,condition,timeout_ms) waits for a live condition. Use #{kind:\"property\",name:\"ValueValue\",equals:value}, #{kind:\"appears\",selector:uia_find_condition}, or kinds disappears (with selector), enabled, focused, focus_changed, and structure_changed. ",
    "uia_snapshot(window_ref,view,properties,max_nodes) captures up to 64 nodes. uia_snapshot_status(id) reports generation and dirty state. uia_snapshot_refresh(id,auto/full/or a snapshot node ref) refreshes the capture. ",
    "Rhai try/catch can read error.code and error.message for other desktop commands. Inputs held by the script are released when the call ends. ",
    "Each admitted script has a 30 second execution deadline."
);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    script: String,
}

pub(crate) fn definition() -> ToolDefinition {
    script_definition("computer_execute", DESCRIPTION)
}

pub(crate) fn ecs_definition() -> ToolDefinition {
    script_definition(
        "ecs_execute",
        concat!(
            "Execute Rhai against Agentelier's in-process Bevy ECS. Variables and functions persist within the live conversation. ",
            "Use ecs_world_info() to discover world statistics and supported operations. Use print(value) or return a value. ",
            "Rhai try/catch can read error.code and error.message. The script has a 30 second deadline."
        ),
    )
}

fn script_definition(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        mode: ToolMode::Synchronous,
        source: ToolSource::Builtin,
        strict: true,
        display_source: None,
        output_schema: None,
        description: description.into(),
        parameters: json!({
            "type": "object",
            "properties": {"script": {"type": "string"}},
            "required": ["script"],
            "additionalProperties": false
        }),
    }
}

pub(crate) async fn execute(
    session: &ScriptSession,
    target: ScriptTarget,
    text: &str,
    cancel: &CancellationToken,
    image_input: Option<bool>,
) -> ToolResult {
    let input: Arguments = match arguments(text, &["script"]) {
        Ok(input) => input,
        Err(error) => return ToolResult::error(ToolStatus::Failed, error.code, &error.to_string()),
    };
    match session.run(target, &input.script, cancel).await {
        Ok(output) => ToolResult {
            status: ToolStatus::Succeeded,
            output: payload(output, image_input),
            cleanup_failed: false,
        },
        Err(error) => {
            let status = match error.code {
                "cancelled" | "uia_cancelled" => ToolStatus::Cancelled,
                "timeout" | "uia_timeout" => ToolStatus::TimedOut,
                "desktop_unconfirmed" => ToolStatus::NotExecuted,
                "desktop_input_cleanup" | "script_worker_failed" | "uia_worker_failed" => {
                    ToolStatus::Unconfirmed
                }
                _ => ToolStatus::Failed,
            };
            let mut result = ToolResult::error(status, error.code, &error.to_string());
            result.cleanup_failed = status == ToolStatus::Unconfirmed;
            result
        }
    }
}

fn payload(output: ScriptOutput, image_input: Option<bool>) -> ToolPayload {
    let ScriptOutput { text, image } = output;
    match image {
        Some(_) if image_input == Some(false) => json!({
            "ok": true,
            "data": {"text": text, "capture_attachment": "model_does_not_accept_images"}
        })
        .to_string()
        .into(),
        Some(image) => ToolPayload::Image {
            summary: json!({
                "ok": true,
                "data": {"text": text, "capture": image.descriptor()}
            })
            .to_string(),
            image,
        },
        None => json!({"ok":true,"data":{"text":text}}).to_string().into(),
    }
}
