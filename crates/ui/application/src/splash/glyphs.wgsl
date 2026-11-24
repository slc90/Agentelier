struct Geometry {
    artwork_crop: vec4<f32>,
    viewport: vec2<f32>,
    elapsed: f32,
    duration: f32,
}

struct Glyph {
    atlas: vec4<f32>,
    placement: vec4<f32>,
    detail: vec4<f32>,
    shape: vec4<f32>,
    motion: vec4<f32>,
    variation: vec4<f32>,
}

@group(2) @binding(0) var<uniform> geometry: Geometry;
@group(2) @binding(1) var<storage, read> glyphs: array<Glyph>;
@group(2) @binding(2) var atlas: texture_2d<f32>;
@group(2) @binding(3) var atlas_sampler: sampler;
@group(2) @binding(4) var artwork: texture_2d<f32>;
@group(2) @binding(5) var artwork_sampler: sampler;

const GYRES_BEGIN: f32 = 0.65;
const STREAMS_BEGIN: f32 = 1.36;
const SETTLING_BEGINS: f32 = 2.54;
const DAWN_BEGINS: f32 = 3.28;
const TAU: f32 = 6.2831853;
const BACKDROP: u32 = 0xffffffffu;
const PORTRAIT: f32 = 0.0;
const FREE: f32 = 1.0;
const STREAM: f32 = 2.0;
const BRIDGE: f32 = 3.0;

struct Gyre {
    origin: vec2<f32>,
    landing: vec2<f32>,
    radius: f32,
    spin: f32,
    flatten: f32,
    delay: f32,
    density: f32,
}

const GYRES = array<Gyre, 4>(
    Gyre(vec2(0.26, 0.38), vec2(0.43, 0.54), 0.89, 1.0, 0.45, 0.00, 0.92),
    Gyre(vec2(0.51, 0.22), vec2(0.64, 0.18), 0.66, -1.0, 0.31, 0.13, 0.82),
    Gyre(vec2(0.79, 0.43), vec2(0.82, 0.36), 1.02, 1.0, 0.54, 0.22, 0.87),
    Gyre(vec2(0.56, 0.70), vec2(0.61, 0.74), 0.75, -1.0, 0.37, 0.08, 0.75),
);

fn artwork_uv(screen: vec2<f32>) -> vec2<f32> {
    return geometry.artwork_crop.xy + screen * geometry.artwork_crop.zw;
}

fn screen_uv(art: vec2<f32>) -> vec2<f32> {
    return (art - geometry.artwork_crop.xy) / geometry.artwork_crop.zw;
}

fn gyre_center(gyre: Gyre, branch: u32, time: f32) -> vec2<f32> {
    let drift = vec2(sin(time * 0.66 + f32(branch) * 1.5), cos(time * 0.83 + f32(branch) * 1.8))
        * vec2(0.022, 0.027);
    let heading = smoothstep(GYRES_BEGIN, STREAMS_BEGIN + 0.55, time) * 0.42;
    return mix(gyre.origin + drift, clamp(screen_uv(gyre.landing), vec2(0.14), vec2(0.88, 0.83)), heading);
}

fn sea_crest(lane: f32, x: f32, tide: f32) -> f32 {
    return 0.51 + lane * 0.10 - 0.12 * (x - 0.5)
        + sin(x * 6.7 - tide * 0.42 - lane * 0.62) * (0.079 + lane * 0.022)
        + sin(x * 13.1 + tide * 0.32 + lane * 1.33) * 0.031;
}

struct Pose {
    center: vec2<f32>,
    span: f32,
    angle: f32,
    opacity: f32,
    blur: f32,
    settled: f32,
    color: vec3<f32>,
}

fn flow(glyph: Glyph, time: f32) -> Pose {
    let tide = time * 2.45;
    let role = glyph.shape.y;
    let lane = glyph.shape.z;
    let depth = glyph.shape.w;
    let perspective = depth * depth;
    let near_air = glyph.variation.w > 0.66;
    let across = select(1.0, -1.0, u32(lane) % 2u == 1u);
    let drift = fract(glyph.motion.x + tide * (0.019 + perspective * 0.049) * across);
    let strand = drift * TAU;
    let wave = sin(strand * 0.71 + lane * 1.46 + tide * 0.20);
    let cross_wave = cos(strand * 1.54 - tide * 0.27 + lane * 0.37);
    let glint = pow(0.5 + 0.5 * sin(tide * (0.72 + glyph.variation.x) + glyph.motion.y), 5.0);
    let edge = smoothstep(0.0, 0.14, drift) * (1.0 - smoothstep(0.86, 1.0, drift));
    let unit = geometry.viewport.y * 0.0042 + 3.0;
    var pose: Pose;
    pose.center = vec2(
        0.5 + (drift - 0.5) * mix(1.25, 1.91, perspective) + 0.033 * cross_wave * (1.0 + depth),
        select(
            0.407 + perspective * 0.62 + wave * (0.041 + perspective * 0.13)
                + cross_wave * 0.048 + (lane - 2.0) * 0.024,
            0.078 + depth * 0.48 + wave * 0.062 + cross_wave * 0.043,
            near_air
        )
    );
    pose.span = unit * (0.62 + perspective * (1.65 + glyph.variation.y * 1.34));
    pose.angle = (cross_wave * 0.114 + wave * 0.074) * select(1.0, 0.6, near_air);
    pose.blur = clamp((perspective - 0.67) * 2.5 + select(0.0, 0.16, near_air), 0.0, 1.0);
    pose.opacity = (0.16 + glyph.variation.z * 0.50 + glint * 0.21) * edge * select(1.0, 0.62, near_air);
    pose.color = mix(vec3(0.031, 0.24, 0.108), vec3(0.16, 0.67, 0.36), glyph.motion.w)
        + vec3(0.040, 0.15, 0.075) * glint;
    pose.settled = 0.0;
    if role == STREAM {
        let stream = f32(u32(lane) % 4u);
        let x = drift * 1.43 - 0.215;
        let crest = sea_crest(stream, x, tide);
        pose.center = vec2(x, crest + (glyph.motion.z - 0.5) * (0.025 + stream * 0.009));
        pose.angle = atan2((sea_crest(stream, x + 0.01, tide) - crest) * geometry.viewport.y,
            0.01 * geometry.viewport.x) * 0.70;
        pose.span = unit * (0.82 + glyph.variation.y * 1.54 + stream * 0.28);
        pose.opacity = edge * (0.27 + glyph.variation.z * 0.29 + glint * 0.25);
        pose.blur = 0.11;
    }
    return pose;
}

fn move_glyph(glyph: Glyph, time: f32) -> Pose {
    let branch = u32(glyph.detail.w);
    let gyre = GYRES[branch];
    let role = glyph.shape.y;
    let is_portrait = role == PORTRAIT;
    let formation = smoothstep(STREAMS_BEGIN, SETTLING_BEGINS, time);
    let dawn = smoothstep(DAWN_BEGINS, geometry.duration, time);
    let draw_start = GYRES_BEGIN + gyre.delay + glyph.variation.x * 0.15;
    let draw = smoothstep(draw_start, draw_start + 0.49, time)
        * select(1.0 - formation * 0.66, 1.0, is_portrait)
        * select(1.0, 0.0, role == FREE);
    let tide = time * 2.45;
    var pose = flow(glyph, time);
    if is_portrait {
        pose.opacity *= mix(0.47, 1.0, draw);
    }
    if draw > 0.0 {
        let center = gyre_center(gyre, branch, time);
        let orbit = pow(glyph.motion.z, 1.37);
        let turn = glyph.motion.y + gyre.spin * tide * (0.66 + (1.0 - orbit) * 0.96);
        let curl = turn + sin(glyph.variation.w * TAU + tide * 0.24) * 0.24;
        let aspect = geometry.viewport.x / geometry.viewport.y;
        let radius = (0.37 + aspect * 0.041) * gyre.radius * (0.10 + orbit * 0.93);
        let twist = 0.14 * sin(curl * 2.0 + f32(branch));
        let orbit_offset = vec2(cos(curl) * (1.0 + twist) / aspect,
            sin(curl) * gyre.flatten + cos(curl) * gyre.spin * 0.12) * radius;
        var path = center + orbit_offset;
        if role == BRIDGE {
            let next = (branch + select(2u, 1u, branch % 2u == 0u)) % 4u;
            let travel = fract(glyph.motion.x + tide * 0.127);
            let arc = sin(travel * TAU * 0.5);
            path = mix(center, gyre_center(GYRES[next], next, time), travel)
                + arc * vec2(0.10 * gyre.spin + twist * 0.25, 0.17 * gyre.spin + twist * 0.35);
        }
        pose.center = mix(pose.center, path, draw);
        pose.angle = mix(pose.angle, atan(gyre.flatten * cos(curl) / max(abs(sin(curl)), 0.01)) * 0.44, draw);
        pose.span = mix(pose.span, geometry.viewport.y * (0.0048 + glyph.shape.w * 0.0045), draw);
        pose.opacity = mix(pose.opacity, (0.28 + glyph.variation.z * 0.51) * gyre.density, draw);
        pose.blur *= 1.0 - draw * 0.72;
    }

    if is_portrait {
        let arrival = glyph.detail.z;
        let settle = smoothstep(max(STREAMS_BEGIN, arrival - 0.54), max(SETTLING_BEGINS, arrival + 0.49), time);
        let remaining = 1.0 - settle;
        let landing = screen_uv(glyph.placement.xy);
        let difference = (landing - pose.center) * geometry.viewport;
        let tangent = vec2(cos(glyph.placement.w), sin(glyph.placement.w));
        let bend = settle * remaining * remaining * gyre.spin * (0.94 + glyph.motion.z * 0.82);
        let direction = clamp(dot(difference, tangent) / (1.0 + length(difference) * 0.33), -1.0, 1.0);
        let approach = settle * settle * remaining * direction * min(length(difference) * 0.53, geometry.viewport.y * 0.26);
        let offset = difference * settle + vec2(-difference.y, difference.x) * bend - tangent * approach;
        let tone = smoothstep(0.06, 0.93, glyph.detail.y);
        let flash = 4.0 * settle * remaining;
        pose.center += offset / geometry.viewport;
        pose.angle = mix(pose.angle, glyph.placement.w, settle);
        pose.span = mix(pose.span, glyph.placement.z * geometry.viewport.y / geometry.artwork_crop.w, settle)
            * (1.0 + flash * 0.10);
        pose.color = mix(pose.color,
            mix(vec3(0.020, 0.105, 0.064), vec3(0.63, 0.94, 0.77), tone), settle);
        pose.opacity = mix(pose.opacity, 0.28 + glyph.detail.x * 0.44 + tone * 0.26, settle)
            * (1.0 - smoothstep(0.25, 0.99, dawn));
        pose.blur = mix(pose.blur, 0.03, settle);
        pose.settled = settle;
    } else {
        pose.opacity *= (1.0 - formation * 0.76) * (1.0 - smoothstep(0.02, 0.73, dawn));
    }
    return pose;
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
    @location(1) corner: vec2<f32>,
    @location(2) screen: vec2<f32>,
    @location(3) ink: vec4<f32>,
    @location(4) blur_step: vec2<f32>,
    @location(5) optics: vec2<f32>,
    @location(6) @interpolate(flat) backdrop: u32,
}

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) index: u32) -> VertexOutput {
    let corner = position.xy;
    var out: VertexOutput;
    out.corner = corner;
    if index == BACKDROP {
        out.position = vec4(corner * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.5, 1.0);
        out.screen = corner;
        out.backdrop = 1u;
        return out;
    }

    let glyph = glyphs[index];
    let pose = move_glyph(glyph, geometry.elapsed);
    let offset = (corner - 0.5) * pose.span * vec2(glyph.shape.x, 1.0);
    let c = cos(pose.angle);
    let s = sin(pose.angle);
    let rotated = mat2x2(vec2(c, s), vec2(-s, c)) * offset;
    let screen = pose.center + rotated / geometry.viewport;
    out.position = vec4(screen * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.5, 1.0);
    out.texel = mix(glyph.atlas.xy, glyph.atlas.zw, corner);
    out.screen = screen;
    out.ink = vec4(pose.color, pose.opacity);
    out.blur_step = (glyph.atlas.zw - glyph.atlas.xy) / vec2(64.0, 35.0) * (1.0 + pose.blur * 1.8);
    out.optics = vec2(pose.blur, pose.settled);
    out.backdrop = 0u;
    return out;
}

fn hash(point: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(point, vec2(0.06711056, 0.00583715))));
}

fn noise(point: vec2<f32>) -> f32 {
    let cell = floor(point);
    let f = fract(point);
    let ease = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(cell), hash(cell + vec2(1.0, 0.0)), ease.x),
        mix(hash(cell + vec2(0.0, 1.0)), hash(cell + vec2(1.0)), ease.x), ease.y);
}

fn atmosphere(uv: vec2<f32>, time: f32, dawn: f32) -> vec3<f32> {
    let tide = time * 2.45;
    let aspect = geometry.viewport.x / geometry.viewport.y;
    let plane = (uv - 0.5) * vec2(aspect, 1.0);
    let mist = noise(plane * vec2(5.2, 9.0) + vec2(-tide * 0.054, sin(plane.x * 4.0) * 0.64));
    let pattern = noise(plane * vec2(14.0, 23.0) + vec2(tide * 0.061, mist * 1.8));
    let crest = plane.y - 0.095 + 0.092 * sin(plane.x * 3.6 - tide * 0.16) + 0.033 * sin(plane.x * 8.4 + tide * 0.22);
    let shelf = exp(-5.4 * abs(crest)) * smoothstep(0.24, 0.84, pattern);
    var wake = 0.0;
    var cores = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let lane = f32(i);
        let wave = (uv.y - sea_crest(lane, uv.x, tide)) * 34.0;
        wake += exp(-wave * wave) * smoothstep(0.27, 0.79, noise(vec2(uv.x * 14.0 - tide * 0.7, 2.7 + lane * 8.2)));
        let gyre = GYRES[i];
        let strength = smoothstep(GYRES_BEGIN + gyre.delay, GYRES_BEGIN + gyre.delay + 0.50, time);
        let delta = (uv - gyre_center(gyre, i, time)) * vec2(aspect, 2.05);
        let radius = length(delta);
        let heading = atan2(delta.y, delta.x);
        let arm = 0.5 + 0.5 * cos((2.0 + f32(i % 2u)) * heading + 16.0 * radius + tide * 0.61 * gyre.spin + lane * 2.1);
        cores += (pow(arm, 6.0) * exp(-radius * 6.5) * smoothstep(0.028, 0.13, radius)
            + exp(-23.0 * radius) * 0.23) * strength * gyre.density;
    }
    let cohesion = smoothstep(STREAMS_BEGIN, SETTLING_BEGINS, time);
    let sea = (shelf * 0.056 + mist * exp(-length(plane * vec2(1.6, 4.0))) * 0.033
        + wake * 0.039 * (1.0 - 0.86 * cohesion) + cores * 0.038 * (1.0 - 0.88 * cohesion));
    let vignette = clamp(1.12 - dot(plane * vec2(0.63, 1.06), plane * vec2(0.63, 1.06)), 0.18, 1.0);
    let grain = (hash(uv * geometry.viewport + floor(tide * 12.0) * 0.47) - 0.5) * 0.004;
    let night = max(vec3(0.0), (vec3(0.001, 0.007, 0.004) + vec3(0.19, 0.79, 0.38) * sea) * vignette + grain);
    let source = textureSampleLevel(artwork, artwork_sampler, artwork_uv(uv), 0.0).rgb;
    let morning = clamp(dawn + (mist - 0.5) * 0.53 * dawn * (1.0 - dawn), 0.0, 1.0);
    return mix(night, source, morning);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let dawn = smoothstep(DAWN_BEGINS, geometry.duration, geometry.elapsed);
    if input.backdrop == 1u {
        return vec4(atmosphere(input.screen, geometry.elapsed, dawn), 1.0);
    }
    let d = input.blur_step;
    let fill = textureSampleLevel(atlas, atlas_sampler, input.texel, 0.0).r;
    let soft = (
        textureSampleLevel(atlas, atlas_sampler, input.texel + vec2(d.x, 0.0), 0.0).r
        + textureSampleLevel(atlas, atlas_sampler, input.texel - vec2(d.x, 0.0), 0.0).r
        + textureSampleLevel(atlas, atlas_sampler, input.texel + vec2(0.0, d.y), 0.0).r
        + textureSampleLevel(atlas, atlas_sampler, input.texel - vec2(0.0, d.y), 0.0).r
    ) * 0.25;
    let inner = min(input.corner, 1.0 - input.corner);
    let area = smoothstep(0.0, 0.09, min(inner.x, inner.y));
    let settled = smoothstep(0.60, 1.0, input.optics.y);
    let coverage = mix(fill, soft, input.optics.x * 0.72) + soft * mix(0.14, 0.05, settled);
    let alpha = mix(coverage, pow(min(1.0, coverage), 0.43), settled) * area * input.ink.a;
    let art = artwork_uv(input.screen);
    let source = textureSampleLevel(artwork, artwork_sampler, art, 0.0).rgb;
    let tone = smoothstep(0.56, 0.84, dot(source, vec3(0.45, 0.46, 0.09)));
    let eye = min(length((art - vec2(0.679, 0.274)) / vec2(0.022, 0.028)),
        length((art - vec2(0.744, 0.234)) / vec2(0.020, 0.030)));
    let iris = (1.0 - smoothstep(0.42, 1.0, eye)) * smoothstep(0.07, 0.21, source.g - source.r);
    let pigment = mix(vec3(0.008, 0.032, 0.025), vec3(0.76, 0.98, 0.82), tone)
        + vec3(0.04, 0.42, 0.21) * iris;
    let print = mix(source * vec3(0.48, 0.71, 0.57), source, 0.32 + 0.67 * tone);
    let painted = mix(pigment, print, smoothstep(0.06, 0.59, dawn));
    return vec4(mix(input.ink.rgb, painted, settled * 0.92), alpha);
}
