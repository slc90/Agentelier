use bevy::prelude::*;
use bevy::ui::{UiSystems, widget::NodeImageMode};
use bevy_app_assets::AvatarAssets;
use bevy_widgetry::theme::WidgetryTheme;

const DEFAULT_SIZE: f32 = 32.0;

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
    pub const MAX_ZOOM: f32 = 6.0;

    pub fn constrained(self) -> Self {
        if !self.center.is_finite() || !self.zoom.is_finite() {
            return Self::default();
        }
        let zoom = self.zoom.clamp(1.0, Self::MAX_ZOOM);
        Self {
            center: self.center.clamp(Vec2::ZERO, Vec2::ONE),
            zoom,
        }
    }

    pub(super) fn area(self, size: Vec2) -> Option<Rect> {
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
    DefaultUser,
    Assistant,
    Custom {
        image: Handle<Image>,
        crop: AvatarCrop,
    },
}

#[derive(Resource, Default)]
pub struct UserAvatar(pub Option<(Handle<Image>, AvatarCrop)>);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SyncAvatars;

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
    app.init_resource::<UserAvatar>()
        .register_type::<AvatarPresentation>()
        .add_systems(
            PostUpdate,
            sync.in_set(SyncAvatars).before(UiSystems::Content),
        );
    super::avatar_cropper::install(app);
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
    user: Res<UserAvatar>,
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
        let user_changed = user.is_changed() && matches!(avatar.source, AvatarSource::User);
        if !changed && !user_changed && !images.is_changed() && !defaults.is_changed() {
            continue;
        }
        let (preferred, crop) = match &avatar.source {
            AvatarSource::User => user.0.as_ref().map_or(
                (&defaults.default_user, AvatarCrop::default()),
                |(image, crop)| (image, *crop),
            ),
            AvatarSource::DefaultUser => (&defaults.default_user, AvatarCrop::default()),
            AvatarSource::Assistant => (&defaults.assistant, AvatarCrop::default()),
            AvatarSource::Custom { image, crop } => (image, *crop),
        };
        let (image, rect) = images
            .get(preferred)
            .and_then(|image| crop.area(image.size().as_vec2()))
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
