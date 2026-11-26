use super::{persist_positions, pointer_position};
use bevy::{picking::pointer::PointerId, prelude::*};

#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub pick: fn(&World, Entity, Vec2, f32) -> Option<(usize, Vec2)>,
    pub adjust: fn(&mut World, Entity, usize, Vec2) -> Result<bool>,
}

#[derive(Component)]
pub(super) struct BoardInput {
    pub root: Entity,
    pub canvas: Entity,
    gesture: Gesture,
    drag: Option<Drag>,
}

#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    pointer: PointerId,
    offset: Vec2,
    changed: bool,
}

impl BoardInput {
    pub fn new(root: Entity, canvas: Entity, gesture: Gesture) -> Self {
        Self {
            root,
            canvas,
            gesture,
            drag: None,
        }
    }

    pub fn reset(&mut self) {
        self.drag = None;
    }
}

pub(super) fn install(app: &mut App) {
    app.add_observer(press)
        .add_observer(drag)
        .add_observer(release)
        .add_observer(end_drag)
        .add_observer(cancel);
}

fn press(event: On<PointerPress>, inputs: Query<&BoardInput>, mut commands: Commands) {
    if event.button != PointerButton::Primary || inputs.get(event.entity).is_err() {
        return;
    }
    let (entity, screen, pointer) = (event.entity, event.pointer.position, event.pointer.id);
    commands.queue(move |world: &mut World| {
        let Some(input) = world
            .get::<BoardInput>(entity)
            .filter(|input| input.drag.is_none())
        else {
            return;
        };
        let Some((point, zoom)) = pointer_position(world, input, screen) else {
            return;
        };
        let Some((index, at)) = (input.gesture.pick)(world, input.root, point, 15.0 / zoom) else {
            return;
        };
        if let Some(mut input) = world.get_mut::<BoardInput>(entity) {
            input.drag = Some(Drag {
                index,
                pointer,
                offset: at - point,
                changed: false,
            });
        }
    });
}

fn drag(event: On<PointerDrag>, inputs: Query<&BoardInput>, mut commands: Commands) {
    if event.button != PointerButton::Primary || inputs.get(event.entity).is_err() {
        return;
    }
    let (entity, screen, pointer) = (event.entity, event.pointer.position, event.pointer.id);
    commands.queue(move |world: &mut World| -> Result {
        let Some(input) = world.get::<BoardInput>(entity) else {
            return Ok(());
        };
        let Some(drag) = input.drag.filter(|drag| drag.pointer == pointer) else {
            return Ok(());
        };
        let Some((point, _)) = pointer_position(world, input, screen) else {
            return Ok(());
        };
        if (input.gesture.adjust)(world, input.root, drag.index, point + drag.offset)?
            && let Some(mut input) = world.get_mut::<BoardInput>(entity)
            && let Some(drag) = &mut input.drag
        {
            drag.changed = true;
        }
        Ok(())
    });
}

fn release(event: On<PointerRelease>, inputs: Query<&BoardInput>, mut commands: Commands) {
    if event.button == PointerButton::Primary && inputs.get(event.entity).is_ok() {
        finish(&mut commands, event.entity, event.pointer.id);
    }
}

fn end_drag(event: On<PointerDragEnd>, inputs: Query<&BoardInput>, mut commands: Commands) {
    if event.button == PointerButton::Primary && inputs.get(event.entity).is_ok() {
        finish(&mut commands, event.entity, event.pointer.id);
    }
}

fn cancel(event: On<PointerCancel>, inputs: Query<&BoardInput>, mut commands: Commands) {
    if inputs.get(event.entity).is_ok() {
        finish(&mut commands, event.entity, event.pointer.id);
    }
}

fn finish(commands: &mut Commands, entity: Entity, pointer: PointerId) {
    commands.queue(move |world: &mut World| {
        let Some(mut input) = world.get_mut::<BoardInput>(entity) else {
            return;
        };
        if !input.drag.is_some_and(|drag| drag.pointer == pointer) {
            return;
        }
        let root = input.root;
        if input.drag.take().is_some_and(|drag| drag.changed) {
            persist_positions(world, root);
        }
    });
}
