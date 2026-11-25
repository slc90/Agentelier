use bevy::prelude::*;
use bevy::ui::{UiSystems, UiTransform};
use bevy::window::{PrimaryWindow, RequestRedraw};

const STROKE: f32 = 2.0;
const COLOR: Color = Color::srgb_u8(0x25, 0x66, 0x4C);

#[derive(Component, Clone, Default)]
struct Spinner;

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, animate.after(UiSystems::Layout));
}

pub(super) fn ring(size: f32) -> impl Scene {
    bsn! {
        Spinner Pickable::IGNORE
        Node { width: px(size), height: px(size), flex_shrink: 0.0, border: UiRect::all(px(STROKE)), border_radius: BorderRadius::all(percent(50)) }
        BorderColor { top: COLOR, left: COLOR, right: Color::NONE, bottom: Color::NONE }
    }
}

fn animate(
    time: Res<Time<Real>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut spinners: Query<(&ComputedNode, &InheritedVisibility, &mut UiTransform), With<Spinner>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    if !window.single().is_ok_and(|window| window.visible) {
        return;
    }
    let rotation = Rot2::turn_fraction(time.elapsed_secs_f64().fract() as f32);
    let mut active = false;
    for (node, visibility, mut transform) in &mut spinners {
        if !node.is_empty() && visibility.get() {
            transform.rotation = rotation;
            active = true;
        }
    }
    if active {
        redraw.write(RequestRedraw);
    }
}
