use crate::config::ConfigStore;
use agentelier_chat_ui::{CommitWorkspaceLayout, LayoutSide, WorkspaceLayout};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::ValueChange;
use bevy_widgetry::slider::{
    SliderDragEnded, SliderDragOutcome, WidgetrySlider, WidgetrySliderHeadless,
};
use bevy_widgetry::text::WidgetryText;

#[derive(Resource, Default)]
struct SyncingSliders(bool);

#[derive(Component, Clone, Copy)]
struct LayoutSlider(LayoutSide);

#[derive(Component, Clone, Copy)]
struct LayoutLabel(LayoutSide);

#[derive(Component, Clone, Default)]
struct LayoutStatus;

pub(super) fn install(app: &mut App) {
    app.init_resource::<SyncingSliders>()
        .add_observer(commit)
        .add_systems(PostUpdate, sync.before(UiSystems::Content));
}

fn commit(
    _event: On<CommitWorkspaceLayout>,
    config: Res<ConfigStore>,
    mut layout: ResMut<WorkspaceLayout>,
) {
    if layout.ratios == layout.committed {
        layout.error = None;
        return;
    }
    match config.save_workspace_layout(layout.ratios) {
        Ok(()) => {
            layout.committed = layout.ratios;
            layout.error = None;
        }
        Err(failure) => layout.error = Some(failure.to_string()),
    }
}

pub(super) fn section() -> impl Scene {
    bsn! {
        Node { width: percent(100), margin: UiRect::top(px(8)), flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
        Children [
            Text("布局") WidgetryText TextFont { font_size: bevy::text::FontSize::Px(22.0) }--
            @slider("会话栏（C）", "SettingsSessionPaneRatio", LayoutSide::Session)--
            @slider("右侧区域（E/F）", "SettingsRightPaneRatio", LayoutSide::Right)--
            Text::default() WidgetryText LayoutStatus Name("SettingsWorkspaceLayoutStatus")
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        ]
    }
}

fn slider(label: &'static str, name: &'static str, side: LayoutSide) -> impl Scene {
    bsn! {
        Node { align_items: AlignItems::Center, column_gap: px(14), min_width: px(0) }
        Children [
            Text(label) WidgetryText Node { min_width: px(135), flex_shrink: 0.0 }--
            @WidgetrySlider { @min: 0.0, @max: 1.0 }
            template(move |_| Ok(LayoutSlider(side))) Name(name) on(adjust) on(finish)
            Node { flex_grow: 1.0, min_width: px(0) }--
            Text::default() WidgetryText template(move |_| Ok(LayoutLabel(side)))
            Node { min_width: px(60), flex_shrink: 0.0 }
        ]
    }
}

fn adjust(
    event: On<ValueChange<f32>>,
    sliders: Query<&LayoutSlider>,
    updating: Res<SyncingSliders>,
    mut layout: ResMut<WorkspaceLayout>,
    mut commands: Commands,
) {
    if updating.0 {
        return;
    }
    if let Ok(slider) = sliders.get(event.source) {
        layout.adjust(slider.0, event.value);
        if event.is_final {
            commands.trigger(CommitWorkspaceLayout);
        }
    }
}

fn finish(event: On<SliderDragEnded>, mut commands: Commands) {
    if event.outcome == SliderDragOutcome::Completed {
        commands.trigger(CommitWorkspaceLayout);
    }
}

fn sync(world: &mut World) -> Result {
    let layout = world.resource::<WorkspaceLayout>();
    let ratios = layout.ratios;
    let status = match &layout.error {
        Some(failure) => format!("当前布局未保存。{failure}"),
        None if layout.ratios != layout.committed => "当前布局未保存".into(),
        None => String::new(),
    };
    let value = |side| match side {
        LayoutSide::Session => ratios.session,
        LayoutSide::Right => ratios.right,
    };
    let sliders = world
        .query::<(Entity, &LayoutSlider)>()
        .iter(world)
        .map(|(entity, slider)| (entity, slider.0))
        .collect::<Vec<_>>();
    for (entity, side) in sliders {
        let next = value(side);
        let current = WidgetrySliderHeadless::value(world, entity)
            .map_err(|failure| failure.into_bevy_error())?;
        if (current - next).abs() > 0.0001 {
            world.resource_mut::<SyncingSliders>().0 = true;
            let result = WidgetrySliderHeadless::set_value_in_world(world, entity, next);
            world.resource_mut::<SyncingSliders>().0 = false;
            result.map_err(|failure| failure.into_bevy_error())?;
        }
    }
    for (label, mut text) in world.query::<(&LayoutLabel, &mut Text)>().iter_mut(world) {
        text.set_if_neq(Text(format!("{:.1}%", value(label.0) * 100.0)));
    }
    for mut text in world
        .query_filtered::<&mut Text, With<LayoutStatus>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(status.clone()));
    }
    Ok(())
}
