use crate::{AgentelierAvatar, AvatarCrop, AvatarPresentation, AvatarSource};
use bevy::picking::{events::PointerDrag, pointer::PointerButton};
use bevy::prelude::*;

#[derive(SceneComponent, Clone, Default)]
#[scene(AvatarCropperProps)]
pub struct AvatarCropper;

#[derive(Component, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct AvatarCropperState {
    image: Option<Handle<Image>>,
    crop: AvatarCrop,
}

pub struct AvatarCropperProps {
    pub size: f32,
}

impl Default for AvatarCropperProps {
    fn default() -> Self {
        Self { size: 144.0 }
    }
}

impl AvatarCropper {
    fn scene(props: AvatarCropperProps) -> impl Scene {
        let size = props.size;
        bsn! {
            template(|_| Ok(AvatarCropperState::default()))
            Node { width: px(size), height: px(size), flex_shrink: 0.0 }
            Pickable::default() on(drag)
            Children [ @AgentelierAvatar { @size: size } ]
        }
    }
}

impl AvatarCropperState {
    pub fn replace(&mut self, image: Option<Handle<Image>>, crop: AvatarCrop) {
        self.image = image;
        self.crop = crop.constrained();
    }

    pub fn crop(&self) -> AvatarCrop {
        self.crop
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        if self.image.is_some() {
            self.crop.zoom = zoom;
            self.crop = self.crop.constrained();
        }
    }
}

fn drag(mut event: On<PointerDrag>, mut croppers: Query<(&ComputedNode, &mut AvatarCropperState)>) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok((node, mut cropper)) = croppers.get_mut(event.entity) else {
        return;
    };
    event.propagate(false);
    let logical_width = node.size().x * node.inverse_scale_factor();
    if cropper.image.is_some() && logical_width > 0.0 {
        let crop = cropper.crop;
        cropper.crop = AvatarCrop {
            center: crop.center - event.delta / (logical_width * crop.zoom),
            ..crop
        }
        .constrained();
    }
}

pub(super) fn sync(
    croppers: Query<(&AvatarCropperState, &Children), Changed<AvatarCropperState>>,
    mut avatars: Query<&mut AvatarPresentation>,
) {
    for (cropper, children) in &croppers {
        for child in children.iter() {
            if let Ok(mut avatar) = avatars.get_mut(child) {
                avatar.source = cropper
                    .image
                    .as_ref()
                    .map_or(AvatarSource::DefaultUser, |image| AvatarSource::Custom {
                        image: image.clone(),
                        crop: cropper.crop,
                    });
            }
        }
    }
}
