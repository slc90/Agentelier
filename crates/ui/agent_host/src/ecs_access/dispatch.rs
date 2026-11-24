use super::read;
use agentelier_bridge_connection::ecs_access::{EcsEntity, EcsError};
use bevy::{
    prelude::*,
    ui_widgets::Activate,
    window::{PrimaryWindow, WindowCloseRequested},
};
use bevy_widgetry::disabled::WidgetryEffectiveDisabled;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc};

type Payload = Map<String, Value>;
type Result<T> = std::result::Result<T, EcsError>;
type Handler = Arc<dyn Fn(&mut World, &Payload) -> Result<()> + Send + Sync>;

#[derive(Clone)]
struct Adapter {
    description: &'static str,
    entity_field: Option<&'static str>,
    dispatch: Handler,
}

#[derive(Clone, Resource)]
pub(crate) struct Registry {
    events: BTreeMap<&'static str, Adapter>,
    messages: BTreeMap<&'static str, Adapter>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            events: BTreeMap::from([(
                "Activate",
                Adapter {
                    description: "Dispatch the normal activation observer event to a live, enabled widget.",
                    entity_field: Some("entity"),
                    dispatch: Arc::new(activate),
                },
            )]),
            messages: BTreeMap::from([(
                "WindowCloseRequested",
                Adapter {
                    description: "Request closure of a secondary window through its normal window lifecycle.",
                    entity_field: Some("window"),
                    dispatch: Arc::new(close_window),
                },
            )]),
        }
    }
}

impl Registry {
    pub(crate) fn register_unit_event<E: Event<Trigger<'static>: Default>>(
        &mut self,
        name: &'static str,
        description: &'static str,
        event: fn() -> E,
    ) -> Result<()> {
        let std::collections::btree_map::Entry::Vacant(entry) = self.events.entry(name) else {
            return Err(EcsError::new(
                "duplicate_ecs_event",
                format!("ECS Event {name} 已登记"),
            ));
        };
        entry.insert(Adapter {
            description,
            entity_field: None,
            dispatch: Arc::new(move |world, payload| {
                if !payload.is_empty() {
                    return Err(EcsError::new(
                        "invalid_ecs_payload",
                        "此 Event 要求空映射参数",
                    ));
                }
                world.trigger(event());
                Ok(())
            }),
        });
        Ok(())
    }
}

pub(super) fn overview(world: &World) -> Value {
    let Some(registry) = world.get_resource::<Registry>() else {
        return Value::Null;
    };
    let list = |adapters: &BTreeMap<&'static str, Adapter>| {
        adapters
            .iter()
            .map(|(name, adapter)| {
                let mut fields = Payload::new();
                if let Some(field) = adapter.entity_field {
                    fields.insert(field.into(), json!("EcsEntity"));
                }
                json!({"name": name, "description": adapter.description, "payload": fields})
            })
            .collect::<Vec<_>>()
    };
    json!({"events": list(&registry.events), "messages": list(&registry.messages)})
}

pub(super) fn event(world: &mut World, name: &str, payload: &Payload) -> Result<Value> {
    let dispatch = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.events.get(name))
        .ok_or_else(|| EcsError::new("unsupported_event", format!("未登记 Event {name}")))?
        .dispatch
        .clone();
    dispatch(world, payload)?;
    Ok(json!({"dispatched": true}))
}

pub(super) fn message(world: &mut World, name: &str, payload: &Payload) -> Result<Value> {
    let dispatch = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.messages.get(name))
        .ok_or_else(|| EcsError::new("unsupported_message", format!("未登记 Message {name}")))?
        .dispatch
        .clone();
    dispatch(world, payload)?;
    Ok(json!({"queued": true}))
}

fn target<'a>(world: &'a World, payload: &Payload, field: &str) -> Result<EntityRef<'a>> {
    let token = payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|_| payload.len() == 1)
        .ok_or_else(|| {
            EcsError::new(
                "invalid_ecs_payload",
                format!("参数必须仅包含实体引用 {field}"),
            )
        })?;
    read::entity(world, EcsEntity::parse(token)?)
}

fn activate(world: &mut World, payload: &Payload) -> Result<()> {
    let target = target(world, payload, "entity")?;
    if target
        .get::<WidgetryEffectiveDisabled>()
        .is_some_and(WidgetryEffectiveDisabled::is_disabled)
    {
        return Err(EcsError::new("ecs_target_disabled", "目标控件当前已禁用"));
    }
    world.trigger(Activate {
        entity: target.id(),
    });
    Ok(())
}

fn close_window(world: &mut World, payload: &Payload) -> Result<()> {
    let target = target(world, payload, "window")?;
    if !target.contains::<Window>() || target.contains::<PrimaryWindow>() {
        return Err(EcsError::new(
            "invalid_ecs_target",
            "目标必须是辅助窗口实体",
        ));
    }
    world
        .write_message(WindowCloseRequested {
            window: target.id(),
        })
        .ok_or_else(|| {
            EcsError::new(
                "message_unavailable",
                "WindowCloseRequested 消息缓冲区不可用",
            )
        })?;
    Ok(())
}
