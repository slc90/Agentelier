use crate::parse::Formula;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::text::LineBreak;
use bevy::ui::{ComputedUiRenderTargetInfo, UiSystems, widget::NodeImageMode};
use bevy::ui_widgets::Activate;
use bevy::window::RequestRedraw;
use bevy_widgetry::scroll_area::{ScrollAxis, WidgetryScrollArea};
use bevy_widgetry::{text::WidgetryText, theme::WidgetryTheme};
use latex_rust::{Dim, MathFont, MathStyle, SvgOptions};
use resvg::{tiny_skia, usvg};
use std::sync::OnceLock;

pub(crate) mod inline;
mod notation;

const DISPLAY_EM_PT: i64 = 15;
const SVG_PIXELS_PER_POINT: f32 = 96.0 / 72.0;
const MAX_FORMULA_BYTES: usize = 16 * 1024;
const MAX_RASTER_EDGE: f32 = 8192.0;
const MAX_RASTER_AREA: f32 = 4_000_000.0;
const MAX_JOBS: usize = 2;
const SCROLLBAR_SPACE: f32 = 12.0;

#[derive(Component, Clone)]
#[require(FormulaState)]
struct DisplayFormula(Formula);

#[derive(Component, Default)]
struct FormulaState {
    style: Option<RasterStyle>,
    job: Option<Task<Result<FormulaRaster, String>>>,
    metrics: Option<FormulaMetrics>,
}

#[derive(Component, Clone, Copy, Default)]
enum FormulaPart {
    #[default]
    Scroll,
    Image,
    Source,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
struct RasterStyle {
    scale: f32,
    ink: [u8; 3],
}

struct FormulaRaster {
    image: Image,
    metrics: FormulaMetrics,
}

#[derive(Clone, Copy)]
struct FormulaMetrics {
    width: f32,
    ascent: f32,
    descent: f32,
}

impl FormulaMetrics {
    fn height(self) -> f32 {
        self.ascent + self.descent
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(PostUpdate, refresh.after(UiSystems::PostLayout));
    inline::install(app);
}

pub(crate) fn scene(formula: Formula) -> impl Scene {
    let original = formula.original.clone();
    bsn! {
        Name("MarkdownDisplayMath") template(move |_| Ok(DisplayFormula(formula.clone())))
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, align_items: AlignItems::Start, flex_shrink: 0.0, row_gap: px(4) }
        Children [
            @WidgetryScrollArea {
                @axis: ScrollAxis::Horizontal,
                @scrollbar_thickness: SCROLLBAR_SPACE,
                @content: {Some(Box::new(bsn! { on(crate::code::forward_vertical) }) as Box<dyn Scene>)},
                @children: {Some(Box::new(bsn_list! {
                    Name("MarkdownFormulaImage") FormulaPart::Image
                    ImageNode { image_mode: NodeImageMode::Stretch, color: Color::NONE }
                    Node { width: px(0), height: px(0), flex_shrink: 0.0 }
                }) as Box<dyn SceneList>)}
            }
            Name("MarkdownFormulaScroll") FormulaPart::Scroll
            Node { display: Display::None, width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            Name("MarkdownFormulaSource") FormulaPart::Source Text(original) WidgetryText
            TextLayout::linebreak(LineBreak::AnyCharacter) Node { width: percent(100), min_width: px(0) }--
            Name("MarkdownFormulaError") FormulaPart::Error Text("公式无法排版") WidgetryText
            Node { display: Display::None }--
            @crate::source::copy_button("复制公式", "MarkdownCopyFormula") on(copy)
        ]
    }
}

fn copy(event: On<Activate>, mut commands: Commands) {
    let button = event.entity;
    commands.queue(move |world: &mut World| -> Result {
        let mut root = button;
        let formula = loop {
            if let Some(formula) = world.get::<DisplayFormula>(root) {
                break formula;
            }
            root = world
                .get::<ChildOf>(root)
                .ok_or_else(|| std::io::Error::other("复制公式缺少原文"))?
                .parent();
        };
        let original = formula.0.original.clone();
        crate::source::copy(world, button, &original, "公式")
    });
}

fn refresh(
    mut formulas: Query<(
        Entity,
        &DisplayFormula,
        &mut FormulaState,
        &ComputedNode,
        &ComputedUiRenderTargetInfo,
    )>,
    theme: Res<WidgetryTheme>,
    children: Query<&Children>,
    mut parts: Query<(&FormulaPart, &mut Node, Option<&mut ImageNode>)>,
    mut images: ResMut<Assets<Image>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let mut busy = formulas
        .iter()
        .filter(|(_, _, state, ..)| state.job.is_some())
        .count();
    let ink = theme
        .text
        .normal
        .foreground
        .to_srgba()
        .to_u8_array_no_alpha();
    for (entity, formula, mut state, node, target) in &mut formulas {
        let style = RasterStyle {
            scale: target.scale_factor(),
            ink,
        };
        let mut finished = None;
        if let Some(job) = &mut state.job
            && let Some(result) = check_ready(job)
        {
            state.job = None;
            busy -= 1;
            finished = Some(result);
        }

        if let Some(result) = finished {
            let rendered = match result {
                Ok(raster) => {
                    state.metrics = Some(raster.metrics);
                    Some(images.add(raster.image))
                }
                Err(error) => {
                    tracing::debug!(%error, "数学公式排版回退到原文");
                    state.metrics = None;
                    None
                }
            };
            for child in children.iter_descendants(entity) {
                let Ok((part, mut node, image)) = parts.get_mut(child) else {
                    continue;
                };
                match part {
                    FormulaPart::Image if let Some(mut image) = image => {
                        if let (Some(handle), Some(metrics)) = (&rendered, state.metrics) {
                            image.image = handle.clone();
                            image.color = Color::WHITE;
                            node.width = px(metrics.width);
                            node.height = px(metrics.height());
                        } else {
                            *image = ImageNode::default();
                        }
                    }
                    FormulaPart::Scroll => {
                        node.display = if rendered.is_some() {
                            Display::Grid
                        } else {
                            Display::None
                        }
                    }
                    FormulaPart::Source | FormulaPart::Error => {
                        node.display = if rendered.is_some() {
                            Display::None
                        } else {
                            Display::Flex
                        }
                    }
                    FormulaPart::Image => {}
                }
            }
            redraw.write(RequestRedraw);
        }

        if let Some(metrics) = state.metrics {
            let width = node.size().x * node.inverse_scale_factor;
            let scroll = if metrics.width > width {
                SCROLLBAR_SPACE
            } else {
                0.0
            };
            for child in children.iter_descendants(entity) {
                if let Ok((FormulaPart::Scroll, node, _)) = parts.get_mut(child) {
                    if node
                        .map_unchanged(|node| &mut node.height)
                        .set_if_neq(px(metrics.height() + scroll))
                    {
                        redraw.write(RequestRedraw);
                    }
                    break;
                }
            }
        }

        if state.job.is_none()
            && state.style != Some(style)
            && busy < MAX_JOBS
            && target.physical_size() != UVec2::ZERO
        {
            let latex = formula.0.latex.clone();
            state.style = Some(style);
            state.job = Some(AsyncComputeTaskPool::get().spawn(async move {
                rasterize(
                    &latex,
                    style,
                    MathStyle::Display,
                    Dim::from_i64(DISPLAY_EM_PT),
                )
            }));
            busy += 1;
        }
    }
    if busy != 0 {
        redraw.write(RequestRedraw);
    }
}

fn rasterize(
    latex: &str,
    style: RasterStyle,
    math_style: MathStyle,
    em: Dim,
) -> Result<FormulaRaster, String> {
    static FONT: OnceLock<Result<MathFont, String>> = OnceLock::new();
    if latex.trim().is_empty() || latex.len() > MAX_FORMULA_BYTES {
        return Err("公式正文为空或过长".into());
    }
    let font = FONT
        .get_or_init(|| MathFont::stix_two_math().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(Clone::clone)?;
    let ast = notation::parse(latex)?;
    let layout = latex_rust::layout_with_em_size_pt(&ast, font, math_style, &em)
        .map_err(|error| error.to_string())?;
    let [red, green, blue] = style.ink;
    let options = SvgOptions {
        font_size_pt: em,
        color: latex_rust::Color::rgb(red, green, blue),
        display: math_style == MathStyle::Display,
    };
    let vector = latex_rust::render_svg_with_metrics(&layout, font, &options)
        .map_err(|error| error.to_string())?;
    let metrics = FormulaMetrics {
        width: points_to_pixels(&vector.width_pt),
        ascent: points_to_pixels(&vector.baseline_pt),
        descent: points_to_pixels(&(&vector.height_pt - &vector.baseline_pt)),
    };
    let svg = usvg::Tree::from_str(&vector.svg, &usvg::Options::default())
        .map_err(|error| error.to_string())?;
    let size = Vec2::new(svg.size().width(), svg.size().height());
    let scale = style
        .scale
        .min(MAX_RASTER_EDGE / size.max_element())
        .min((MAX_RASTER_AREA / (size.x * size.y)).sqrt());
    let width = (size.x * scale).ceil() as u32;
    let height = (size.y * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("无法分配公式显示图像")?;
    resvg::render(
        &svg,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let bytes = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    let image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    Ok(FormulaRaster { image, metrics })
}

fn points_to_pixels(value: &Dim) -> f32 {
    f32::from_bits(value.to_ieee32_bits()) * SVG_PIXELS_PER_POINT
}
