use super::{Control, MAX_SCRIPT_BYTES, TaggedEngine, ecs, engine, worker_failure};
use crate::{
    ToolFailure,
    ecs_access::{EcsAccess, SceneDefinition, SceneStored},
};
use agentelier_skills::{SkillScript, UserSkills};
use rhai::{AST, Dynamic, Engine, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const TEMPLATE: &str = "agentelier.scripted_widget";
pub type State = Map<String, Value>;
const STATE_FIELD: &str = "script_state";

pub fn encode_state(state: &State) -> Result<State, ToolFailure> {
    let value = serde_json::to_string(state)
        .map_err(|error| ToolFailure::new("invalid_widget_state", error.to_string()))?;
    Ok(State::from_iter([(
        STATE_FIELD.into(),
        Value::String(value),
    )]))
}

pub fn decode_state(values: &State) -> Result<State, ToolFailure> {
    let value = values
        .get(STATE_FIELD)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ToolFailure::new("invalid_widget_state", "控件的存档缺少已确认的脚本状态")
        })?;
    serde_json::from_str(value)
        .map_err(|error| ToolFailure::new("invalid_widget_state", error.to_string()))
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillScript>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub parameters: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub resources: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<View>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub parts: Vec<Value>,
    #[serde(default)]
    pub state: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<Playback>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Playback {
    pub running: bool,
    pub interval_ms: u64,
    #[serde(default)]
    pub reset_clock: bool,
}

#[derive(Clone)]
pub struct Program(Arc<Compiled>);

pub struct Candidate {
    pub expected_revision: u64,
    pub captured: Option<State>,
    pub program: Program,
}

struct Compiled {
    engine: Mutex<Engine>,
    ast: AST,
    parameters: State,
    resources: State,
    has_restore: bool,
    has_events: bool,
}

impl Program {
    pub fn accepts_events(&self) -> bool {
        self.0.has_events
    }
}

#[derive(Clone, Serialize)]
pub struct Event {
    pub part: String,
    #[serde(flatten)]
    pub input: Input,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Click,
    Number {
        value: Value,
        is_final: bool,
    },
    Selection {
        value: Value,
    },
    Text {
        value: String,
        is_final: bool,
    },
    Key {
        key: String,
        pressed: bool,
        repeat: bool,
    },
    Drag {
        phase: DragPhase,
        position: [f32; 2],
        delta: [f32; 2],
    },
    Tick {
        dt: f64,
        elapsed: f64,
    },
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DragPhase {
    Start,
    Move,
    End,
}

pub struct Reply {
    pub program: Program,
    pub view: Option<View>,
    pub time: Option<(Instant, Duration)>,
}

pub struct Timer {
    pub due: Instant,
    pub last: Instant,
    pub elapsed: Duration,
}

enum Invocation {
    Init(Box<Definition>),
    Restore(Box<Definition>, State, RestoreSource),
    Event(Program, State, Event),
    Tick(Program, State, Timer),
}

enum RestoreSource {
    Snapshot,
    CurrentSkill,
}

pub struct Job {
    invocation: Invocation,
    result: mpsc::Sender<Result<Reply, ToolFailure>>,
    cancel: CancellationToken,
}

pub struct Ticket {
    result: Mutex<mpsc::Receiver<Result<Reply, ToolFailure>>>,
    cancel: CancellationToken,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Ticket {
    pub fn poll(&self) -> Option<Result<Reply, ToolFailure>> {
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
    pub fn restore(mut definition: Definition, state: State) -> (Self, Ticket) {
        definition.prepared = None;
        Self::new(Invocation::Restore(
            Box::new(definition),
            state,
            RestoreSource::CurrentSkill,
        ))
    }

    pub fn event(program: Program, state: State, event: Event) -> (Self, Ticket) {
        Self::new(Invocation::Event(program, state, event))
    }

    pub fn tick(program: Program, state: State, timer: Timer) -> (Self, Ticket) {
        Self::new(Invocation::Tick(program, state, timer))
    }

    fn new(invocation: Invocation) -> (Self, Ticket) {
        let (send, receive) = mpsc::channel();
        let cancel = CancellationToken::new();
        (
            Self {
                invocation,
                result: send,
                cancel: cancel.clone(),
            },
            Ticket {
                result: Mutex::new(receive),
                cancel,
            },
        )
    }

    pub async fn run(self, access: EcsAccess, skills: UserSkills, shutdown: CancellationToken) {
        let Self {
            invocation,
            result,
            cancel,
        } = self;
        if let Invocation::Tick(_, _, timer) = &invocation {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = cancel.cancelled() => return,
                () = tokio::time::sleep_until(timer.due.into()) => {}
            }
        }
        let task = evaluate(invocation, access, skills, &cancel);
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
    stored: Option<&SceneStored>,
    state: Option<State>,
    access: EcsAccess,
    skills: UserSkills,
    cancel: &CancellationToken,
) -> Result<Program, ToolFailure> {
    let mut definition: Definition = serde_json::from_value(Value::Object(scene.props.clone()))
        .map_err(|error| ToolFailure::new("invalid_scene_props", error.to_string()))?;
    let previous = stored
        .filter(|stored| stored.definition.template == TEMPLATE)
        .and_then(|stored| stored.definition.props.get("skill"))
        .filter(|skill| !skill.is_null())
        .cloned()
        .map(serde_json::from_value::<SkillScript>)
        .transpose()
        .map_err(|error| ToolFailure::new("invalid_scene_props", error.to_string()))?;
    if definition.skill.is_none() {
        definition.skill = previous.clone();
    }
    let mut selected = None;
    if let Some(skill) = &definition.skill
        && (previous.as_ref() != Some(skill) || definition.source.trim().is_empty())
    {
        let source = skill.clone();
        let reader = skills.clone();
        let request = cancel.child_token();
        let _cancel_on_drop = request.clone().drop_guard();
        definition.source =
            tokio::task::spawn_blocking(move || read_skill(&reader, &source, &request))
                .await
                .map_err(worker_failure)??
                .ok_or_else(|| {
                    ToolFailure::new("widget_skill_missing", format!("找不到 Skill 脚本 {skill}"))
                })?;
        selected = Some(skill.clone());
    }
    definition.prepared = None;
    let source = Box::new(definition.clone());
    let invocation = match state {
        Some(state) => Invocation::Restore(source, state, RestoreSource::Snapshot),
        None => Invocation::Init(source),
    };
    let initial = evaluate(invocation, access, skills, cancel)
        .await
        .map_err(|error| skill_error(selected.as_ref(), error))?;
    definition.prepared = initial.view;
    scene.props = serde_json::from_value(serde_json::to_value(definition).map_err(output_error)?)
        .map_err(output_error)?;
    scene
        .validate()
        .map_err(|error| ToolFailure::new(error.code, error.message))?;
    Ok(initial.program)
}

pub(super) fn prior_state(stored: Option<&SceneStored>) -> Result<Option<State>, ToolFailure> {
    let Some(stored) = stored.filter(|stored| stored.definition.template == TEMPLATE) else {
        return Ok(None);
    };
    if let Some(values) = &stored.values {
        return decode_state(values).map(Some);
    }
    let previous: Definition =
        serde_json::from_value(Value::Object(stored.definition.props.clone()))
            .map_err(|error| ToolFailure::new("invalid_widget_state", error.to_string()))?;
    Ok(previous.prepared.map(|view| view.state))
}

async fn evaluate(
    invocation: Invocation,
    access: EcsAccess,
    skills: UserSkills,
    cancel: &CancellationToken,
) -> Result<Reply, ToolFailure> {
    let control = Control::new(cancel);
    let _cancel_on_drop = control.cancel.clone().drop_guard();
    tokio::task::spawn_blocking(move || run(invocation, access, &skills, control))
        .await
        .map_err(worker_failure)?
}

fn read_skill(
    skills: &UserSkills,
    source: &SkillScript,
    cancel: &CancellationToken,
) -> Result<Option<String>, ToolFailure> {
    skills
        .read_script(source, MAX_SCRIPT_BYTES, cancel)
        .map_err(|error| {
            ToolFailure::new(
                "widget_skill_read_failed",
                format!("Skill 脚本 {source}：{error}"),
            )
        })
}

fn skill_error(skill: Option<&SkillScript>, error: ToolFailure) -> ToolFailure {
    match skill {
        Some(skill) => ToolFailure::new(error.code, format!("Skill 脚本 {skill}：{error}")),
        None => error,
    }
}

fn run(
    invocation: Invocation,
    access: EcsAccess,
    skills: &UserSkills,
    control: Control,
) -> Result<Reply, ToolFailure> {
    control.check()?;
    let mut selected = None;
    let (program, state, event, restoring, time) = match invocation {
        Invocation::Init(definition) => (compile(*definition)?, State::new(), None, false, None),
        Invocation::Restore(mut definition, state, source) => {
            if matches!(source, RestoreSource::CurrentSkill)
                && let Some(skill) = definition.skill.take()
                && let Some(source) = read_skill(skills, &skill, &control.cancel)?
            {
                definition.source = source;
                selected = Some(skill);
            }
            let program =
                compile(*definition).map_err(|error| skill_error(selected.as_ref(), error))?;
            (program, state, None, true, None)
        }
        Invocation::Event(program, state, event) => (program, state, Some(event), false, None),
        Invocation::Tick(program, state, timer) => {
            let now = Instant::now();
            let dt = now.saturating_duration_since(timer.last);
            let elapsed = timer.elapsed.saturating_add(dt);
            let event = Event {
                part: String::new(),
                input: Input::Tick {
                    dt: dt.as_secs_f64(),
                    elapsed: elapsed.as_secs_f64(),
                },
            };
            (program, state, Some(event), false, Some((now, elapsed)))
        }
    };
    let compiled = &program.0;
    let mut engine = compiled
        .engine
        .lock()
        .map_err(|_| ToolFailure::new("widget_worker_failed", "控件执行器状态无效"))?;
    let progress = control.clone();
    engine.on_progress(move |_| {
        progress
            .check()
            .err()
            .map(|error| Dynamic::from(error.code))
    });
    let context = rhai::serde::to_dynamic(
        json!({"parameters": compiled.parameters, "resources": compiled.resources, "state": state}),
    )
    .map_err(|error| ToolFailure::new("invalid_widget_input", error.to_string()))?;
    engine.set_default_tag(Dynamic::from(ecs::Run {
        access,
        control: control.clone(),
    }));
    let tagged = TaggedEngine(&mut engine);
    let mut scope = Scope::new();
    let (result, optional) = if let Some(event) = event {
        let event = rhai::serde::to_dynamic(event)
            .map_err(|error| ToolFailure::new("invalid_widget_input", error.to_string()))?;
        (
            tagged
                .0
                .call_fn::<Dynamic>(&mut scope, &compiled.ast, "on_event", (context, event)),
            true,
        )
    } else {
        let entry = if restoring && compiled.has_restore {
            "restore"
        } else {
            "init"
        };
        (
            tagged
                .0
                .call_fn::<Dynamic>(&mut scope, &compiled.ast, entry, (context,)),
            false,
        )
    };
    drop(tagged);
    drop(engine);
    control.check()?;
    let result = result
        .map_err(|error| ToolFailure::new("widget_script_failed", error.to_string()))
        .map_err(|error| skill_error(selected.as_ref(), error))?;
    let view = if optional && result.is_unit() {
        None
    } else {
        Some(decode(&result).map_err(|error| skill_error(selected.as_ref(), error))?)
    };
    Ok(Reply {
        program,
        view,
        time,
    })
}

fn compile(definition: Definition) -> Result<Program, ToolFailure> {
    if definition.source.trim().is_empty() || definition.source.len() > MAX_SCRIPT_BYTES {
        return Err(ToolFailure::new(
            "invalid_script",
            "控件 Rhai 源文必须非空且不超过 64 KiB",
        ));
    }
    let mut engine = engine();
    engine.on_print(|_| {}).on_debug(|_, _, _| {});
    let ast = engine
        .compile(&definition.source)
        .map_err(|error| ToolFailure::new("script_parse_failed", error.to_string()))?;
    let has = |name, arity| {
        ast.iter_functions()
            .any(|function| function.name == name && function.params.len() == arity)
    };
    let (has_restore, has_events) = (has("restore", 1), has("on_event", 2));
    Ok(Program(Arc::new(Compiled {
        engine: Mutex::new(engine),
        ast,
        parameters: definition.parameters,
        resources: definition.resources,
        has_restore,
        has_events,
    })))
}

fn decode(result: &Dynamic) -> Result<View, ToolFailure> {
    let view: View = rhai::serde::from_dynamic(result)
        .map_err(|error| ToolFailure::new("invalid_widget_output", error.to_string()))?;
    if view
        .playback
        .is_some_and(|play| !(16..=1000).contains(&play.interval_ms))
    {
        return Err(ToolFailure::new(
            "invalid_widget_output",
            "播放间隔 interval_ms 必须在 16 到 1000 毫秒之间",
        ));
    }
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
