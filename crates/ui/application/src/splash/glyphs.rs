use super::{atlas, portrait};
use bevy::{
    asset::{AssetPath, RenderAssetUsages, embedded_asset, embedded_path},
    camera::visibility::{NoFrustumCulling, RenderLayers},
    core_pipeline::core_2d::Transparent2d,
    mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef},
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        render_phase::ViewSortedRenderPhases,
        render_resource::{
            AsBindGroup, CachedPipelineState, PipelineCache, PrimitiveTopology,
            RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
        },
        storage::ShaderBuffer,
    },
    shader::{ShaderCacheError, ShaderRef},
    sprite_render::{AlphaMode2d, Material2d, Material2dKey, Material2dPipeline, Material2dPlugin},
};
use std::{
    io,
    sync::{Arc, OnceLock},
};
use tracing::debug;

const SPLASH_LAYER: usize = 1;
const GLYPH_INDEX: MeshVertexAttribute =
    MeshVertexAttribute::new("SplashGlyphIndex", 6_101_421_375, VertexFormat::Uint32);
const QUAD: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [1.0, 1.0, 0.0],
];
const TRIANGLES: [u32; 6] = [0, 2, 1, 1, 2, 3];

#[derive(ShaderType, Clone)]
struct Geometry {
    artwork_crop: Vec4,
    viewport: Vec2,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct GlyphMaterial {
    #[uniform(0)]
    geometry: Geometry,
    #[storage(1, read_only)]
    glyphs: Handle<ShaderBuffer>,
    #[texture(2)]
    #[sampler(3)]
    atlas: Handle<Image>,
    #[texture(4)]
    #[sampler(5)]
    artwork: Handle<Image>,
}

impl Material2d for GlyphMaterial {
    fn vertex_shader() -> ShaderRef {
        AssetPath::from_path_buf(embedded_path!("glyphs.wgsl"))
            .with_source("embedded")
            .into()
    }

    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }

    fn specialize(
        _pipeline: &Material2dPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            GLYPH_INDEX.at_shader_location(1),
        ])?];
        Ok(())
    }
}

type PipelineResult = Arc<OnceLock<Result<(), String>>>;

#[derive(Resource, Clone)]
struct GlyphPipeline {
    batch: Entity,
    result: PipelineResult,
}

pub(super) struct GlyphScene {
    pub batch: Entity,
    pipeline: PipelineResult,
}

impl GlyphScene {
    pub fn ready(&self) -> io::Result<bool> {
        match self.pipeline.get() {
            None => Ok(false),
            Some(Ok(())) => Ok(true),
            Some(Err(error)) => Err(io::Error::other(format!(
                "Splash 字形 Shader 无法编译：{error}"
            ))),
        }
    }

    pub fn retire(self, world: &mut World) {
        world.remove_resource::<GlyphPipeline>();
        world.despawn(self.batch);
    }
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "glyphs.wgsl");
    app.add_plugins(Material2dPlugin::<GlyphMaterial>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(ExtractSchedule, extract_pipeline);
        render.add_systems(Render, check_pipeline.after(RenderSystems::Render));
    }
}

pub(super) fn layer() -> RenderLayers {
    RenderLayers::layer(SPLASH_LAYER)
}

pub(super) fn spawn(
    world: &mut World,
    artwork: Handle<Image>,
    viewport: UVec2,
    pixel_crop: Rect,
) -> Result<GlyphScene> {
    let atlas = atlas::build()?;
    let mut images = world.resource_mut::<Assets<Image>>();
    let image = images
        .get(&artwork)
        .ok_or_else(|| io::Error::other("Splash 背景尚未加载"))?;
    let image_size = image.size().as_vec2();
    let targets = portrait::sample(image, &atlas.entries[atlas.passages..])?;
    let count = targets.len();
    let data = targets
        .into_iter()
        .map(|target| {
            [
                target.entry.uv.to_array(),
                [target.uv.x, target.uv.y, target.height, target.angle],
                [
                    target.weight,
                    target.tone,
                    target.arrival,
                    f32::from(target.route),
                ],
                [target.entry.ratio, 0.0, 0.0, 0.0],
            ]
        })
        .collect::<Vec<_>>();
    let atlas_image = images.add(atlas.image);
    let buffer = world
        .resource_mut::<Assets<ShaderBuffer>>()
        .add(ShaderBuffer::from(data));
    let mesh = world.resource_mut::<Assets<Mesh>>().add(batch_mesh(count)?);
    let crop = pixel_crop.min / image_size;
    let extent = pixel_crop.size() / image_size;
    let material = world
        .resource_mut::<Assets<GlyphMaterial>>()
        .add(GlyphMaterial {
            geometry: Geometry {
                artwork_crop: Vec4::new(crop.x, crop.y, extent.x, extent.y),
                viewport: viewport.as_vec2(),
            },
            glyphs: buffer,
            atlas: atlas_image,
            artwork,
        });
    let batch = world
        .spawn((
            Name::new("SplashGlyphBatch"),
            Mesh2d(mesh),
            MeshMaterial2d(material),
            NoFrustumCulling,
            layer(),
        ))
        .id();
    let pipeline = PipelineResult::default();
    world.insert_resource(GlyphPipeline {
        batch,
        result: pipeline.clone(),
    });
    debug!(
        glyphs = count,
        atlas_entries = atlas.entries.len(),
        "已生成 Splash 字形图集和人物构图"
    );
    Ok(GlyphScene { batch, pipeline })
}

fn batch_mesh(count: usize) -> Result<Mesh> {
    let count = u32::try_from(count)?;
    let mut corners = Vec::with_capacity(count as usize * QUAD.len());
    let mut ids = Vec::with_capacity(corners.capacity());
    let mut indices = Vec::with_capacity(count as usize * TRIANGLES.len());
    for glyph in 0..count {
        let base = glyph * QUAD.len() as u32;
        corners.extend(QUAD);
        ids.extend([glyph; QUAD.len()]);
        indices.extend(TRIANGLES.map(|triangle| base + triangle));
    }
    Ok(Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, corners)
    .with_inserted_attribute(GLYPH_INDEX, ids)
    .with_inserted_indices(Indices::U32(indices)))
}

fn extract_pipeline(
    probe: Extract<Option<Res<GlyphPipeline>>>,
    current: Option<Res<GlyphPipeline>>,
    mut commands: Commands,
) {
    match (probe.as_deref(), current) {
        (Some(probe), None) => commands.insert_resource(probe.clone()),
        (None, Some(_)) => commands.remove_resource::<GlyphPipeline>(),
        _ => {}
    }
}

fn check_pipeline(
    probe: Option<Res<GlyphPipeline>>,
    cache: Res<PipelineCache>,
    phases: Res<ViewSortedRenderPhases<Transparent2d>>,
) {
    let Some(probe) = probe
        .as_deref()
        .filter(|probe| probe.result.get().is_none())
    else {
        return;
    };
    let pipeline = phases.values().find_map(|phase| {
        phase
            .items
            .iter()
            .find_map(|(_, item)| (item.entity.1.id() == probe.batch).then_some(item.pipeline))
    });
    if let Some(pipeline) = pipeline {
        match cache.get_render_pipeline_state(pipeline) {
            CachedPipelineState::Ok(_) => {
                probe.result.get_or_init(|| Ok(()));
            }
            CachedPipelineState::Err(
                error @ (ShaderCacheError::ProcessShaderError(_)
                | ShaderCacheError::CreateShaderModule(_)),
            ) => {
                probe.result.get_or_init(|| Err(error.to_string()));
            }
            _ => {}
        }
    }
}
