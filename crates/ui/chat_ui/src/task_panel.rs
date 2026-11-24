use crate::{
    conversation::ChatState, global_tasks::GlobalTaskState, workspace::TASK_PANEL_BACKGROUND,
};
use agentelier_conversation_bridge::SessionId;
use bevy::{prelude::*, ui::UiSystems};
use bevy_widgetry::list_view::{
    WidgetryListModel, WidgetryListView, WidgetryListViewAppExt, WidgetryListViewColorOverrides,
    WidgetryListViewContainerColorOverrides, WidgetryListViewContainerStateColorOverrides,
    WidgetryListViewItemColorOverrides, WidgetryListViewItemStateColorOverrides,
    WidgetryListViewRenderer,
};
use bevy_widgetry::text::WidgetryText;
use std::collections::HashSet;

#[derive(Resource)]
struct TaskSource(Entity);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RowId {
    Session(SessionId),
    Task(SessionId),
}

#[derive(PartialEq, Eq)]
enum Row {
    Session {
        id: SessionId,
        title: String,
        current: bool,
    },
    Task {
        id: SessionId,
        name: String,
        status: &'static str,
    },
}

impl Row {
    fn id(&self) -> RowId {
        match self {
            Self::Session { id, .. } => RowId::Session(*id),
            Self::Task { id, .. } => RowId::Task(*id),
        }
    }

    fn content(&self) -> impl SceneList + use<> {
        let (entity_name, label, status) = match self {
            Self::Session { id, title, current } => (
                format!("GlobalTaskGroup-{id}"),
                if *current {
                    format!("● {title}")
                } else {
                    title.clone()
                },
                None,
            ),
            Self::Task { id, name, status } => {
                (format!("GlobalTaskRow-{id}"), name.clone(), Some(*status))
            }
        };
        let indent = if status.is_some() { 12.0 } else { 0.0 };
        let status = status.map(|status| {
            bsn! {
                Text(status) WidgetryText TextFont { font_size: bevy::text::FontSize::Px(17.0) } TextLayout::no_wrap()
                Node { flex_shrink: 0.0 }
            }
        });
        bsn_list! {
            Name(entity_name)
            Node { width: percent(100), min_width: px(0), padding: UiRect::left(px(indent)), align_items: AlignItems::Center, column_gap: px(8) }
            Children [
                Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }
                Children [ Text(label) WidgetryText TextFont { font_size: bevy::text::FontSize::Px(17.0) } TextLayout::no_wrap() ]--
                @status
            ]
        }
    }
}

pub(crate) fn install(app: &mut App) -> Result {
    app.register_widgetry_list_view::<Row>()
        .map_err(|failure| failure.into_bevy_error())?;
    let source = app
        .world_mut()
        .spawn(WidgetryListModel::<Row>::default())
        .id();
    app.insert_resource(TaskSource(source))
        .add_systems(PostUpdate, reconcile.before(UiSystems::Content));
    Ok(())
}

pub(crate) fn panel(world: &World) -> impl Scene + use<> {
    let source = world.resource::<TaskSource>().0;
    bsn! {
        @WidgetryListView::<Row> {
            @source: source,
            @item_height: 30.0,
            @colors: {WidgetryListViewColorOverrides {
                container: WidgetryListViewContainerColorOverrides {
                    normal: WidgetryListViewContainerStateColorOverrides {
                        background: Some(TASK_PANEL_BACKGROUND),
                        ..default()
                    },
                    focused: WidgetryListViewContainerStateColorOverrides {
                        background: Some(TASK_PANEL_BACKGROUND),
                        ..default()
                    },
                    ..default()
                },
                item: WidgetryListViewItemColorOverrides {
                    disabled: WidgetryListViewItemStateColorOverrides {
                        foreground: Some(Color::srgb_u8(0x45, 0x66, 0x56)),
                        ..default()
                    },
                    ..default()
                },
            }},
            @renderer: {WidgetryListViewRenderer::new(Row::content)},
        }
        Name("GlobalTaskList")
        Node { width: percent(100), min_height: px(0), flex_grow: 1.0 }
    }
}

fn reconcile(
    state: Res<GlobalTaskState>,
    chat: Res<ChatState>,
    source: Res<TaskSource>,
    mut active: Local<Option<SessionId>>,
    mut models: Query<&mut WidgetryListModel<Row>>,
) -> Result {
    let current = chat.session.as_ref().map(|session| session.id);
    if !state.is_changed() && *active == current {
        return Ok(());
    }
    *active = current;
    let rows: Vec<_> = state
        .groups
        .iter()
        .filter(|group| !group.tasks.is_empty())
        .flat_map(|group| {
            std::iter::once(Row::Session {
                id: group.session,
                title: group.title.clone(),
                current: current == Some(group.session),
            })
            .chain(group.tasks.iter().map(|task| Row::Task {
                id: task.id,
                name: task.name.clone(),
                status: task.status,
            }))
        })
        .collect();

    let mut model = models.get_mut(source.0)?;
    let ids: HashSet<_> = rows.iter().map(Row::id).collect();
    for index in (0..model.len()).rev() {
        if model.get(index).is_some_and(|row| !ids.contains(&row.id())) {
            model.remove(index);
        }
    }
    for (index, row) in rows.into_iter().enumerate() {
        let id = row.id();
        if let Some(old) =
            (index..model.len()).find(|&old| model.get(old).is_some_and(|row| row.id() == id))
        {
            if old != index {
                model.move_item(old, index);
            }
            if model.get(index) != Some(&row)
                && let Some(entry) = model
                    .get_mut(index)
                    .map_err(|failure| failure.into_bevy_error())?
            {
                *entry = row;
            }
        } else {
            model
                .insert(index, row)
                .map_err(|failure| failure.into_bevy_error())?;
            model.set_disabled(index, true);
        }
    }
    Ok(())
}
