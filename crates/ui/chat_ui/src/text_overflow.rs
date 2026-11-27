use bevy::prelude::*;
use bevy::text::{
    ComputedTextBlock, FontCx, LayoutCx, LetterSpacing, LineBreak, LineHeight, RemSize,
    TextElement, TextError, TextPipeline,
};
use bevy::ui::{ComputedUiRenderTargetInfo, UiSystems};
use bevy_widgetry::scene::WidgetrySceneEntityCommandsExt;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::tooltip::{
    TooltipContentFactory, WidgetryTooltip, WidgetryTooltipHeadless, WidgetryTooltipPlugin,
};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

const ELLIPSIS: &str = "…";
const TOOLTIP_MAX_WIDTH: f32 = 480.0;

#[derive(Component, Clone, Default)]
pub(crate) struct EllipsizedText {
    full: String,
    fitted_width: Option<f32>,
}

impl EllipsizedText {
    pub(crate) fn full(&self) -> &str {
        &self.full
    }

    pub(crate) fn replace(&mut self, value: &str, displayed: &mut Text) {
        self.full.clear();
        self.full.push_str(value);
        self.fitted_width = None;
        displayed.0.clone_from(&self.full);
    }
}

pub(crate) fn text(title: String, elide: bool) -> impl Scene {
    let fitting = elide.then(|| {
        let full = title.clone();
        bsn! { template(move |_| Ok(EllipsizedText { full: full.clone(), fitted_width: None })) }
    });
    bsn! {
        Text(title) WidgetryText TextLayout::no_wrap() Pickable::IGNORE
        @{fitting}
    }
}

pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<WidgetryTooltipPlugin>() {
        app.add_plugins(WidgetryTooltipPlugin);
    }
    app.add_systems(PostUpdate, fit_labels.after(UiSystems::Layout));
}

fn tooltip(full: String) -> impl Scene {
    let content = TooltipContentFactory::new(move || {
        let full = full.clone();
        bsn_list! {
            Name("FullLabelTooltip") Text(full) WidgetryText Pickable::IGNORE
            TextLayout::linebreak(LineBreak::WordOrCharacter)
            Node { max_width: px(TOOLTIP_MAX_WIDTH) }
        }
    });
    bsn! { @WidgetryTooltip { @content: {content} } }
}

fn fit_labels(
    mut titles: Query<(
        Entity,
        &ChildOf,
        &mut EllipsizedText,
        &mut Text,
        Ref<TextFont>,
        &LineHeight,
        Ref<LetterSpacing>,
        &TextLayout,
        Ref<ComputedUiRenderTargetInfo>,
    )>,
    containers: Query<(&ComputedNode, Has<WidgetryTooltipHeadless>)>,
    fonts: Res<Assets<Font>>,
    rem: Res<RemSize>,
    mut pipeline: ResMut<TextPipeline>,
    mut font_cx: ResMut<FontCx>,
    mut layout_cx: ResMut<LayoutCx>,
    mut scratch: Local<ComputedTextBlock>,
    mut commands: Commands,
) -> Result {
    for (entity, parent, mut title, mut text, font, line_height, spacing, layout, target) in
        &mut titles
    {
        let container = parent.parent();
        let (node, has_tooltip) = containers.get(container)?;
        let width = node.content_box().size().x;
        if width <= 0.0
            || (title.fitted_width == Some(width)
                && !font.is_changed()
                && !spacing.is_changed()
                && !target.is_changed()
                && !fonts.is_changed()
                && !rem.is_changed())
        {
            continue;
        }
        let measured_width = |value: &str| {
            let span = TextElement::Text {
                text: value,
                font: &font,
                color: Color::WHITE,
                line_height: *line_height,
                letter_spacing: *spacing,
            };
            pipeline
                .create_text_measure(
                    entity,
                    &fonts,
                    std::iter::once((entity, 0, span)),
                    target.scale_factor(),
                    layout,
                    &mut scratch,
                    &mut font_cx,
                    &mut layout_cx,
                    target.logical_size(),
                    *rem,
                )
                .map(|measure| measure.max.x)
        };
        let fitted = match fit(&title.full, width, measured_width) {
            Ok(fitted) => fitted,
            Err(
                TextError::NoSuchFont
                | TextError::NoSuchFontFamily(_)
                | TextError::DegenerateScaleFactor,
            ) => {
                title.fitted_width = None;
                continue;
            }
            Err(failure) => return Err(failure.into()),
        };
        let truncated = fitted != title.full.as_str();
        // Mount the popup beside the measured text so it remains a leaf in the UI layout.
        if truncated && (!has_tooltip || title.fitted_width.is_none()) {
            commands
                .entity(container)
                .try_remove::<WidgetryTooltipHeadless>()
                .apply_scene_with_error_handler(tooltip(title.full.clone()));
        } else if !truncated && has_tooltip {
            commands
                .entity(container)
                .try_remove::<WidgetryTooltipHeadless>();
        }
        if text.0 != fitted {
            text.0 = fitted.into_owned();
        }
        title.fitted_width = Some(width);
    }
    Ok(())
}

fn fit(
    full: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> Result<f32, TextError>,
) -> Result<Cow<'_, str>, TextError> {
    if measure(full)? <= width {
        return Ok(Cow::Borrowed(full));
    }
    if measure(ELLIPSIS)? > width {
        return Ok(Cow::Borrowed(""));
    }
    let cuts: Vec<_> = full
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .collect();
    let mut first = 0;
    let mut past = cuts.len();
    let mut label = String::new();
    while first + 1 < past {
        let candidate = first + (past - first) / 2;
        label.clear();
        label.push_str(full[..cuts[candidate]].trim_end());
        label.push_str(ELLIPSIS);
        if measure(&label)? <= width {
            first = candidate;
        } else {
            past = candidate;
        }
    }
    label.clear();
    label.push_str(full[..cuts[first]].trim_end());
    label.push_str(ELLIPSIS);
    Ok(Cow::Owned(label))
}
