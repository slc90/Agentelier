use crate::{AgentelierAvatar, AvatarCrop, AvatarPresentation, AvatarSource};
use bevy::picking::{events::PointerDrag, pointer::PointerButton};
use bevy::prelude::*;

#[derive(Component, Clone, Reflect)]
#[reflect(Component)]
pub struct AvatarCropperState {
    #[reflect(ignore)]
    image: Handle<Image>,
    pixels: Vec2,
    display: Vec2,
    crop: AvatarCrop,
}

#[derive(Component, Clone, Default)]
struct CropMask;

pub fn avatar_cropper(image: Handle<Image>, pixels: UVec2, max_extent: f32) -> impl Scene {
    let pixels = pixels.as_vec2();
    let display = pixels * (max_extent / pixels.max_element());
    let state = AvatarCropperState {
        image: image.clone(),
        pixels,
        display,
        crop: AvatarCrop::default(),
    };
    let preview = AvatarSource::Custom {
        image: image.clone(),
        crop: AvatarCrop::default(),
    };
    let original = image.clone();
    bsn! {
        Node { align_items: AlignItems::Center, column_gap: px(20), width: percent(100), min_height: px(max_extent) }
        Name("SettingsAvatarCropper") template(move |_| Ok(state.clone()))
        Children [
            Name("AvatarOriginal") template(move |_| Ok(ImageNode::new(original.clone()))) Pickable::IGNORE
            Node { width: px(display.x), height: px(display.y), flex_shrink: 0.0 }
            Children [
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.48)) Pickable::IGNORE
                Node { width: percent(100), height: percent(100) }--
                CropMask Name("AvatarCropMask") template(move |_| Ok(ImageNode { image: image.clone(), visual_box: bevy::ui::VisualBox::BorderBox, ..default() }))
                Node { position_type: PositionType::Absolute, border: UiRect::all(px(2)), border_radius: BorderRadius::all(percent(50)) }
                BorderColor::all(Color::WHITE) on(drag)
            ]--
            Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(8), flex_grow: 1.0 }
            Children [
                Text("预览") bevy_widgetry::text::WidgetryText--
                @AgentelierAvatar { @size: 96.0, @source: preview } Name("AvatarCropPreview")
            ]
        ]
    }
}

impl AvatarCropperState {
    pub fn crop(&self) -> AvatarCrop {
        self.crop
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.crop.zoom = zoom;
        self.constrain_to_image();
    }

    fn constrain_to_image(&mut self) {
        self.crop = self.crop.constrained();
        if let Some(area) = self.crop.area(self.pixels) {
            self.crop.center = area.center() / self.pixels;
        }
    }
}

fn drag(
    mut event: On<PointerDrag>,
    parents: Query<&ChildOf>,
    mut croppers: Query<&mut AvatarCropperState>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    event.propagate(false);
    for parent in parents.iter_ancestors(event.entity) {
        if let Ok(mut cropper) = croppers.get_mut(parent) {
            let delta = event.delta / cropper.display;
            cropper.crop.center += delta;
            cropper.constrain_to_image();
            break;
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.register_type::<AvatarCropperState>()
        .add_systems(PostUpdate, sync.before(crate::SyncAvatars));
}

fn sync(
    croppers: Query<(&AvatarCropperState, &Children), Changed<AvatarCropperState>>,
    mut masks: Query<(&ChildOf, &mut Node, &mut ImageNode), With<CropMask>>,
    mut avatars: Query<(&ChildOf, &mut AvatarPresentation)>,
) {
    for (cropper, children) in &croppers {
        let Some(area) = cropper.crop.area(cropper.pixels) else {
            continue;
        };
        let scale = cropper.display / cropper.pixels;
        for (parent, mut node, mut image) in &mut masks {
            if children.contains(&parent.parent()) {
                node.left = px(area.min.x * scale.x);
                node.top = px(area.min.y * scale.y);
                node.width = px(area.width() * scale.x);
                node.height = px(area.height() * scale.y);
                image.rect = Some(area);
            }
        }
        for (parent, mut avatar) in &mut avatars {
            if children.contains(&parent.parent()) {
                avatar.source = AvatarSource::Custom {
                    image: cropper.image.clone(),
                    crop: cropper.crop,
                };
            }
        }
    }
}
