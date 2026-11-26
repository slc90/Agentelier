use super::{Control, MAX_SCRIPT_BYTES, ecs, engine, worker_failure};
use crate::{
    ToolFailure,
    ecs_access::{EcsAccess, SceneDefinition},
};
use rhai::{Dynamic, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

pub const TEMPLATE: &str = "agentelier.scripted_widget";
pub type State = Map<String, Value>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub source: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub parameters: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub resources: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<View>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub parts: Vec<Value>,
    #[serde(default)]
    pub state: Map<String, Value>,
}

pub struct Job {
    definition: Definition,
    state: Map<String, Value>,
    result: mpsc::Sender<Result<View, ToolFailure>>,
    cancel: CancellationToken,
}

pub struct Ticket {
    result: Mutex<mpsc::Receiver<Result<View, ToolFailure>>>,
    cancel: CancellationToken,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Ticket {
    pub fn poll(&self) -> Option<Result<View, ToolFailure>> {
        let Ok(result) = self.result.lock() else {
            return Some(Err(ToolFailure::new(
                "widget_worker_failed",
                "控件工作者回执无效",
            )));
        };
        match result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(ToolFailure::new(
                "widget_worker_failed",
                "控件工作者未返回结果",
            ))),
        }
    }
}

impl Job {
    pub fn restore(mut definition: Definition, state: Map<String, Value>) -> (Self, Ticket) {
        let (send, receive) = mpsc::channel();
        let cancel = CancellationToken::new();
        definition.prepared = None;
        (
            Self {
                definition,
                state,
                result: send,
                cancel: cancel.clone(),
            },
            Ticket {
                result: Mutex::new(receive),
                cancel,
            },
        )
    }

    pub async fn run(self, access: EcsAccess, shutdown: CancellationToken) {
        let Self {
            definition,
            state,
            result,
            cancel,
        } = self;
        let task = evaluate(definition, Some(state), access, &cancel);
        tokio::pin!(task);
        let value = tokio::select! {
            biased;
            () = shutdown.cancelled() => { cancel.cancel(); task.await }
            value = &mut task => value,
        };
        drop(result.send(value));
    }
}

pub(super) async fn prepare(
    scene: &mut SceneDefinition,
    access: EcsAccess,
    cancel: &CancellationToken,
) -> Result<(), ToolFailure> {
    let mut definition: Definition = serde_json::from_value(Value::Object(scene.props.clone()))
        .map_err(|error| ToolFailure::new("invalid_scene_props", error.to_string()))?;
    definition.prepared = None;
    definition.prepared = Some(evaluate(definition.clone(), None, access, cancel).await?);
    scene.props = serde_json::from_value(serde_json::to_value(definition).map_err(output_error)?)
        .map_err(output_error)?;
    scene
        .validate()
        .map_err(|error| ToolFailure::new(error.code, error.message))
}

async fn evaluate(
    definition: Definition,
    state: Option<Map<String, Value>>,
    access: EcsAccess,
    cancel: &CancellationToken,
) -> Result<View, ToolFailure> {
    let control = Control::new(cancel);
    let _cancel_on_drop = control.cancel.clone().drop_guard();
    tokio::task::spawn_blocking(move || run(definition, state, access, control))
        .await
        .map_err(worker_failure)?
}

fn run(
    definition: Definition,
    state: Option<Map<String, Value>>,
    access: EcsAccess,
    control: Control,
) -> Result<View, ToolFailure> {
    control.check()?;
    if definition.source.trim().is_empty() || definition.source.len() > MAX_SCRIPT_BYTES {
        return Err(ToolFailure::new(
            "invalid_script",
            "控件 Rhai 源文必须非空且不超过 64 KiB",
        ));
    }
    let mut engine = engine();
    let progress = control.clone();
    engine
        .on_print(|_| {})
        .on_debug(|_, _, _| {})
        .on_progress(move |_| {
            progress
                .check()
                .err()
                .map(|error| Dynamic::from(error.code))
        });
    let program = engine
        .compile(&definition.source)
        .map_err(|error| ToolFailure::new("script_parse_failed", error.to_string()))?;
    let entry = if state.is_some()
        && program
            .iter_functions()
            .any(|fn_meta| fn_meta.name == "restore" && fn_meta.params.len() == 1)
    {
        "restore"
    } else {
        "init"
    };
    let context = rhai::serde::to_dynamic(json!({"parameters": definition.parameters, "resources": definition.resources, "state": state.unwrap_or_default()}))
        .map_err(|error| ToolFailure::new("invalid_widget_input", error.to_string()))?;
    engine.set_default_tag(Dynamic::from(ecs::Run {
        access,
        control: control.clone(),
    }));
    let result = engine.call_fn::<Dynamic>(&mut Scope::new(), &program, entry, (context,));
    control.check()?;
    let result =
        result.map_err(|error| ToolFailure::new("widget_script_failed", error.to_string()))?;
    let view: View = rhai::serde::from_dynamic(&result)
        .map_err(|error| ToolFailure::new("invalid_widget_output", error.to_string()))?;
    if !(1..=16).contains(&view.parts.len())
        || view.parts.iter().any(|part| {
            part.pointer("/scene/template")
                .and_then(Value::as_str)
                .is_none_or(|template| template == TEMPLATE)
        })
    {
        return Err(ToolFailure::new(
            "invalid_widget_output",
            "Rhai 控件需返回 1 至 16 个有效原生子 Scene，且不能自我嵌套",
        ));
    }
    Ok(view)
}

fn output_error(error: serde_json::Error) -> ToolFailure {
    ToolFailure::new("invalid_widget_output", error.to_string())
}
