import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear};

@group(0) @binding(0) var window_color: texture_2d<f32>;

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(window_color, vec2<i32>(position.xy), 0);
    if color.a == 0.0 {
        return vec4(0.0);
    }
    if color.a == 1.0 {
        return color;
    }
    let straight_srgb = clamp(linear_to_srgb(color.rgb / color.a), vec3(0.0), vec3(1.0));
    // The sRGB attachment encodes RGB again when the pass writes it.
    return vec4(srgb_to_linear(straight_srgb * color.a), color.a);
}
