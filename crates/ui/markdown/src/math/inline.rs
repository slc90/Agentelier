use super::{FormulaMetrics, FormulaRaster, MAX_JOBS, RasterStyle, rasterize};
use crate::parse::Formula;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::text::{InlineBox, InlineBoxKind, LineHeight, TextLayoutInfo};
use bevy::ui::{ComputedUiRenderTargetInfo, UiSystems, widget::NodeImageMode};
use bevy::window::RequestRedraw;
use bevy_widgetry::theme::WidgetryTheme;
use latex_rust::{Dim, MathStyle};

#[derive(Component, Clone)]
#[require(ParagraphState)]
pub(crate) struct Paragraph {
    pub(crate) size: f32,
    pub(crate) formulas: Vec<Formula>,
}

#[derive(Component, Clone)]
pub(crate) struct Source(pub(crate) usize);

#[derive(Component, Clone)]
struct Paint(usize);

#[derive(Component, Default)]
struct ParagraphState {
    style: Option<RasterStyle>,
    job: Option<Task<Vec<Result<FormulaRaster, String>>>>,
    metrics: Vec<Option<FormulaMetrics>>,
    fitted: Vec<Option<FormulaMetrics>>,
    width: f32,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        refresh
            .after(crate::view::render)
            .before(UiSystems::Content),
    )
    .add_systems(PostUpdate, position.after(UiSystems::PostLayout));
}

pub(crate) fn image(index: usize) -> impl Scene {
    bsn! {
        Name("MarkdownInlineFormulaImage") template(move |_| Ok(Paint(index)))
        ImageNode { image_mode: NodeImageMode::Stretch, color: Color::NONE }
        Node { position_type: PositionType::Absolute, display: Display::None, left: px(0), top: px(0), width: px(0), height: px(0) }
        Pickable::IGNORE
    }
}

fn refresh(
    mut paragraphs: Query<(
        &Paragraph,
        &mut ParagraphState,
        &Children,
        &ChildOf,
        &ComputedNode,
        &ComputedUiRenderTargetInfo,
        &mut Node,
    )>,
    siblings: Query<&Children>,
    mut spans: Query<&mut LineHeight>,
    sources: Query<(&Source, Option<&InlineBox>)>,
    mut paints: Query<(&Paint, &mut ImageNode)>,
    theme: Res<WidgetryTheme>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let ink = theme
        .text
        .normal
        .foreground
        .to_srgba()
        .to_u8_array_no_alpha();
    let mut busy = paragraphs
        .iter()
        .filter(|(_, state, ..)| state.job.is_some())
        .count();
    for (paragraph, mut state, children, parent, computed, target, mut node) in &mut paragraphs {
        let style = RasterStyle {
            scale: target.scale_factor(),
            ink,
        };
        let mut update = false;
        if let Some(job) = &mut state.job
            && let Some(results) = check_ready(job)
        {
            state.job = None;
            busy -= 1;
            let rendered = results
                .into_iter()
                .map(|result| match result {
                    Ok(raster) => Some((raster.metrics, images.add(raster.image))),
                    Err(error) => {
                        tracing::debug!(%error, "行内公式排版回退到原文");
                        None
                    }
                })
                .collect::<Vec<_>>();
            for child in siblings
                .get(parent.parent())
                .into_iter()
                .flat_map(|children| children.iter())
            {
                if let Ok((paint, mut image)) = paints.get_mut(child) {
                    if let Some(Some((_, handle))) = rendered.get(paint.0) {
                        image.image = handle.clone();
                        image.color = Color::WHITE;
                    } else {
                        *image = ImageNode::default().with_color(Color::NONE);
                    }
                }
            }
            state.metrics = rendered
                .into_iter()
                .map(|result| result.map(|(metrics, _)| metrics))
                .collect();
            update = true;
        }

        let width = computed.content_box().size().x * computed.inverse_scale_factor;
        if width > 0.0 && (update || state.width != width && !state.metrics.is_empty()) {
            state.width = width;
            state.fitted = state
                .metrics
                .iter()
                .map(|metrics| metrics.map(|metrics| fitted(metrics, width)))
                .collect();
            let (ascent, descent) = state.fitted.iter().flatten().fold(
                (0.0_f32, 0.0_f32),
                |(ascent, descent), metrics| {
                    (ascent.max(metrics.ascent), descent.max(metrics.descent))
                },
            );
            // InlineBox reserves space above the baseline. Keep clearance below it even on lines without text.
            let line_height = if descent > 0.0 {
                let text_ascent = paragraph.size * 1.15;
                let text_descent = paragraph.size * 0.30;
                LineHeight::Px(
                    ((ascent + descent).max(text_ascent) + text_descent + 2.0 * descent).ceil(),
                )
            } else {
                LineHeight::default()
            };
            node.padding.bottom = px(descent.ceil());
            for child in children.iter() {
                if let Ok(mut height) = spans.get_mut(child) {
                    height.set_if_neq(line_height);
                }
                if let Ok((source, current)) = sources.get(child) {
                    let next = state
                        .fitted
                        .get(source.0)
                        .copied()
                        .flatten()
                        .map(|metrics| InlineBox {
                            kind: InlineBoxKind::InFlow,
                            size: Vec2::new(metrics.width, metrics.ascent + descent),
                        });
                    if current != next.as_ref() {
                        let mut entity = commands.entity(child);
                        if let Some(next) = next {
                            entity.try_insert(next);
                        } else {
                            entity.try_remove::<InlineBox>();
                        }
                    }
                }
            }
            redraw.write(RequestRedraw);
        }

        if state.style != Some(style)
            && state.job.is_none()
            && busy < MAX_JOBS
            && target.physical_size() != UVec2::ZERO
        {
            state.style = Some(style);
            let math = paragraph.formulas.clone();
            let em = Dim::ratio((paragraph.size * 3.0) as i64, 4);
            state.job = Some(AsyncComputeTaskPool::get().spawn(async move {
                math.iter()
                    .map(|formula| rasterize(&formula.latex, style, MathStyle::Text, em.clone()))
                    .collect()
            }));
            busy += 1;
        }
    }
    if busy != 0 {
        redraw.write(RequestRedraw);
    }
}

fn fitted(metrics: FormulaMetrics, width: f32) -> FormulaMetrics {
    let scale = (width / metrics.width).min(1.0);
    FormulaMetrics {
        width: metrics.width * scale,
        ascent: metrics.ascent * scale,
        descent: metrics.descent * scale,
    }
}

fn position(
    paragraphs: Query<(&ParagraphState, &TextLayoutInfo, &ChildOf)>,
    siblings: Query<&Children>,
    sources: Query<&Source>,
    mut paints: Query<(&Paint, &mut Node)>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    for (state, layout, parent) in &paragraphs {
        let Ok(children) = siblings.get(parent.parent()) else {
            continue;
        };
        let scale = layout.scale_factor.max(f32::EPSILON);
        for child in children.iter() {
            let Ok((paint, mut node)) = paints.get_mut(child) else {
                continue;
            };
            let placement = state
                .fitted
                .get(paint.0)
                .copied()
                .flatten()
                .and_then(|metrics| {
                    layout
                        .inline_boxes
                        .iter()
                        .find(|(entity, _, _)| {
                            sources.get(*entity).is_ok_and(|source| source.0 == paint.0)
                        })
                        .map(|(_, _, bounds)| (metrics, bounds))
                });
            if let Some((metrics, bounds)) = placement {
                let left = px(bounds.min.x / scale);
                let top = px(bounds.max.y / scale - metrics.ascent);
                let width = px(metrics.width);
                let height = px(metrics.height());
                if node.display != Display::Flex
                    || node.left != left
                    || node.top != top
                    || node.width != width
                    || node.height != height
                {
                    node.display = Display::Flex;
                    node.left = left;
                    node.top = top;
                    node.width = width;
                    node.height = height;
                    redraw.write(RequestRedraw);
                }
            } else if node.display != Display::None {
                node.display = Display::None;
                redraw.write(RequestRedraw);
            }
        }
    }
}
