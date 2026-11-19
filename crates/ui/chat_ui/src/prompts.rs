mod view;
pub(crate) use view::{entry, mcp_entry, memory_entry, render};

use agentelier_login_ui::DataDirectoryReady;
use agentelier_prompts::{
    BaseInstruction, InstructionConfig, InstructionSnapshot, Library, PromptPreset, PromptRef,
    PromptRevision,
};
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt};
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel};
use bevy_widgetry::text_field::WidgetryTextFieldHeadless;
use std::{io, path::PathBuf, sync::Arc};
use tracing::{debug, error, info};

#[derive(Clone, Reflect)]
pub struct PromptChoice {
    pub id: String,
    pub version: u32,
    pub label: String,
}
impl PromptChoice {
    fn reference(&self) -> PromptRef {
        PromptRef {
            id: self.id.clone(),
            version: self.version,
        }
    }
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct PromptState {
    pub ready: bool,
    pub busy: bool,
    pub opened: bool,
    pub entries: Vec<PromptChoice>,
    pub preview: String,
    pub selected_metadata: String,
    pub status: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    Library,
    #[default]
    Config,
    Session,
    Mcp,
    Memory,
}

#[derive(Resource)]
struct PromptStore {
    library: Library,
    config: InstructionConfig,
    path: Option<PathBuf>,
    pending: Option<Pending>,
    selected: Option<PromptRef>,
    supplement: Option<usize>,
    preset: Option<String>,
    page: Page,
    editor: Editor,
    sources: [Entity; 3],
}
#[derive(Default, Clone)]
struct Editor {
    name: String,
    text: String,
    language: String,
    path: String,
    preset_name: String,
}
struct Pending {
    id: JobId,
    receiver: JobReceiver<Reply>,
    reply: Option<Arc<Reply>>,
}
struct Reply {
    result: io::Result<Library>,
    selected: Option<PromptRef>,
}
enum Work {
    Load,
    Save(Library, Option<PromptRef>),
    Import(Library, PathBuf, String),
}

#[derive(Component, Clone, Default)]
pub(crate) struct PromptWorkspace;
#[derive(Component, Clone, Default)]
pub(crate) struct NormalContent;
#[derive(Component, Clone, Copy)]
enum Selector {
    Library,
    Supplement,
    Preset,
}
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Body,
    Path,
    PresetName,
}
#[derive(Component, Clone, Copy, Debug)]
enum Action {
    Open,
    Mcp,
    Memory,
    Close,
    Library,
    Config,
    Session,
    New,
    Language,
    SaveRevision,
    Import,
    Reload,
    Base,
    DefaultBase,
    Add,
    Up,
    Down,
    Remove,
    SavePreset,
    LoadPreset,
}

pub(crate) fn install(app: &mut App) -> Result {
    app.register_widgetry_combo_box::<PromptChoice>()
        .map_err(|e| e.into_bevy_error())?;
    app.init_resource::<PromptState>()
        .register_type::<PromptState>()
        .add_systems(Startup, initialize)
        .add_observer(data_ready)
        .add_observer(mount)
        .add_systems(Update, receive)
        .add_systems(Update, restore_selectors);
    Ok(())
}

fn initialize(mut commands: Commands) -> Result {
    let library: Library = serde_json::from_str(bevy_app_assets::PROMPT_LIBRARY_JSON)?;
    library.validate()?;
    let sources = std::array::from_fn(|_| {
        commands
            .spawn(WidgetryListModel::<PromptChoice>::default())
            .id()
    });
    commands.insert_resource(PromptStore {
        library,
        config: default(),
        path: None,
        pending: None,
        selected: None,
        supplement: None,
        preset: None,
        page: default(),
        editor: Editor {
            language: "zh".into(),
            ..default()
        },
        sources,
    });
    Ok(())
}

fn data_ready(event: On<DataDirectoryReady>, mut commands: Commands) {
    let path = event.environment.root().join("prompts.json");
    commands.queue(move |world: &mut World| -> Result {
        world.resource_mut::<PromptStore>().path = Some(path);
        start(world, Work::Load)
    });
}

fn mount(_event: On<Add<PromptWorkspace>>, mut commands: Commands) {
    commands.queue(refresh);
}

pub(super) fn memory_visible(world: &World) -> bool {
    world.resource::<PromptState>().opened && world.resource::<PromptStore>().page == Page::Memory
}

pub(super) fn mcp_visible(world: &World) -> bool {
    world.resource::<PromptState>().opened && world.resource::<PromptStore>().page == Page::Mcp
}

pub(crate) fn snapshot(world: &World) -> io::Result<InstructionSnapshot> {
    let state = world.resource::<PromptState>();
    if !state.ready {
        return Err(io::Error::other(
            "提示词库尚未加载，请检查提示词管理中的状态",
        ));
    }
    let store = world.resource::<PromptStore>();
    store.library.resolve(&store.config)
}

fn refresh(world: &mut World) -> Result {
    if mcp_visible(world) {
        crate::mcp::capture(world)?;
    }
    let store = world.resource::<PromptStore>();
    let entries: Vec<_> = store
        .library
        .entries
        .iter()
        .map(|entry| PromptChoice {
            id: entry.reference.id.clone(),
            version: entry.reference.version,
            label: format!("{} · v{}", entry.name, entry.reference.version),
        })
        .collect();
    let supplements: Vec<_> = store
        .config
        .supplements
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            let entry = store.library.find(reference)?;
            Ok(PromptChoice {
                id: index.to_string(),
                version: 0,
                label: format!("{} · {} · v{}", index + 1, entry.name, reference.version),
            })
        })
        .collect::<io::Result<_>>()?;
    let presets: Vec<_> = store
        .library
        .presets
        .iter()
        .map(|preset| PromptChoice {
            id: preset.name.clone(),
            version: 0,
            label: preset.name.clone(),
        })
        .collect();
    let sources = store.sources;
    let snapshot = store.library.resolve(&store.config)?;
    let preview = serde_json::to_string_pretty(&serde_json::json!({
        "base_overridden": snapshot.base_overridden,
        "messages": snapshot.messages(),
    }))?;
    let metadata = store
        .selected
        .as_ref()
        .map(|reference| {
            let entry = store.library.find(reference)?;
            Ok::<_, io::Error>(format!(
                "{} · v{} · {} · {:?}{}\n来源：{}\n范围：{}\n语言配对：{}\n依赖：{}\nSHA-256：{}",
                entry.reference.id,
                entry.reference.version,
                entry.language,
                entry.kind,
                if entry.readonly {
                    " · 来源只读；保存生成派生版本"
                } else {
                    " · 保存新版本"
                },
                entry.source,
                entry.range,
                entry.pair.as_deref().unwrap_or("无"),
                entry.dependencies,
                entry.hash
            ))
        })
        .transpose()?
        .unwrap_or_else(|| "新建自定义提示词".into());
    for (source, items) in sources
        .into_iter()
        .zip([entries.clone(), supplements, presets])
    {
        let mut model = world
            .get_mut::<WidgetryListModel<PromptChoice>>(source)
            .ok_or_else(|| io::Error::other("提示词选择数据源不存在"))?;
        model.clear();
        for item in items {
            model.push(item).map_err(|e| e.into_bevy_error())?;
        }
    }
    let mut state = world.resource_mut::<PromptState>();
    state.entries = entries;
    state.preview = preview;
    state.selected_metadata = metadata;
    view::render(world)
}

fn restore_selectors(world: &mut World) -> Result {
    let mut query = world.query_filtered::<(Entity, &Selector), Added<Selector>>();
    let selectors: Vec<_> = query
        .iter(world)
        .map(|(entity, selector)| (entity, *selector))
        .collect();
    for (entity, selector) in selectors {
        let store = world.resource::<PromptStore>();
        let index = match selector {
            Selector::Library => store.selected.as_ref().and_then(|selected| {
                store
                    .library
                    .entries
                    .iter()
                    .position(|entry| entry.reference == *selected)
            }),
            Selector::Supplement => store.supplement,
            Selector::Preset => store.preset.as_ref().and_then(|name| {
                store
                    .library
                    .presets
                    .iter()
                    .position(|preset| preset.name == *name)
            }),
        };
        if let Some(index) = index {
            WidgetryComboBox::<PromptChoice>::select_index(&mut world.commands(), entity, index);
        }
    }
    Ok(())
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    selectors: Query<&Selector>,
    mut commands: Commands,
) -> Result {
    let Ok(selector) = selectors.get(event.source).copied() else {
        return Ok(());
    };
    let Some(id) = event.value else { return Ok(()) };
    commands.queue(move |world: &mut World| -> Result {
        if world.resource::<PromptState>().busy {
            return Ok(());
        }
        let store = world.resource::<PromptStore>();
        let source = store.sources[match selector {
            Selector::Library => 0,
            Selector::Supplement => 1,
            Selector::Preset => 2,
        }];
        let choice = world
            .get::<WidgetryListModel<PromptChoice>>(source)
            .and_then(|model| model.get_by_id(id))
            .ok_or_else(|| io::Error::other("提示词选项已变化"))?
            .clone();
        match selector {
            Selector::Library => {
                if store.selected.as_ref() == Some(&choice.reference()) {
                    return Ok(());
                }
                let entry = store.library.find(&choice.reference())?.clone();
                let mut store = world.resource_mut::<PromptStore>();
                store.selected = Some(entry.reference);
                store.editor.name = entry.name;
                store.editor.text = entry.text;
                store.editor.language = entry.language;
            }
            Selector::Supplement => {
                let index = choice.id.parse()?;
                if store.supplement == Some(index) {
                    return Ok(());
                }
                world.resource_mut::<PromptStore>().supplement = Some(index);
            }
            Selector::Preset => {
                if store.preset.as_ref() == Some(&choice.id) {
                    return Ok(());
                }
                world.resource_mut::<PromptStore>().preset = Some(choice.id);
            }
        }
        refresh(world)
    });
    Ok(())
}

fn capture(world: &mut World) -> Result {
    let mut query = world.query::<(Entity, &Field)>();
    let fields: Vec<_> = query
        .iter(world)
        .map(|(entity, field)| (entity, *field))
        .collect();
    for (entity, field) in fields {
        let value =
            WidgetryTextFieldHeadless::text(world, entity).map_err(|e| e.into_bevy_error())?;
        let mut store = world.resource_mut::<PromptStore>();
        match field {
            Field::Name => store.editor.name = value,
            Field::Body => store.editor.text = value,
            Field::Path => store.editor.path = value,
            Field::PresetName => store.editor.preset_name = value,
        }
    }
    Ok(())
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        capture(world)?;
        crate::mcp::capture(world)?;
        if let Err(failure) = apply(world, action) {
            error!(error = %failure, "提示词操作失败");
            world.resource_mut::<PromptState>().status = failure
                .to_string()
                .lines()
                .next()
                .unwrap_or("提示词操作失败，请查看应用日志")
                .to_owned();
        }
        refresh(world)
    });
    Ok(())
}

fn apply(world: &mut World, action: Action) -> Result {
    if world.resource::<PromptState>().busy
        && !matches!(
            action,
            Action::Open | Action::Mcp | Action::Memory | Action::Close
        )
    {
        return Ok(());
    }
    if memory_visible(world) && matches!(action, Action::Open | Action::Mcp | Action::Close) {
        crate::memory::close(world);
    }
    match action {
        Action::Memory => {
            world.resource_mut::<PromptState>().opened = true;
            world.resource_mut::<PromptStore>().page = Page::Memory;
            crate::memory::open(world);
        }
        Action::Open => {
            world.resource_mut::<PromptState>().opened = true;
            let mut store = world.resource_mut::<PromptStore>();
            if matches!(store.page, Page::Mcp | Page::Memory) {
                store.page = Page::Config;
            }
        }
        Action::Mcp => {
            world.resource_mut::<PromptState>().opened = true;
            world.resource_mut::<PromptStore>().page = Page::Mcp;
            crate::mcp::open(world);
        }
        Action::Close => {
            if crate::mcp_materials::opened(world) {
                crate::mcp_materials::close(world)?;
            }
            world.resource_mut::<PromptState>().opened = false;
        }
        Action::Library | Action::Config | Action::Session => {
            world.resource_mut::<PromptStore>().page = match action {
                Action::Library => Page::Library,
                Action::Session => Page::Session,
                _ => Page::Config,
            };
        }
        Action::Reload => start(world, Work::Load)?,
        Action::New => {
            let mut store = world.resource_mut::<PromptStore>();
            store.selected = None;
            store.editor.name.clear();
            store.editor.text.clear();
        }
        Action::Language => {
            let mut store = world.resource_mut::<PromptStore>();
            store.editor.language = if store.editor.language == "zh" {
                "en"
            } else {
                "zh"
            }
            .into();
        }
        Action::SaveRevision => {
            let store = world.resource::<PromptStore>();
            let mut library = store.library.clone();
            let selected = library.revise(
                store.selected.as_ref(),
                store.editor.name.clone(),
                store.editor.text.clone(),
                store.editor.language.clone(),
            )?;
            start(world, Work::Save(library, Some(selected)))?;
        }
        Action::Import => {
            let store = world.resource::<PromptStore>();
            let path = PathBuf::from(store.editor.path.trim());
            if !path.is_absolute() {
                return Err(io::Error::other("请输入提示词 UTF-8 文件的绝对路径").into());
            }
            start(
                world,
                Work::Import(store.library.clone(), path, store.editor.language.clone()),
            )?;
        }
        Action::Base | Action::Add => {
            let mut store = world.resource_mut::<PromptStore>();
            let reference = store
                .selected
                .clone()
                .ok_or_else(|| io::Error::other("请先选择提示词版本"))?;
            if matches!(action, Action::Base) {
                store.config.base = BaseInstruction::Override(reference);
            } else {
                store.config.supplements.push(reference);
                store.supplement = Some(store.config.supplements.len() - 1);
            }
        }
        Action::DefaultBase => {
            world.resource_mut::<PromptStore>().config.base = BaseInstruction::Default
        }
        Action::Up | Action::Down | Action::Remove => {
            let mut store = world.resource_mut::<PromptStore>();
            if let Some(index) = store
                .supplement
                .filter(|index| *index < store.config.supplements.len())
            {
                if matches!(action, Action::Remove) {
                    store.config.supplements.remove(index);
                    store.supplement = None;
                } else {
                    let next = if matches!(action, Action::Up) {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(store.config.supplements.len() - 1)
                    };
                    store.config.supplements.swap(index, next);
                    store.supplement = Some(next);
                }
            }
        }
        Action::SavePreset => {
            let store = world.resource::<PromptStore>();
            let name = store.editor.preset_name.trim().to_owned();
            if name.is_empty() {
                return Err(io::Error::other("请输入组合预设名称").into());
            }
            let mut library = store.library.clone();
            let preset = PromptPreset {
                name: name.clone(),
                config: store.config.clone(),
            };
            if let Some(current) = library.presets.iter_mut().find(|entry| entry.name == name) {
                *current = preset;
            } else {
                library.presets.push(preset);
            }
            start(world, Work::Save(library, None))?;
        }
        Action::LoadPreset => {
            let mut store = world.resource_mut::<PromptStore>();
            let preset = store
                .library
                .presets
                .iter()
                .find(|preset| Some(&preset.name) == store.preset.as_ref())
                .ok_or_else(|| io::Error::other("请选择已保存的组合预设"))?
                .clone();
            store.config = preset.config;
            store.supplement = None;
        }
    }
    info!(action = ?action, "已处理提示词管理操作");
    Ok(())
}

fn start(world: &mut World, work: Work) -> Result {
    let store = world.resource::<PromptStore>();
    if store.pending.is_some() {
        return Ok(());
    }
    let path = store
        .path
        .clone()
        .ok_or_else(|| io::Error::other("数据目录尚未就绪"))?;
    let sources: Library = serde_json::from_str(bevy_app_assets::PROMPT_LIBRARY_JSON)?;
    let (id, receiver) = world
        .resource::<JobManager>()
        .spawn_thread(
            JobOptions {
                label: Some("提示词库文件操作".into()),
                ..default()
            },
            move |context| {
                debug!("提示词库文件任务开始");
                if context.is_cancelled() {
                    context.acknowledge_cancel();
                    return Ok(());
                }
                let mut selected = None;
                let result = (|| -> io::Result<Library> {
                    match work {
                        Work::Load => Library::load(&path, &sources),
                        Work::Save(library, reference) => {
                            library.save(&path)?;
                            selected = reference;
                            Ok(library)
                        }
                        Work::Import(mut library, source, language) => {
                            let text = std::fs::read_to_string(&source)?;
                            let name = source
                                .file_name()
                                .ok_or_else(|| io::Error::other("文件路径缺少名称"))?
                                .to_string_lossy()
                                .into_owned();
                            let revision = PromptRevision::custom(
                                name,
                                text,
                                language,
                                source.to_string_lossy().into_owned(),
                            );
                            revision.validate()?;
                            selected = Some(revision.reference.clone());
                            library.entries.push(revision);
                            library.save(&path)?;
                            Ok(library)
                        }
                    }
                })();
                match &result {
                    Ok(_) => info!("提示词库文件操作完成"),
                    Err(failure) => error!(error = %failure, "提示词库文件操作失败"),
                }
                if context.send_blocking(Reply { result, selected })? == SendStatus::Canceled {
                    context.acknowledge_cancel();
                }
                debug!("提示词库文件任务结束");
                Ok(())
            },
        )
        .map_err(|e| e.into_bevy_error())?;
    world.resource_mut::<PromptStore>().pending = Some(Pending {
        id,
        receiver,
        reply: None,
    });
    let mut state = world.resource_mut::<PromptState>();
    state.busy = true;
    state.status = "正在读写提示词库…".into();
    Ok(())
}

fn receive(world: &mut World) -> Result {
    let Some(mut pending) = world.resource_mut::<PromptStore>().pending.take() else {
        return Ok(());
    };
    let outcome = loop {
        match pending
            .receiver
            .try_recv()
            .map_err(|e| e.into_bevy_error())?
        {
            JobRead::Item(reply) => pending.reply = Some(reply),
            JobRead::Finished(outcome) => break outcome,
            JobRead::Empty => {
                world.resource_mut::<PromptStore>().pending = Some(pending);
                return Ok(());
            }
            JobRead::Closed => return Err(io::Error::other("提示词库任务缺少终态").into()),
        }
    };
    world
        .resource::<JobManager>()
        .remove(pending.id)
        .map_err(|e| e.into_bevy_error())?;
    world.resource_mut::<PromptState>().busy = false;
    match outcome {
        JobOutcome::Completed => {
            let reply = pending
                .reply
                .ok_or_else(|| io::Error::other("提示词库任务缺少结果"))?;
            match &reply.result {
                Ok(library) => {
                    let mut store = world.resource_mut::<PromptStore>();
                    store.library = library.clone();
                    if let Some(reference) = &reply.selected {
                        let entry = library.find(reference)?;
                        store.selected = Some(reference.clone());
                        store.editor.name = entry.name.clone();
                        store.editor.text = entry.text.clone();
                        store.editor.language = entry.language.clone();
                    }
                    let mut state = world.resource_mut::<PromptState>();
                    state.ready = true;
                    state.status = "提示词库已就绪；保存的版本与组合可在下次启动复用".into();
                }
                Err(failure) => {
                    world.resource_mut::<PromptState>().status =
                        format!("提示词库操作失败：{failure}")
                }
            }
        }
        JobOutcome::Failed(failure) => {
            error!(error = %failure, "提示词库任务失败");
            world.resource_mut::<PromptState>().status = failure.to_string();
        }
        JobOutcome::Canceled => {
            world.resource_mut::<PromptState>().status = "提示词库任务已取消".into();
        }
    }
    refresh(world)
}
