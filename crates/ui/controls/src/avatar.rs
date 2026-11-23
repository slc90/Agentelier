use bevy::prelude::*;
use bevy::ui::{UiSystems, widget::NodeImageMode};
use bevy_app_assets::AvatarAssets;
use bevy_widgetry::theme::WidgetryTheme;

const DEFAULT_SIZE: f32 = 32.0;
const ASSISTANT_CROP: AvatarCrop = AvatarCrop {
    center: Vec2::new(0.52, 0.117),
    zoom: 3.55,
};

#[derive(Clone, Copy, Reflect)]
pub struct AvatarCrop {
    pub center: Vec2,
    pub zoom: f32,
}

impl Default for AvatarCrop {
    fn default() -> Self {
        Self {
            center: Vec2::splat(0.5),
            zoom: 1.0,
        }
    }
}

impl AvatarCrop {
    fn area(self, image: &Image) -> Option<Rect> {
        let size = image.size().as_vec2();
        if size.min_element() < 1.0 || !self.center.is_finite() || !self.zoom.is_finite() {
            return None;
        }
        let side = (size.min_element() / self.zoom.max(1.0)).max(1.0);
        let half = Vec2::splat(side / 2.0);
        let center = (self.center.clamp(Vec2::ZERO, Vec2::ONE) * size).clamp(half, size - half);
        Some(Rect {
            min: center - half,
            max: center + half,
        })
    }
}

#[derive(Clone, Default, Reflect)]
pub enum AvatarSource {
    #[default]
    User,
    Assistant,
    Custom {
        image: Handle<Image>,
        crop: AvatarCrop,
    },
}

#[derive(SceneComponent, Clone, Default)]
#[scene(AgentelierAvatarProps)]
pub struct AgentelierAvatar;

#[derive(Component, Clone, Reflect)]
#[reflect(Component)]
pub struct AvatarPresentation {
    pub source: AvatarSource,
    pub size: f32,
}

#[derive(Clone)]
pub struct AgentelierAvatarProps {
    pub source: AvatarSource,
    pub size: f32,
}

impl Default for AgentelierAvatarProps {
    fn default() -> Self {
        Self {
            source: AvatarSource::User,
            size: DEFAULT_SIZE,
        }
    }
}

impl AgentelierAvatar {
    fn scene(props: AgentelierAvatarProps) -> impl Scene {
        let AgentelierAvatarProps { source, size } = props;
        let extent = extent(size);
        bsn! {
            template(move |_| Ok(AvatarPresentation { source: source.clone(), size }))
            Node {
                width: extent, height: extent, flex_shrink: 0.0,
                border_radius: BorderRadius::all(percent(50)),
            }
            ImageNode { image_mode: NodeImageMode::Stretch }
            BackgroundColor
            Pickable::IGNORE
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.register_type::<AvatarPresentation>()
        .add_systems(PostUpdate, sync.before(UiSystems::Content));
}

fn extent(size: f32) -> Val {
    px(if size.is_finite() && size > 0.0 {
        size
    } else {
        DEFAULT_SIZE
    })
}

fn sync(
    defaults: Res<AvatarAssets>,
    images: Res<Assets<Image>>,
    theme: Res<WidgetryTheme>,
    mut avatars: Query<(
        Ref<AvatarPresentation>,
        &mut Node,
        &mut ImageNode,
        &mut BackgroundColor,
    )>,
) {
    for (avatar, mut node, mut display, mut background) in &mut avatars {
        let changed = avatar.is_changed();
        if changed {
            let extent = extent(avatar.size);
            node.width = extent;
            node.height = extent;
        }
        if changed || theme.is_changed() {
            background.set_if_neq(BackgroundColor(theme.text_field.editable.normal.background));
        }
        if !changed && !images.is_changed() && !defaults.is_changed() {
            continue;
        }
        let (preferred, crop) = match &avatar.source {
            AvatarSource::User => (&defaults.default_user, AvatarCrop::default()),
            AvatarSource::Assistant => (&defaults.assistant, ASSISTANT_CROP),
            AvatarSource::Custom { image, crop } => (image, *crop),
        };
        let (image, rect) = images
            .get(preferred)
            .and_then(|image| crop.area(image))
            .map_or_else(
                || (&defaults.default_user, None),
                |rect| (preferred, Some(rect)),
            );
        if display.image != *image || display.rect != rect {
            display.image = image.clone();
            display.rect = rect;
        }
    }
}
