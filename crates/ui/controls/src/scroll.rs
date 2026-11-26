use bevy::{
    input::mouse::MouseScrollPixelsPerLine,
    picking::events::PointerScroll,
    prelude::*,
    ui::{OverflowAxis, ScrollPosition},
    ui_widgets::ScrollArea,
};

/// Sends vertical wheel input from a horizontal scroll area to its nearest vertical ancestor.
pub fn forward_vertical_scroll(
    event: On<PointerScroll>,
    parents: Query<&ChildOf>,
    mut areas: Query<(&Node, &ComputedNode, &mut ScrollPosition), With<ScrollArea>>,
    conversion: Res<MouseScrollPixelsPerLine>,
) {
    let delta = event.to_pixels(&conversion).y;
    if delta == 0.0 {
        return;
    }
    for ancestor in parents.iter_ancestors(event.entity) {
        if let Ok((node, computed, mut position)) = areas.get_mut(ancestor)
            && node.overflow.y == OverflowAxis::Scroll
        {
            let maximum = ((computed.content_size.y - computed.size.y)
                * computed.inverse_scale_factor)
                .max(0.0);
            position.0.y = (position.0.y - delta).clamp(0.0, maximum);
            break;
        }
    }
}
