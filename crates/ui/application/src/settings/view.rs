use super::{
    Choice, DefaultsSummary, DirectoryPath, DirectoryPicker, Field, Selector, SettingsMessage,
    SettingsWindow,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxContent};
use bevy_widgetry::file_dialog::{WidgetryFileDialog, WidgetryFileDialogMode};
use bevy_widgetry::list_view::WidgetryListViewRenderer;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField};
use bevy_widgetry::window::{
    WidgetryWindowBackground, WidgetryWindowControlsConfig, WidgetryWindowTitle,
    owned_widgetry_window,
};
use std::path::PathBuf;

#[derive(Component, Clone, Default)]
struct ChooseDirectory;

#[derive(Component, Clone, Default)]
struct NextDirectoryLabel;

pub(super) fn window([scale, model, effort]: [Selector; 3]) -> impl Scene {
    bsn! {
        SettingsWindow Name("SettingsWindow")
        @owned_widgetry_window(
            Window { title: "设置".into(), resolution: (690, 700).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                @WidgetryScrollArea {
                    @children: {Some(Box::new(content(scale,model,effort)) as Box<dyn SceneList>)},
                }
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
            },
        )
    }
}

fn content(scale: Selector, model: Selector, effort: Selector) -> impl SceneList {
    bsn_list! {
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, flex_shrink: 0.0, row_gap: px(12), padding: UiRect::all(px(16)) }
    Children [
        Text("常规") WidgetryText TextFont { font_size: bevy::text::FontSize::Px(22.0) }--
        Text("当前数据目录") WidgetryText--
        @path_field(DirectoryPath::Current, "SettingsCurrentDirectory")--
        Text("下次启动使用") WidgetryText NextDirectoryLabel Node { display: Display::None }--
        @path_field(DirectoryPath::Next, "SettingsNextDirectory")--
        @AgentelierButton {
            @style: AgentelierButtonStyle::Secondary,
            @children: {Some(Box::new(bsn_list! {Text("更改数据目录") WidgetryText}) as Box<dyn SceneList>)}
        }
        ChooseDirectory Name("SettingsChooseDataDirectory") Node { align_self: AlignSelf::Start }
        on(choose_directory)--
        @selector("界面缩放", scale)--
        Text("默认会话") WidgetryText TextFont { font_size: bevy::text::FontSize::Px(22.0) }--
        @selector("默认模型", model)--
        @selector("默认推理强度", effort)--
        Node { column_gap: px(12), align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap }
        Children [
            @AgentelierButton {
                @children: {Some(Box::new(bsn_list! {Text("保存会话默认值") WidgetryText}) as Box<dyn SceneList>)}
            }
            Name("SettingsSaveSessionDefaults") on(save_defaults)--
            Text::default() WidgetryText DefaultsSummary Name("SettingsSavedDefaults")
        ]--
        Text::default() WidgetryText SettingsMessage Name("SettingsFeedback")
        TextLayout::linebreak(LineBreak::AnyCharacter)--
        @super::avatar::section()
    ]
    }
}

fn path_field(path: DirectoryPath, name: &'static str) -> impl Scene {
    bsn! {
        @WidgetryReadOnlyTextField {
            @multiline: true,
            @auto_size: {Some(AutoSize { min_rows: 1, max_rows: 2 })},
        }
        template(move |_| Ok(path)) Name(name)
        Node { width: percent(100), min_width: px(0), display: Display::None }
    }
}

fn selector(label: &'static str, selector: Selector) -> impl Scene {
    let source = selector.source;
    let (name, placeholder) = match selector.field {
        Field::Scale => ("SettingsUiScale", "选择界面缩放"),
        Field::Model => ("SettingsDefaultModel", "模型目录暂不可用"),
        Field::Effort => ("SettingsDefaultEffort", "请选择推理强度"),
    };
    bsn! {
        Node { align_items: AlignItems::Center, column_gap: px(12), min_width: px(0) }
        Children [
            Text(label) WidgetryText Node { min_width: px(126), flex_shrink: 0.0 }--
            @WidgetryComboBox::<Choice> {
                @source: source,
                @placeholder: {WidgetryComboBoxContent::new(move || bsn_list! { Text(placeholder) WidgetryText })},
                @renderer: {WidgetryListViewRenderer::new(|choice: &Choice| {
                    let label = choice.label();
                    bsn_list! { Text(label) WidgetryText TextLayout::no_wrap() }
                })},
            }
            Name(name) template(move |_| Ok(selector))
            Node { flex_grow: 1.0, min_width: px(0) } on(super::select)
        ]
    }
}

pub(super) fn picker(initial: Option<PathBuf>) -> impl Scene {
    bsn! {
        DirectoryPicker Name("SettingsDirectoryPickerWindow")
        @owned_widgetry_window(
            Window { title: "选择数据目录".into(), resolution: (840, 580).into(), ..default() },
            WidgetryWindowControlsConfig { maximize_visible: false, ..default() },
            WidgetryWindowBackground::Theme,
            Default::default(),
            bsn_list! { WidgetryWindowTitle WidgetryText Pickable::IGNORE },
            bsn_list! {
                @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickDirectory, @initial_directory: {initial} }
                Name("SettingsDirectoryPicker") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
                on(super::chosen)
            },
        )
    }
}

pub(super) fn set_directory_busy(world: &mut World, busy: bool, show_next: bool) {
    let button = world
        .query_filtered::<Entity, With<ChooseDirectory>>()
        .single(world);
    if let Ok(button) = button
        && let Ok(mut entity) = world.get_entity_mut(button)
    {
        if busy {
            entity.insert(InteractionDisabled);
        } else {
            entity.remove::<InteractionDisabled>();
        }
    }
    for mut node in world
        .query_filtered::<&mut Node, With<NextDirectoryLabel>>()
        .iter_mut(world)
    {
        node.display = if show_next {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn choose_directory(_event: On<Activate>, mut commands: Commands) {
    commands.queue(super::open_picker);
}

fn save_defaults(_event: On<Activate>, mut commands: Commands) {
    commands.queue(super::save_defaults);
}
