use super::{Load, ScriptedWidget, widget};
use crate::agent_scenes::{
    composition,
    discrete::{DiscreteChange, DiscreteValue},
    numeric::{NumericChange, NumericValue},
    text_input::TextChange,
};
use bevy::{
    input::keyboard::KeyboardInput, input_focus::FocusedInput, prelude::*, ui_widgets::Activate,
};
use serde_json::json;
use widget::{DragPhase, Event, Input};

pub(super) fn install(app: &mut App) {
    app.add_observer(click)
        .add_observer(number)
        .add_observer(selection)
        .add_observer(text)
        .add_observer(key)
        .add_observer(drag_start)
        .add_observer(drag_move)
        .add_observer(drag_end);
}

fn click(event: On<Activate>, mut commands: Commands) {
    enqueue(&mut commands, event.entity, Input::Click);
}

fn number(event: On<NumericChange>, mut commands: Commands) {
    let value = match event.value {
        NumericValue::Integer(value) => json!(value),
        NumericValue::Float(value) => json!(value),
    };
    enqueue(
        &mut commands,
        event.source,
        Input::Number {
            value,
            is_final: event.is_final,
        },
    );
}

fn selection(event: On<DiscreteChange>, mut commands: Commands) {
    let value = match &event.value {
        DiscreteValue::Choice(value) => json!(value),
        DiscreteValue::Boolean(value) => json!(value),
    };
    enqueue(&mut commands, event.source, Input::Selection { value });
}

fn text(event: On<TextChange>, mut commands: Commands) {
    enqueue(
        &mut commands,
        event.source,
        Input::Text {
            value: event.value.clone(),
            is_final: event.is_final,
        },
    );
}

fn key(event: On<FocusedInput<KeyboardInput>>, mut commands: Commands) {
    if event.focused_entity != event.original_event_target() {
        return;
    }
    enqueue(
        &mut commands,
        event.original_event_target(),
        Input::Key {
            key: format!("{:?}", event.input.key_code),
            pressed: event.input.state.is_pressed(),
            repeat: event.input.repeat,
        },
    );
}

fn drag_start(event: On<PointerDragStart>, mut commands: Commands) {
    if event.button == PointerButton::Primary && event.entity == event.original_event_target() {
        let position = event.pointer.position.to_array();
        enqueue(
            &mut commands,
            event.entity,
            Input::Drag {
                phase: DragPhase::Start,
                position,
                delta: [0.0; 2],
            },
        );
    }
}

fn drag_move(event: On<PointerDrag>, mut commands: Commands) {
    if event.button == PointerButton::Primary && event.entity == event.original_event_target() {
        let position = event.pointer.position.to_array();
        enqueue(
            &mut commands,
            event.entity,
            Input::Drag {
                phase: DragPhase::Move,
                position,
                delta: event.delta.to_array(),
            },
        );
    }
}

fn drag_end(event: On<PointerDragEnd>, mut commands: Commands) {
    if event.button == PointerButton::Primary && event.entity == event.original_event_target() {
        let position = event.pointer.position.to_array();
        enqueue(
            &mut commands,
            event.entity,
            Input::Drag {
                phase: DragPhase::End,
                position,
                delta: event.distance.to_array(),
            },
        );
    }
}

fn enqueue(commands: &mut Commands, source: Entity, input: Input) {
    commands.queue(move |world: &mut World| {
        let Some((root, part)) = owner(world, source) else {
            return;
        };
        let Some(mut widget) = world.get_mut::<ScriptedWidget>(root) else {
            return;
        };
        let Load::Live(live) = &mut widget.load else {
            return;
        };
        if !live.program.accepts_events() {
            return;
        }
        let event = Event { part, input };
        if let Some(previous) = live.events.back_mut()
            && previous.part == event.part
            && combine(&mut previous.input, &event.input)
        {
            return;
        }
        live.events.push_back(event);
    });
}

fn owner(world: &World, source: Entity) -> Option<(Entity, String)> {
    let mut current = source;
    let mut part = None;
    loop {
        if part.is_none() {
            part = composition::part_id(world, current);
        }
        if world.get::<ScriptedWidget>(current).is_some() {
            return Some((current, part?.to_owned()));
        }
        current = world.get::<ChildOf>(current)?.parent();
    }
}

fn combine(previous: &mut Input, newest: &Input) -> bool {
    match (previous, newest) {
        (
            Input::Number {
                value: old,
                is_final: false,
            },
            Input::Number {
                value,
                is_final: false,
            },
        ) => {
            old.clone_from(value);
            true
        }
        (
            Input::Text {
                value: old,
                is_final: false,
            },
            Input::Text {
                value,
                is_final: false,
            },
        ) => {
            old.clone_from(value);
            true
        }
        (
            Input::Drag {
                phase: DragPhase::Move,
                position: old,
                delta: sum,
            },
            Input::Drag {
                phase: DragPhase::Move,
                position,
                delta,
            },
        ) => {
            *old = *position;
            for (left, right) in sum.iter_mut().zip(delta) {
                *left += right;
            }
            true
        }
        _ => false,
    }
}
