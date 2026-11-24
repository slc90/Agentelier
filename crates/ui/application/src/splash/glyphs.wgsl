struct Geometry {
    artwork_crop: vec4<f32>,
    viewport: vec2<f32>,
}

struct Glyph {
    atlas: vec4<f32>,
    placement: vec4<f32>,
    detail: vec4<f32>,
    shape: vec4<f32>,
}

@group(2) @binding(0) var<uniform> geometry: Geometry;
@group(2) @binding(1) var<storage, read> glyphs: array<Glyph>;
@group(2) @binding(2) var atlas: texture_2d<f32>;
@group(2) @binding(3) var atlas_sampler: sampler;
@group(2) @binding(4) var artwork: texture_2d<f32>;
@group(2) @binding(5) var artwork_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
    @location(1) corner: vec2<f32>,
    @location(2) screen: vec2<f32>,
    @location(3) ink: vec4<f32>,
    @location(4) blur_step: vec2<f32>,
}

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) index: u32) -> VertexOutput {
    let glyph = glyphs[index];
    let corner = position.xy;
    let span = glyph.placement.z * geometry.viewport.y / geometry.artwork_crop.w;
    let offset = (corner - 0.5) * span * vec2(glyph.shape.x, 1.0);
    let c = cos(glyph.placement.w);
    let s = sin(glyph.placement.w);
    let rotated = mat2x2(vec2(c, s), vec2(-s, c)) * offset;
    let center = (glyph.placement.xy - geometry.artwork_crop.xy) / geometry.artwork_crop.zw;
    let screen = center + rotated / geometry.viewport;
    let tone = smoothstep(0.06, 0.93, glyph.detail.y);
    var out: VertexOutput;
    out.position = vec4(screen * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.5, 1.0);
    out.texel = mix(glyph.atlas.xy, glyph.atlas.zw, corner);
    out.corner = corner;
    out.screen = screen;
    out.ink = vec4(mix(vec3(0.020, 0.105, 0.064), vec3(0.63, 0.94, 0.77), tone),
        0.28 + glyph.detail.x * 0.44 + tone * 0.26);
    out.blur_step = (glyph.atlas.zw - glyph.atlas.xy) / vec2(64.0, 35.0);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let d = input.blur_step;
    let fill = textureSample(atlas, atlas_sampler, input.texel).r;
    let soft = (
        textureSample(atlas, atlas_sampler, input.texel + vec2(d.x, 0.0)).r
        + textureSample(atlas, atlas_sampler, input.texel - vec2(d.x, 0.0)).r
        + textureSample(atlas, atlas_sampler, input.texel + vec2(0.0, d.y)).r
        + textureSample(atlas, atlas_sampler, input.texel - vec2(0.0, d.y)).r
    ) * 0.25;
    let inner = min(input.corner, 1.0 - input.corner);
    let area = smoothstep(0.0, 0.09, min(inner.x, inner.y));
    let art_uv = geometry.artwork_crop.xy + input.screen * geometry.artwork_crop.zw;
    let source = textureSample(artwork, artwork_sampler, art_uv).rgb;
    let tone = smoothstep(0.56, 0.84, dot(source, vec3(0.45, 0.46, 0.09)));
    let eye = min(length((art_uv - vec2(0.679, 0.274)) / vec2(0.022, 0.028)),
        length((art_uv - vec2(0.744, 0.234)) / vec2(0.020, 0.030)));
    let iris = (1.0 - smoothstep(0.42, 1.0, eye)) * smoothstep(0.07, 0.21, source.g - source.r);
    let pigment = mix(vec3(0.008, 0.032, 0.025), vec3(0.76, 0.98, 0.82), tone)
        + vec3(0.04, 0.42, 0.21) * iris;
    let alpha = pow(min(1.0, fill + soft * 0.14), 0.43) * area * input.ink.a;
    return vec4(mix(input.ink.rgb, pigment, 0.80), alpha);
}
