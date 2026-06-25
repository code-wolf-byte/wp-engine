// Common utilities for Wallpaper Engine effect shaders (WGSL port)
// Shared constants, color-space conversions, and blend modes.

const M_PI: f32 = 3.14159265359;
const M_PI_HALF: f32 = 1.57079632679;
const M_PI_2: f32 = 6.28318530718;

// ── Global uniforms shared by all effects ────────────────────────────────────

struct Globals {
    time: f32,
    frametime: f32,
    pointer_x: f32,
    pointer_y: f32,
    tex_resolution: vec4<f32>,  // (width, height, 1/width, 1/height)
}

// ── Color-space conversions ──────────────────────────────────────────────────

fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3(0.0), vec3(1.0)), vec3(c.y));
}

fn rgb2hsv(RGB: vec3<f32>) -> vec3<f32> {
    var P: vec4<f32>;
    if RGB.g < RGB.b {
        P = vec4(RGB.b, RGB.g, -1.0, 2.0 / 3.0);
    } else {
        P = vec4(RGB.g, RGB.b, 0.0, -1.0 / 3.0);
    }
    var Q: vec4<f32>;
    if RGB.r < P.x {
        Q = vec4(P.x, P.y, P.w, RGB.r);
    } else {
        Q = vec4(RGB.r, P.y, P.z, P.x);
    }
    let C = Q.x - min(Q.w, Q.y);
    let H = abs((Q.w - Q.y) / (6.0 * C + 1e-10) + Q.z);
    let S = C / (Q.x + 1e-10);
    return vec3(H, S, Q.x);
}

fn rotate_vec2(v: vec2<f32>, r: f32) -> vec2<f32> {
    let cs = vec2(cos(r), sin(r));
    return vec2(v.x * cs.x - v.y * cs.y, v.x * cs.y + v.y * cs.x);
}

fn greyscale(color: vec3<f32>) -> f32 {
    return dot(color, vec3(0.11, 0.59, 0.3));
}

// ── HSL conversions ──────────────────────────────────────────────────────────

fn rgb_to_hsl(color: vec3<f32>) -> vec3<f32> {
    let c = clamp(color, vec3(0.0), vec3(1.0));
    let fmin = min(min(c.r, c.g), c.b);
    let fmax = max(max(c.r, c.g), c.b);
    let delta = fmax - fmin;
    var hsl = vec3(0.0, 0.0, (fmax + fmin) / 2.0);
    if delta != 0.0 {
        if hsl.z < 0.5 {
            hsl.y = delta / (fmax + fmin);
        } else {
            hsl.y = delta / (2.0 - fmax - fmin);
        }
        let dR = (((fmax - c.r) / 6.0) + (delta / 2.0)) / delta;
        let dG = (((fmax - c.g) / 6.0) + (delta / 2.0)) / delta;
        let dB = (((fmax - c.b) / 6.0) + (delta / 2.0)) / delta;
        if c.r == fmax {
            hsl.x = dB - dG;
        } else if c.g == fmax {
            hsl.x = (1.0 / 3.0) + dR - dB;
        } else {
            hsl.x = (2.0 / 3.0) + dG - dR;
        }
        if hsl.x < 0.0 { hsl.x += 1.0; }
        if hsl.x > 1.0 { hsl.x -= 1.0; }
    }
    return hsl;
}

fn hue_to_rgb(f1: f32, f2: f32, hue_in: f32) -> f32 {
    var hue = hue_in;
    if hue < 0.0 { hue += 1.0; }
    if hue > 1.0 { hue -= 1.0; }
    if (6.0 * hue) < 1.0 { return f1 + (f2 - f1) * 6.0 * hue; }
    if (2.0 * hue) < 1.0 { return f2; }
    if (3.0 * hue) < 2.0 { return f1 + (f2 - f1) * ((2.0 / 3.0) - hue) * 6.0; }
    return f1;
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    if hsl.y == 0.0 {
        return vec3(hsl.z);
    }
    var f2: f32;
    if hsl.z < 0.5 {
        f2 = hsl.z * (1.0 + hsl.y);
    } else {
        f2 = (hsl.z + hsl.y) - (hsl.y * hsl.z);
    }
    let f1 = 2.0 * hsl.z - f2;
    return vec3(
        hue_to_rgb(f1, f2, hsl.x + 1.0 / 3.0),
        hue_to_rgb(f1, f2, hsl.x),
        hue_to_rgb(f1, f2, hsl.x - 1.0 / 3.0),
    );
}

// ── Blend modes ──────────────────────────────────────────────────────────────

fn blend_screen_f(base: f32, blend: f32) -> f32 { return 1.0 - ((1.0 - base) * (1.0 - blend)); }
fn blend_overlay_f(base: f32, blend: f32) -> f32 {
    if base < 0.5 { return 2.0 * base * blend; }
    return 1.0 - 2.0 * (1.0 - base) * (1.0 - blend);
}
fn blend_soft_light_f(base: f32, blend: f32) -> f32 {
    if blend < 0.5 { return 2.0 * base * blend + base * base * (1.0 - 2.0 * blend); }
    return sqrt(base) * (2.0 * blend - 1.0) + 2.0 * base * (1.0 - blend);
}
fn blend_color_dodge_f(base: f32, blend: f32) -> f32 {
    if blend == 1.0 { return blend; }
    return min(base / (1.0 - blend), 1.0);
}
fn blend_color_burn_f(base: f32, blend: f32) -> f32 {
    if blend == 0.0 { return blend; }
    return max(1.0 - ((1.0 - base) / blend), 0.0);
}

fn blend_screen(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    return vec3(blend_screen_f(base.r, blend.r), blend_screen_f(base.g, blend.g), blend_screen_f(base.b, blend.b));
}
fn blend_overlay(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    return vec3(blend_overlay_f(base.r, blend.r), blend_overlay_f(base.g, blend.g), blend_overlay_f(base.b, blend.b));
}
fn blend_soft_light(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    return vec3(blend_soft_light_f(base.r, blend.r), blend_soft_light_f(base.g, blend.g), blend_soft_light_f(base.b, blend.b));
}
fn blend_color_dodge(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    return vec3(blend_color_dodge_f(base.r, blend.r), blend_color_dodge_f(base.g, blend.g), blend_color_dodge_f(base.b, blend.b));
}
fn blend_color_burn(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    return vec3(blend_color_burn_f(base.r, blend.r), blend_color_burn_f(base.g, blend.g), blend_color_burn_f(base.b, blend.b));
}
fn blend_multiply(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> { return base * blend; }
fn blend_add(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> { return min(base + blend, vec3(1.0)); }
fn blend_difference(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> { return abs(base - blend); }
fn blend_exclusion(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> { return base + blend - 2.0 * base * blend; }
fn blend_tint(base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> { return vec3(max(base.x, max(base.y, base.z))) * blend; }

fn apply_blending(mode: i32, A: vec3<f32>, B: vec3<f32>, opacity: f32) -> vec3<f32> {
    var result: vec3<f32>;
    switch mode {
        case 2 { result = blend_multiply(A, B); }
        case 6 { result = max(A, B); }
        case 7 { result = blend_screen(A, B); }
        case 9 { result = blend_add(A, B); }
        case 11 { result = blend_overlay(A, B); }
        case 12 { result = blend_soft_light(A, B); }
        case 18 { result = blend_difference(A, B); }
        case 19 { result = blend_exclusion(A, B); }
        case 30 { result = blend_tint(A, B); }
        case 31 { return A + B * opacity; }
        default { result = B; }
    }
    return mix(A, result, opacity);
}
