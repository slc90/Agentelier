use bevy::camera::NormalizedRenderTarget;
use bevy::core_pipeline::{FullscreenShader, schedule::Core2d, upscaling::upscaling};
use bevy::prelude::*;
use bevy::render::{
    RenderApp, RenderStartup,
    camera::ExtractedCamera,
    render_resource::{
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedRenderPipelineId, ColorTargetState, ColorWrites, FragmentState, LoadOp, Operations,
        PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor,
        ShaderStages, StoreOp, TextureFormat, TextureSampleType, binding_types::texture_2d,
    },
    renderer::{RenderContext, ViewQuery},
    sync_world::MainEntity,
    view::{ExtractedWindow, ViewTarget},
};
use bevy::ui_render::ui_pass;
use bevy::window::CompositeAlphaMode;

const SCENE_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

#[derive(Resource)]
struct WindowAlphaPipeline {
    layout: BindGroupLayoutDescriptor,
    id: CachedRenderPipelineId,
}

pub(crate) fn install(app: &mut App) {
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, prepare)
        .add_systems(
            Core2d,
            prepare_window_alpha.after(ui_pass).before(upscaling),
        );
}

fn prepare(
    mut commands: Commands,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "window alpha",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            texture_2d(TextureSampleType::Float { filterable: false }),
        ),
    );
    let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("window alpha".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: bevy_app_assets::window_alpha_shader(&server),
            entry_point: Some("fragment".into()),
            targets: vec![Some(ColorTargetState {
                format: SCENE_FORMAT,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });
    commands.insert_resource(WindowAlphaPipeline { layout, id });
}

fn prepare_window_alpha(
    view: ViewQuery<(&ViewTarget, &ExtractedCamera)>,
    windows: Query<(&MainEntity, &ExtractedWindow)>,
    state: Res<WindowAlphaPipeline>,
    cache: Res<PipelineCache>,
    mut context: RenderContext,
) {
    let (target, camera) = view.into_inner();
    let Some(NormalizedRenderTarget::Window(window)) = camera.target else {
        return;
    };
    if target.main_texture_format() != SCENE_FORMAT
        || !matches!(
            target.out_texture_view_format(),
            Some(TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb)
        )
        || !windows.iter().any(|(entity, extracted)| {
            entity.id() == window.entity()
                && extracted.alpha_mode == CompositeAlphaMode::PreMultiplied
        })
    {
        return;
    }
    let Some(pipeline) = cache.get_render_pipeline(state.id) else {
        return;
    };
    let output = target.post_process_write();
    let bind_group = context.render_device().create_bind_group(
        "window alpha source",
        &cache.get_bind_group_layout(&state.layout),
        &BindGroupEntries::single(output.source),
    );
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("window alpha"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: output.destination,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(LinearRgba::NONE.into()),
                    store: StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}
