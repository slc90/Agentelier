use ab_glyph::{Font as _, FontRef, Glyph, GlyphId, ScaleFont, point};
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use bevy_widgetry_asset::BuiltinFont;
use std::io;

const WIDTH: u32 = 2048;
const HEIGHT: u32 = 1024;
const ROW_HEIGHT: u32 = 42;
const PADDING: u32 = 7;
const FONT_SIZE: f32 = 30.0;

const PASSAGES: &[&str] = &[
    "We tell ourselves stories",
    "A map is not the territory",
    "a memory of water",
    "学而时习之",
    "积水成渊",
    "万物并作",
    "星野垂平阔",
    "语言的边界",
    "相变与涌现",
    "光阴之流",
    "const tide = sample(t)",
    "fn fold<T>(xs: &[T])",
    "for x in constellation:",
    "return f(x, y)",
    "SELECT word FROM corpus",
    "await stream.next()",
    "let mut memory = Vec::new()",
    "{ \"kind\": \"fragment\" }",
    "key: [concept, context]",
    "a² + b² = c²",
    "exp(iπ) + 1 = 0",
    "Σ 1/n² = π²/6",
    "ΔG = ΔH − TΔS",
    "dω = 0",
    "P(A|B) = P(B|A)P(A)/P(B)",
    "2H2 + O2 → 2H2O",
    "CO2 + H2O",
    "A–T · G–C",
    "rRNA · tRNA · mRNA",
    "ν = c/λ",
    "pV = nRT",
    "F = Gm1m2/r²",
    "epoch: [−∞, now]",
    "x → query → memory",
    "edges: [[0,1], [1,2]]",
    "fold → branch → leaf",
];

const FRAGMENTS: &[&str] = &[
    "ξ", "λ", "星", "海", "源", "墨", "序", "形", "文", "言", "光", "知", "流", "识", "律", "和",
    "梦", "深", "像", "字", "空", "μ", "Σ", "π", "κ", "δ", "→", "01", "11", "H2", "O2", "Na+",
    "ΔG", "CO2", "m/s", "K+", "φ(x)", "iπ", "c²", "σx", "r²", "1/n", "Γ(s)", "if", "fn", "let",
    "pub", "map", "[]", "{}", "<x>", "::", "A–T", "G–C", "ρ/ε", "dx", "dy", "log", "sea", "水面",
    "星图", "语言", "时间", "知识", "记忆", "∞",
];

#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub uv: Vec4,
    pub ratio: f32,
}

pub(super) struct Atlas {
    pub image: Image,
    pub entries: Vec<Entry>,
    pub passages: usize,
}

pub(super) fn build() -> Result<Atlas> {
    let font = FontRef::try_from_slice(BuiltinFont::Default.bytes())?;
    let font = font.as_scaled(FONT_SIZE);
    let mut pixels = vec![0; (WIDTH * HEIGHT) as usize];
    let mut entries = Vec::new();
    let mut passages = 0;
    let mut origin = UVec2::splat(PADDING);
    for (index, word) in PASSAGES.iter().chain(FRAGMENTS).enumerate() {
        let ids: Vec<_> = word
            .chars()
            .map(|character| font.glyph_id(character))
            .collect();
        if ids.contains(&GlyphId(0)) {
            continue;
        }
        let span: f32 = ids.iter().map(|&id| font.h_advance(id)).sum();
        let width = span.ceil() as u32 + PADDING * 2;
        if origin.x + width + PADDING > WIDTH {
            origin.x = PADDING;
            origin.y += ROW_HEIGHT + PADDING;
        }
        if origin.y + ROW_HEIGHT + PADDING > HEIGHT || width + PADDING * 2 > WIDTH {
            return Err(io::Error::other("Splash 字形超出图集容量").into());
        }
        let extent = UVec2::new(width, ROW_HEIGHT);
        rasterize(&font, &ids, origin, extent, &mut pixels);
        let end = origin + extent;
        let atlas_size = Vec2::new(WIDTH as f32, HEIGHT as f32);
        let uv_min = origin.as_vec2() / atlas_size;
        let uv_max = end.as_vec2() / atlas_size;
        entries.push(Entry {
            uv: Vec4::new(uv_min.x, uv_min.y, uv_max.x, uv_max.y),
            ratio: width as f32 / ROW_HEIGHT as f32,
        });
        if index < PASSAGES.len() {
            passages += 1;
        }
        origin.x = end.x + PADDING;
    }
    if passages == 0 || passages == entries.len() {
        return Err(io::Error::other("Splash 字形图集缺少段落或短碎片").into());
    }
    let mut image = Image::new(
        Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    Ok(Atlas {
        image,
        entries,
        passages,
    })
}

fn rasterize<F: ab_glyph::Font>(
    font: &impl ScaleFont<F>,
    ids: &[GlyphId],
    origin: UVec2,
    extent: UVec2,
    pixels: &mut [u8],
) {
    let baseline = (ROW_HEIGHT as f32 - font.height()) * 0.5 + font.ascent();
    let mut left = PADDING as f32;
    for &id in ids {
        let glyph = Glyph {
            id,
            scale: font.scale(),
            position: point(left, baseline),
        };
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            let start = IVec2::new(bounds.min.x as i32, bounds.min.y as i32);
            outline.draw(|x, y, coverage| {
                let local = start + IVec2::new(x as i32, y as i32);
                if local.cmpge(IVec2::ZERO).all() && local.cmplt(extent.as_ivec2()).all() {
                    let pixel = origin + local.as_uvec2();
                    let index = (pixel.y * WIDTH + pixel.x) as usize;
                    pixels[index] =
                        pixels[index].max((coverage * f32::from(u8::MAX)).round() as u8);
                }
            });
        }
        left += font.h_advance(id);
    }
}
