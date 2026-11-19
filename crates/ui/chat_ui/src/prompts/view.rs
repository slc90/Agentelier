use super::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::combo_box::WidgetryComboBoxContent;
use bevy_widgetry::list_view::WidgetryListViewRenderer;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryTextField};

const GAP: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 36.0;
const EDITOR_MIN_ROWS: usize = 6;
const EDITOR_MAX_ROWS: usize = 12;
const PREVIEW_HEIGHT: f32 = 320.0;

pub(crate) fn entry() -> impl Scene {
    button("提示词管理", "OpenPromptManager", Action::Open, false)
}

pub(crate) fn memory_entry() -> impl Scene {
    button("长期记忆", "OpenMemoryInspection", Action::Memory, false)
}

pub(crate) fn mcp_entry() -> impl Scene {
    button("MCP 服务器", "OpenMcpSettings", Action::Mcp, false)
}

fn button(label: &str, name: &'static str, action: Action, disabled: bool) -> impl Scene {
    let label = label.to_owned();
    (
        bsn! {
            @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText }) as Box<dyn SceneList>)} }
            Name(name) template(move |_| Ok(action)) on(activate)
            Node { height: px(BUTTON_HEIGHT), flex_shrink: 0.0 }
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn selector(source: Entity, kind: Selector, name: &'static str, placeholder: &str) -> impl Scene {
    let placeholder = placeholder.to_owned();
    bsn! {
        @WidgetryComboBox::<PromptChoice> {
            @source: source,
            @placeholder: {WidgetryComboBoxContent::new(move || { let label = placeholder.clone(); bsn_list! { Text(label) WidgetryText } })},
            @renderer: {WidgetryListViewRenderer::new(|choice: &PromptChoice| { let label = choice.label.clone(); bsn_list! { Text(label) WidgetryText } })},
        }
        Name(name) template(move |_| Ok(kind)) on(select)
        Node { width: percent(100), flex_shrink: 0.0 }
    }
}

fn field(
    value: String,
    hint: &'static str,
    field: Field,
    name: &'static str,
    multiline: bool,
) -> impl Scene {
    bsn! {
        @WidgetryTextField {
            @value: value, @hint_text: {Some(hint.into())}, @multiline: multiline,
            @auto_size: {multiline.then_some(AutoSize { min_rows: EDITOR_MIN_ROWS, max_rows: EDITOR_MAX_ROWS })},
        }
        Name(name) template(move |_| Ok(field))
        Node { width: percent(100), flex_shrink: 0.0 }
    }
}

fn readonly(value: String, name: &'static str) -> impl Scene {
    bsn! {
        @WidgetryScrollArea {
            @children: {Some(Box::new(bsn_list! {
                Text(value) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)
                Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
            }) as Box<dyn SceneList>)},
        }
        Name(name) Node { width: percent(100), height: px(PREVIEW_HEIGHT), min_width: px(0), flex_shrink: 0.0 }
    }
}

pub(crate) fn render(world: &mut World) -> Result {
    let opened = world.resource::<PromptState>().opened;
    let mut nodes = world.query_filtered::<&mut Node, With<NormalContent>>();
    for mut node in nodes.iter_mut(world) {
        node.display = if opened { Display::None } else { Display::Flex };
    }
    let mut hosts = world.query_filtered::<Entity, With<PromptWorkspace>>();
    let hosts: Vec<_> = hosts.iter(world).collect();
    for host in hosts {
        if let Some(mut node) = world.get_mut::<Node>(host) {
            node.display = if opened { Display::Flex } else { Display::None };
        }
        let children = world
            .get::<Children>(host)
            .map(|children| children.to_vec())
            .unwrap_or_default();
        for child in children {
            world.entity_mut(child).despawn();
        }
        if !opened {
            continue;
        }
        let state = world.resource::<PromptState>();
        let status = state.status.clone();
        let metadata = state.selected_metadata.clone();
        let preview = state.preview.clone();
        let busy = state.busy;
        let store = world.resource::<PromptStore>();
        let editor = store.editor.clone();
        let page = store.page;
        let sources = store.sources;
        let base = match &store.config.base {
            BaseInstruction::Default => {
                "基础条件：沿用默认请求（当前不附加客户端基础文本）".to_owned()
            }
            BaseInstruction::Override(reference) => format!(
                "基础条件：覆盖为 {} · v{}",
                store.library.find(reference)?.name,
                reference.version
            ),
        };
        let prompt_page = !matches!(page, Page::Mcp | Page::Memory);
        let prompt_navigation = prompt_page.then(|| {
            bsn_list! {
                @button("提示词库", "PromptLibraryTab", Action::Library, busy)--
                @button("组合与预览", "PromptConfigTab", Action::Config, busy)--
                @button("会话与请求核对", "PromptSessionTab", Action::Session, busy)--
                @button("重新加载库", "ReloadPromptLibrary", Action::Reload, busy)
            }
        });
        let prompt_status = prompt_page.then(|| bsn! {
            Text(status) WidgetryText Name("PromptManagerStatus") TextLayout::linebreak(LineBreak::AnyCharacter)
        });
        let body: Box<dyn Scene> = match page {
            Page::Mcp => crate::mcp::panel(world),
            Page::Memory => Box::new(crate::memory::panel(
                world.resource::<crate::memory::MemoryPane>(),
            )),
            Page::Library => Box::new(bsn! {
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
                Children [
                    @selector(sources[0], Selector::Library, "PromptLibrarySelector", "选择来源或已有版本")--
                    Text(metadata) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                    Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                    Children [
                        @button("新建", "NewPrompt", Action::New, busy)--
                        @button(&format!("编辑语言：{}（切换）", editor.language), "PromptLanguage", Action::Language, busy)--
                        @button("保存为新版本", "SavePromptRevision", Action::SaveRevision, busy)
                    ]--
                    @field(editor.name, "提示词名称", Field::Name, "PromptNameEditor", false)--
                    @field(editor.text, "提示词正文；保存不会覆盖来源或旧版本", Field::Body, "PromptBodyEditor", true)--
                    Text("从 UTF-8 文件导入；语言使用上方选择") WidgetryText--
                    @field(editor.path, "输入文件绝对路径", Field::Path, "PromptImportPath", false)--
                    @button("导入并保存", "ImportPrompt", Action::Import, busy)
                ]
            }),
            Page::Config => Box::new(bsn! {
                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
                Children [
                    @selector(sources[0], Selector::Library, "PromptConfigSelector", "选择要使用的提示词版本")--
                    Text(base) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                    Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                    Children [
                        @button("用所选版本覆盖基础", "SetPromptBase", Action::Base, busy)--
                        @button("沿用默认基础", "DefaultPromptBase", Action::DefaultBase, busy)--
                        @button("追加所选版本", "AddPromptSupplement", Action::Add, busy)
                    ]--
                    Text("补充指令按列表顺序发送；移除即停用，可重新追加") WidgetryText--
                    @selector(sources[1], Selector::Supplement, "PromptSupplementSelector", "当前没有选中的补充指令")--
                    Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                    Children [
                        @button("上移", "MovePromptUp", Action::Up, busy)--
                        @button("下移", "MovePromptDown", Action::Down, busy)--
                        @button("移除", "RemovePromptSupplement", Action::Remove, busy)
                    ]--
                    @selector(sources[2], Selector::Preset, "PromptPresetSelector", "选择已保存的组合预设")--
                    @field(editor.preset_name, "组合预设名称；同名保存更新配置", Field::PresetName, "PromptPresetName", false)--
                    Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
                    Children [
                        @button("保存组合", "SavePromptPreset", Action::SavePreset, busy)--
                        @button("加载组合", "LoadPromptPreset", Action::LoadPreset, busy)
                    ]--
                    Text("新会话有效指令预览（顺序即发送顺序；角色均为 developer）") WidgetryText--
                    @readonly(preview, "PromptEffectivePreview")
                ]
            }),
            Page::Session => {
                let state = world.resource::<crate::conversation::ChatState>();
                let value = crate::conversation::request_details(state);
                Box::new(bsn! {
                    Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
                    Children [
                        @crate::request_inspection::panel()--
                        @crate::project_context::panel(state)--
                        @readonly(value, "PromptSessionSnapshot")
                    ]
                })
            }
        };
        world.commands().spawn_scene_with_error_handler(bsn! {
            ChildOf(host) Name("PromptManager")
            Node { width: percent(100), height: percent(100), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
            Children [
                Node { column_gap: px(GAP), row_gap: px(GAP), flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0 }
                Children [
                    @button("返回聊天", "ClosePromptManager", Action::Close, false)--
                    {prompt_navigation}
                ]--
                @prompt_status--
                @WidgetryScrollArea { @children: {Some(Box::new(bsn_list! { @body }) as Box<dyn SceneList>)} }
                Name("PromptManagerScroll") Node { width: percent(100), flex_grow: 1.0, min_height: px(0) }
            ]
        });
    }
    Ok(())
}
