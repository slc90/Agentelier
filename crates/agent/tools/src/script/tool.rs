use super::{ScriptOutput, ScriptSession};
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
    "Use uia_status() to check whether the session can connect to Windows UI Automation. ",
    "Rhai try/catch can read error.code and error.message. Inputs held by the script are released when the call ends. ",
    "Each admitted script has a 30 second execution deadline."
);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    script: String,
}

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "computer_execute".into(),
        mode: ToolMode::Synchronous,
        source: ToolSource::Builtin,
        strict: true,
        display_source: None,
        output_schema: None,
        description: DESCRIPTION.into(),
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
    text: &str,
    cancel: &CancellationToken,
    image_input: Option<bool>,
) -> ToolResult {
    let input: Arguments = match arguments(text, &["script"]) {
        Ok(input) => input,
        Err(error) => return ToolResult::error(ToolStatus::Failed, error.code, &error.to_string()),
    };
    match session.run(&input.script, cancel).await {
        Ok(output) => ToolResult {
            status: ToolStatus::Succeeded,
            output: payload(output, image_input),
            cleanup_failed: false,
        },
        Err(error) => {
            let status = match error.code {
                "cancelled" => ToolStatus::Cancelled,
                "timeout" => ToolStatus::TimedOut,
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
