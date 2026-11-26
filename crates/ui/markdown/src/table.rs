use crate::parse::{Cell, Table};
use bevy::prelude::*;
use bevy::text::Justify;
use bevy::ui::UiSystems;
use bevy::window::RequestRedraw;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::{ScrollAxis, WidgetryScrollArea};
use pulldown_cmark::Alignment;

const COLUMN_MIN_WIDTH: f32 = 180.0;
const CELL_PADDING: f32 = 10.0;
const CELL_BORDER: f32 = 1.0;
const CELL_BORDER_COLOR: Color = Color::srgb(0.45, 0.5, 0.6);
const CELL_GAP: f32 = 6.0;
const SCROLLBAR_SPACE: f32 = 16.0;
const INITIAL_HEIGHT: f32 = 100.0;

#[derive(Component, Clone, Default)]
struct TableScroll;
#[derive(Component, Clone, Default)]
struct TableGrid;
#[derive(Component, Clone)]
struct TableCell {
    row: usize,
    column: usize,
    cell: Cell,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        fit_height
            .after(UiSystems::Layout)
            .before(UiSystems::PostLayout),
    );
}

fn fit_height(
    grids: Query<(Entity, &ComputedNode), With<TableGrid>>,
    parents: Query<&ChildOf>,
    mut roots: Query<&mut Node, With<TableScroll>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    for (grid, computed) in &grids {
        for ancestor in parents.iter_ancestors(grid) {
            if let Ok(mut root) = roots.get_mut(ancestor) {
                let height = px(computed.size.y * computed.inverse_scale_factor + SCROLLBAR_SPACE);
                if root.height != height {
                    root.height = height;
                    redraw.write(RequestRedraw);
                }
                break;
            }
        }
    }
}

pub(crate) fn scene(table: &Table) -> Box<dyn Scene> {
    let minimum = table.alignments.len() as f32 * COLUMN_MIN_WIDTH;
    let tracks: Vec<_> = table
        .alignments
        .iter()
        .map(|_| {
            RepeatedGridTrack::minmax(
                1,
                MinTrackSizingFunction::Px(COLUMN_MIN_WIDTH),
                MaxTrackSizingFunction::Fraction(1.0),
            )
        })
        .collect();
    let cells: Vec<Box<dyn Scene>> = table
        .rows
        .iter()
        .enumerate()
        .flat_map(|(row, cells)| {
            cells
                .iter()
                .enumerate()
                .map(move |(column, cell)| cell_scene(row, column, cell, table.alignments[column]))
        })
        .collect();
    Box::new(bsn! {
        @WidgetryScrollArea {
            @axis: ScrollAxis::Horizontal,
            @content: {Some(Box::new(bsn! { on(crate::code::forward_vertical) }) as Box<dyn Scene>)},
            @children: {Some(Box::new(bsn_list! {
                TableGrid Name("MarkdownTableGrid")
                Node { display: Display::Grid, width: percent(100), min_width: px(minimum), flex_shrink: 0.0, align_self: AlignSelf::Start, grid_template_columns: tracks }
                Children [{cells}]
            }) as Box<dyn SceneList>)}
        }
        TableScroll Name("MarkdownTableScroll") Node { width: percent(100), min_width: px(0), height: px(INITIAL_HEIGHT), flex_shrink: 0.0 }
    })
}

fn cell_scene(row: usize, column: usize, cell: &Cell, alignment: Alignment) -> Box<dyn Scene> {
    let links: Vec<_> = cell
        .links
        .iter()
        .map(|link| crate::links::scene(link))
        .collect();
    let cell = cell.clone();
    let justify = match alignment {
        Alignment::Center => Justify::Center,
        Alignment::Right => Justify::Right,
        _ => Justify::Left,
    };
    let paragraph = crate::view::text(&cell.runs, crate::view::BODY_SIZE, row == 0, justify);
    Box::new(bsn! {
        Name(format!("MarkdownTableCell-{row}-{column}"))
        template(move |_| Ok(TableCell { row, column, cell: cell.clone() }))
        Node { min_width: px(0), flex_direction: FlexDirection::Column, padding: UiRect::all(px(CELL_PADDING)), border: UiRect::all(px(CELL_BORDER)), row_gap: px(CELL_GAP) }
        BorderColor::all(CELL_BORDER_COLOR)
        Children [
            @paragraph--
            {links}
        ]
    })
}

pub(crate) fn update(world: &mut World, root: Entity, table: &Table) -> Result {
    let grid = world
        .query_filtered::<Entity, With<TableGrid>>()
        .iter(world)
        .find(|entity| {
            let mut current = *entity;
            while let Some(parent) = world.get::<ChildOf>(current) {
                current = parent.parent();
                if current == root {
                    return true;
                }
            }
            false
        })
        .ok_or_else(|| std::io::Error::other("表格缺少网格"))?;
    let existing: Vec<_> = world
        .query::<(Entity, &TableCell, &ChildOf)>()
        .iter(world)
        .filter(|(_, _, parent)| parent.parent() == grid)
        .map(|(entity, cell, _)| (entity, cell.clone()))
        .collect();
    let mut ordered = Vec::new();
    for (row, cells) in table.rows.iter().enumerate() {
        for (column, cell) in cells.iter().enumerate() {
            if let Some((entity, _)) = existing
                .iter()
                .find(|(_, old)| old.row == row && old.column == column && old.cell == *cell)
            {
                ordered.push(*entity);
            } else {
                let scene = cell_scene(row, column, cell, table.alignments[column]);
                ordered.push(
                    world
                        .commands()
                        .spawn_scene_with_error_handler(bsn! { ChildOf(grid) @{scene} })
                        .id(),
                );
            }
        }
    }
    for (entity, _) in existing {
        if !ordered.contains(&entity) {
            world.entity_mut(entity).despawn();
        }
    }
    world.commands().entity(grid).replace_children(&ordered);
    Ok(())
}
