use crate::conversation::ChatState;
use agentelier_conversation_bridge::{ProjectCollectionStatus, ProjectContextView};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::UiSystems;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField};
use bevy_widgetry_path_display::display_path;

const GAP: f32 = 8.0;
const MIN_BODY_ROWS: usize = 3;
const MAX_BODY_ROWS: usize = 12;

#[derive(Clone, Default, Reflect, PartialEq, Eq)]
pub enum CollectionStatus {
    #[default]
    NotCollected,
    Loaded,
    Failed(String),
}
#[derive(Clone, Default, Reflect, PartialEq, Eq)]
pub struct ProjectContext {
    pub status: CollectionStatus,
    pub snapshot: Option<ProjectSnapshot>,
}
#[derive(Clone, Reflect, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub working_directory: String,
    pub project_root: Option<String>,
    pub source_bytes: usize,
    pub hash: String,
    pub sources: Vec<RuleSource>,
}
#[derive(Clone, Reflect, PartialEq, Eq)]
pub struct RuleSource {
    pub path: String,
    pub scope: String,
    pub text: String,
    pub hash: String,
}

pub(crate) fn project(view: ProjectContextView) -> ProjectContext {
    ProjectContext {
        status: match view.status {
            ProjectCollectionStatus::NotCollected => CollectionStatus::NotCollected,
            ProjectCollectionStatus::Loaded => CollectionStatus::Loaded,
            ProjectCollectionStatus::Failed(message) => CollectionStatus::Failed(message),
        },
        snapshot: view.snapshot.map(|snapshot| ProjectSnapshot {
            working_directory: display_path(&snapshot.working_directory),
            project_root: snapshot.project_root.as_deref().map(display_path),
            source_bytes: snapshot.source_bytes,
            hash: snapshot.hash,
            sources: snapshot
                .sources
                .into_iter()
                .map(|source| RuleSource {
                    path: display_path(&source.path),
                    scope: display_path(&source.scope),
                    text: source.text,
                    hash: source.hash,
                })
                .collect(),
        }),
    }
}

#[derive(Component, Clone, Default)]
struct ProjectSummary;
#[derive(Component, Clone, Default)]
struct ToggleLabel;
#[derive(Component, Clone, Default)]
struct ProjectSources {
    rendered: Option<(agentelier_conversation_bridge::SessionId, ProjectContext)>,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(PostUpdate, synchronize.before(UiSystems::Content));
}

pub(crate) fn panel(state: &ChatState) -> impl Scene {
    let summary = summary(
        state
            .session
            .as_ref()
            .map(|session| &session.project_context),
    );
    let label = toggle_label(state.project_rules_expanded);
    bsn! {
        Name("ProjectContextInspection")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Text(summary) WidgetryText ProjectSummary Name("ProjectContextStatus") TextLayout::linebreak(LineBreak::AnyCharacter)--
            @WidgetryButton { @children: {Some(Box::new(bsn_list! {Text(label) WidgetryText ToggleLabel}) as Box<dyn SceneList>)} }
            Name("ToggleProjectRules") Node { align_self: AlignSelf::Start } on(toggle)--
            ProjectSources::default() Name("ProjectRuleSources")
            Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        ]
    }
}

fn toggle_label(expanded: bool) -> &'static str {
    if expanded {
        "收起项目规则原文"
    } else {
        "展开项目规则原文"
    }
}
fn toggle(_: On<Activate>, mut state: ResMut<ChatState>) {
    state.project_rules_expanded = !state.project_rules_expanded;
}

fn summary(project: Option<&ProjectContext>) -> String {
    let Some(project) = project else {
        return "项目规则：尚未创建会话".into();
    };
    let mut text = match &project.status {
        CollectionStatus::NotCollected => "项目规则：尚未采集；请求准备前读取".into(),
        CollectionStatus::Loaded => {
            if project.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .sources
                    .iter()
                    .any(|source| !source.text.trim().is_empty())
            }) {
                "项目规则：最近采集成功；请求材料以已保存的准备记录为准".into()
            } else {
                "项目规则：采集成功，当前规则为空，不产生空规则消息".into()
            }
        }
        CollectionStatus::Failed(message) => {
            format!("项目规则：采集失败，本次模型请求未启动\n{message}")
        }
    };
    if let Some(snapshot) = &project.snapshot {
        if matches!(project.status, CollectionStatus::Failed(_)) {
            text.push_str("\n以下仅为上次成功采集的材料，本次未重新确认");
        }
        text.push_str(&format!(
            "\n会话目录：{}\n项目边界：{}\n来源 {} 份，原始字节 {}，内容摘要 {}",
            snapshot.working_directory,
            snapshot
                .project_root
                .as_deref()
                .unwrap_or("未发现 .git，仅检查会话目录"),
            snapshot.sources.len(),
            snapshot.source_bytes,
            snapshot.hash
        ));
    }
    text
}

fn source_scene(source: RuleSource, index: usize) -> Box<dyn Scene> {
    let metadata = format!(
        "{}. {}\n作用目录：{}\n内容摘要：{}{}",
        index + 1,
        source.path,
        source.scope,
        source.hash,
        if source.text.trim().is_empty() {
            "\n空白文件，不形成规则正文"
        } else {
            ""
        }
    );
    Box::new(bsn! {
        Name(format!("ProjectRuleSource-{index}"))
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Text(metadata) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
            @WidgetryReadOnlyTextField {
                @value: {source.text}, @multiline: true,
                @auto_size: {Some(AutoSize { min_rows: MIN_BODY_ROWS, max_rows: MAX_BODY_ROWS })},
            }
            Name(format!("ProjectRuleBody-{index}")) Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
        ]
    })
}

fn synchronize(world: &mut World) -> Result {
    if world
        .query_filtered::<Entity, With<ProjectSources>>()
        .iter(world)
        .next()
        .is_none()
    {
        return Ok(());
    }
    let mounted = world
        .query::<Ref<ProjectSources>>()
        .iter(world)
        .any(|sources| sources.is_added());
    let state = world.resource_ref::<ChatState>();
    if !state.is_changed() && !mounted {
        return Ok(());
    }
    let expanded = state.project_rules_expanded;
    let summary = summary(
        state
            .session
            .as_ref()
            .map(|session| &session.project_context),
    );
    let current = state
        .session
        .as_ref()
        .map(|session| (session.id, session.project_context.clone()));
    let mut summaries = world.query_filtered::<&mut Text, With<ProjectSummary>>();
    for mut text in summaries.iter_mut(world) {
        text.set_if_neq(Text(summary.clone()));
    }
    let mut labels = world.query_filtered::<&mut Text, With<ToggleLabel>>();
    for mut text in labels.iter_mut(world) {
        text.set_if_neq(Text(toggle_label(expanded).into()));
    }
    let mut roots = world.query::<(Entity, &ProjectSources)>();
    let roots: Vec<_> = roots
        .iter(world)
        .map(|(entity, sources)| (entity, sources.rendered != current))
        .collect();
    for (root, changed) in roots {
        world
            .get_mut::<Node>(root)
            .ok_or_else(|| std::io::Error::other("项目来源缺少布局"))?
            .display = if expanded {
            Display::Flex
        } else {
            Display::None
        };
        if !changed {
            continue;
        }
        let children = world
            .get::<Children>(root)
            .map(|children| children.to_vec())
            .unwrap_or_default();
        for child in children {
            world.entity_mut(child).despawn();
        }
        if let Some((_, context)) = &current
            && let Some(snapshot) = &context.snapshot
        {
            for (index, source) in snapshot.sources.iter().enumerate() {
                let scene = source_scene(source.clone(), index);
                world
                    .commands()
                    .spawn_scene_with_error_handler(bsn! { ChildOf(root) @scene });
            }
        }
        world
            .get_mut::<ProjectSources>(root)
            .ok_or_else(|| std::io::Error::other("项目来源缺少展示状态"))?
            .rendered = current.clone();
    }
    Ok(())
}
