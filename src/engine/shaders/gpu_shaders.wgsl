// ── Shared output struct ──────────────────────────────────────────────────────

struct VsOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// ── Vertex-quad shader (layer composite) ─────────────────────────────────────
// Vertices carry pre-computed NDC positions and UVs.
// Position is already in NDC space (computed on CPU from origin/size/scale/angle).

@vertex
fn vs_quad(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOutput {
    var out: VsOutput;
    out.position = vec4(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}

// ── WE dynamic-effect vertex shader ─────────────────────────────────────────
// Default vertex shader for translated WE GLSL fragment shaders that declare
// (vec4 v_TexCoord, vec2 v_Scroll) as their varyings. Fullscreen triangle.

struct WeOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) v_TexCoord: vec4<f32>,
    @location(1) v_Scroll: vec2<f32>,
}

@vertex
fn vs_we_effect(@builtin(vertex_index) vi: u32) -> WeOutput {
    var out: WeOutput;
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    out.position = vec4(x, y, 0.0, 1.0);
    let u = (x + 1.0) * 0.5;
    let v = (1.0 - y) * 0.5;
    out.v_TexCoord = vec4(u, v, u, v);
    out.v_Scroll = vec2(u, v);
    return out;
}

// ── Fullscreen triangle vertex shader (effects) ───────────────────────────────
// Used for effect passes that fill the entire layer render target.

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VsOutput {
    var out: VsOutput;
    // Oversized triangle that covers the full viewport.
    // vi=0: (-1,-1), vi=1: (3,-1), vi=2: (-1,3)
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    out.position = vec4(x, y, 0.0, 1.0);
    // UV: (0,0)=top-left, (1,1)=bottom-right — matches wgpu texture convention.
    // NDC y=1 (top) → v=0; NDC y=-1 (bottom) → v=1.
    out.uv = vec2((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// ── Shared bindings ──────────────────────────────────────────────────────────

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

// ── Passthrough (compositing / layer blit) ───────────────────────────────────

// CompositeParams places one scene object's quad and carries its per-frame
// blending state. `uv_offset` holds camera dynamics (shake + parallax);
// `rect` is (center_ndc.xy, half_size_ndc.xy) for the quad vertices.
// `angle` is the object's z-rotation in radians (WE `angles.z`); `aspect`
// (scene width/height) lets the vertex shader undo the anisotropic NDC scale
// before rotating so the quad rotates rigidly instead of shearing.
struct CompositeParams {
    opacity: f32,
    mode: i32,                      // bytes 4-7: WE colorBlendMode (0 = normal alpha blend)
    uv_offset: vec2<f32>,           // byte 8 (vec2 alignment=8)
    color: vec3<f32>,               // byte 16 (vec3 alignment=16)
    angle: f32,                     // byte 28 (fills the vec3's std140 tail)
    rect: vec4<f32>,                // byte 32: center.xy, half_extent.xy (NDC)
    aspect: f32,                    // byte 48: scene width / height
}
@group(0) @binding(2) var<uniform> composite: CompositeParams;
@group(0) @binding(3) var dest_copy_tex: texture_2d<f32>;

// Quad vertex shader for object composites: two triangles covering the
// object's rect, generated from vertex_index (no vertex buffer).
@vertex
fn vs_composite_quad(@builtin(vertex_index) vi: u32) -> VsOutput {
    // Corner order: (-1,-1) (1,-1) (-1,1) | (1,-1) (1,1) (-1,1)
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[vi];
    let he = corner * composite.rect.zw;
    // Rotate in isotropic (pixel-shaped) space: undo the NDC aspect scale on
    // one axis, rotate, then reapply it — otherwise a rotation would shear
    // non-square scenes. At angle=0 this reduces exactly to `he`.
    // NDC is Y-up while `composite.angle` is derived assuming Y-down pixel
    // space (matching the CPU compositor's convention), so negate sin here.
    let c = cos(composite.angle);
    let s = -sin(composite.angle);
    let offset = vec2(
        c * he.x - s * he.y / composite.aspect,
        s * he.x * composite.aspect + c * he.y,
    );
    var out: VsOutput;
    out.position = vec4(composite.rect.xy + offset, 0.0, 1.0);
    // NDC y=+1 (top) → v=0.
    out.uv = vec2(corner.x * 0.5 + 0.5, 0.5 - corner.y * 0.5);
    return out;
}

@fragment
fn fs_composite(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv - composite.uv_offset);
    return vec4(s.rgb * composite.color, s.a * composite.opacity);
}

// ── Photoshop-style colorBlendMode compositing ───────────────────────────────
// Mirrors WE_COMMON_BLENDING_H / ApplyBlending in shaders/transpiler.rs and
// engine/blend.rs (the CPU compositor's copy of the same table). Reads the
// destination (already-composited scene) via `dest_copy_tex` since wgpu can't
// read+write the same attachment in one pass.

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let fmin = min(min(c.r, c.g), c.b);
    let fmax = max(max(c.r, c.g), c.b);
    let delta = fmax - fmin;
    let l = (fmax + fmin) / 2.0;
    if (delta == 0.0) {
        return vec3(0.0, 0.0, l);
    }
    var s: f32;
    if (l < 0.5) {
        s = delta / (fmax + fmin);
    } else {
        s = delta / (2.0 - fmax - fmin);
    }
    let d_r = (((fmax - c.r) / 6.0) + (delta / 2.0)) / delta;
    let d_g = (((fmax - c.g) / 6.0) + (delta / 2.0)) / delta;
    let d_b = (((fmax - c.b) / 6.0) + (delta / 2.0)) / delta;
    var h: f32;
    if (c.r == fmax) {
        h = d_b - d_g;
    } else if (c.g == fmax) {
        h = (1.0 / 3.0) + d_r - d_b;
    } else {
        h = (2.0 / 3.0) + d_g - d_r;
    }
    if (h < 0.0) {
        h = h + 1.0;
    } else if (h > 1.0) {
        h = h - 1.0;
    }
    return vec3(h, s, l);
}

fn hue_to_rgb(f1: f32, f2: f32, hue_in: f32) -> f32 {
    var hue = hue_in;
    if (hue < 0.0) {
        hue = hue + 1.0;
    } else if (hue > 1.0) {
        hue = hue - 1.0;
    }
    if (6.0 * hue < 1.0) {
        return f1 + (f2 - f1) * 6.0 * hue;
    }
    if (2.0 * hue < 1.0) {
        return f2;
    }
    if (3.0 * hue < 2.0) {
        return f1 + (f2 - f1) * ((2.0 / 3.0) - hue) * 6.0;
    }
    return f1;
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    if (hsl.y == 0.0) {
        return vec3(hsl.z);
    }
    var f2: f32;
    if (hsl.z < 0.5) {
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

fn screen_f(base: f32, blend: f32) -> f32 { return 1.0 - ((1.0 - base) * (1.0 - blend)); }
fn overlay_f(base: f32, blend: f32) -> f32 {
    if (base < 0.5) { return 2.0 * base * blend; }
    return 1.0 - 2.0 * (1.0 - base) * (1.0 - blend);
}
fn soft_light_f(base: f32, blend: f32) -> f32 {
    if (blend < 0.5) { return 2.0 * base * blend + base * base * (1.0 - 2.0 * blend); }
    return sqrt(base) * (2.0 * blend - 1.0) + 2.0 * base * (1.0 - blend);
}
fn color_dodge_f(base: f32, blend: f32) -> f32 {
    if (blend == 1.0) { return blend; }
    return min(base / (1.0 - blend), 1.0);
}
fn color_burn_f(base: f32, blend: f32) -> f32 {
    if (blend == 0.0) { return blend; }
    return max(1.0 - ((1.0 - base) / blend), 0.0);
}

fn blend_rgb(mode: i32, base: vec3<f32>, blend: vec3<f32>) -> vec3<f32> {
    if (mode == 1) { return min(blend, base); }
    if (mode == 2) { return base * blend; }
    if (mode == 3) {
        return vec3(color_burn_f(base.r, blend.r), color_burn_f(base.g, blend.g), color_burn_f(base.b, blend.b));
    }
    if (mode == 4) { return max(base + blend - vec3(1.0), vec3(0.0)); }
    if (mode == 6) { return max(blend, base); }
    if (mode == 7) {
        return vec3(screen_f(base.r, blend.r), screen_f(base.g, blend.g), screen_f(base.b, blend.b));
    }
    if (mode == 8) {
        return vec3(color_dodge_f(base.r, blend.r), color_dodge_f(base.g, blend.g), color_dodge_f(base.b, blend.b));
    }
    if (mode == 9) { return min(base + blend, vec3(1.0)); }
    if (mode == 11) {
        return vec3(overlay_f(base.r, blend.r), overlay_f(base.g, blend.g), overlay_f(base.b, blend.b));
    }
    if (mode == 12) {
        return vec3(soft_light_f(base.r, blend.r), soft_light_f(base.g, blend.g), soft_light_f(base.b, blend.b));
    }
    if (mode == 18) { return abs(base - blend); }
    if (mode == 26) {
        let b = rgb_to_hsl(base);
        return hsl_to_rgb(vec3(rgb_to_hsl(blend).x, b.y, b.z));
    }
    if (mode == 27) {
        let b = rgb_to_hsl(base);
        return hsl_to_rgb(vec3(b.x, rgb_to_hsl(blend).y, b.z));
    }
    if (mode == 28) {
        let b = rgb_to_hsl(blend);
        return hsl_to_rgb(vec3(b.x, b.y, rgb_to_hsl(base).z));
    }
    if (mode == 29) {
        let b = rgb_to_hsl(base);
        return hsl_to_rgb(vec3(b.x, b.y, rgb_to_hsl(blend).z));
    }
    return blend;
}

@fragment
fn fs_composite_blend(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv - composite.uv_offset);
    let dest = textureSample(dest_copy_tex, src_sampler, uv);
    let src_rgb = s.rgb * composite.color;
    let src_a = s.a * composite.opacity;
    let blended = blend_rgb(composite.mode, dest.rgb, src_rgb);
    let out_rgb = mix(dest.rgb, blended, src_a);
    return vec4(out_rgb, dest.a);
}

// ── Blit (surface presentation / FBO up- and down-sampling) ──────────────────

struct BlitParams {
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
}
@group(0) @binding(2) var<uniform> blit: BlitParams;

@fragment
fn fs_blit(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, uv * blit.uv_scale + blit.uv_offset);
}

// ── Bloom (scene-level chain) ─────────────────────────────────────────────────
// Mirrors the reference's `downsample_quarter_bloom` / `downsample_eighth_blur_v`
// / `blur_h_bloom` / `combine` shaders exactly (the linux-wallpaperengine port
// synthesizes bloom from these four real WE utility shaders via a hidden
// fullscreen effect object — see CScene.cpp/WallpaperApplication.cpp). All
// texel offsets use the *scene's* texel size (WE's `g_TexelSize` is always
// scene-relative, never the destination FBO's own size — CPass.cpp).

struct BloomThresholdParams {
    // Scene texel size (1/sceneWidth, 1/sceneHeight) — the four taps sample
    // at +-texel like the reference vertex shader's v_TexCoord[0..3].
    texel: vec2<f32>,
    threshold: f32,
    strength: f32,
}
@group(1) @binding(0) var<uniform> bloom_threshold: BloomThresholdParams;

@fragment
fn fs_bloom_threshold(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = bloom_threshold.texel;
    var albedo = textureSample(src_tex, src_sampler, uv - t).rgb
        + textureSample(src_tex, src_sampler, uv + t).rgb
        + textureSample(src_tex, src_sampler, uv + vec2(-t.x, t.y)).rgb
        + textureSample(src_tex, src_sampler, uv + vec2(t.x, -t.y)).rgb;
    albedo = albedo * 0.25;

    let scale = max(max(albedo.x, albedo.y), albedo.z);
    albedo = albedo * saturate(scale - bloom_threshold.threshold);

    // http://stackoverflow.com/a/34183839 (saturation boost, sat=1.0)
    let grayscale = dot(vec3(0.2989, 0.5870, 0.1140), albedo);
    albedo = -grayscale + albedo * 2.0;

    return vec4(max(vec3(0.0), albedo * bloom_threshold.strength), 1.0);
}

struct BlurParams {
    // Blur-direction step: scene texel size * 8 along x or y (WE's
    // `localTexel = g_TexelSize.{x,y} * 8.0`), zero in the other axis.
    dir_texel: vec2<f32>,
    _p1: f32, _p2: f32,
}
@group(1) @binding(0) var<uniform> blur: BlurParams;

@fragment
fn fs_blur9(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // 13-tap kernel, weights and offsets verbatim from
    // downsample_eighth_blur_v.frag / blur_h_bloom.frag.
    let weights = array<f32, 13>(
        0.006299, 0.017298, 0.039533, 0.075189, 0.119007, 0.156756, 0.171834,
        0.156756, 0.119007, 0.075189, 0.039533, 0.017298, 0.006299,
    );
    var acc = vec3(0.0);
    for (var i = 0; i < 13; i = i + 1) {
        let o = blur.dir_texel * f32(i - 6);
        acc = acc + textureSample(src_tex, src_sampler, uv + o).rgb * weights[i];
    }
    return vec4(acc, 1.0);
}

@fragment
fn fs_bloom_combine(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // dest_copy_tex (binding 3, "extra slot 0") carries the pre-bloom scene copy.
    let scene = textureSample(dest_copy_tex, src_sampler, uv).rgb;
    let bloom = textureSample(src_tex, src_sampler, uv).rgb;
    return vec4(scene + bloom, 1.0);
}

// ── Pulse ────────────────────────────────────────────────────────────────────

struct PulseParams {
    time: f32,
    speed: f32,
    amount: f32,
    power: f32,
    phase: f32,
    _p1: f32,
    _p2: f32,
    _p3: f32,
    tint_low: vec3<f32>,
    _pad0: f32,
    tint_high: vec3<f32>,
    _pad1: f32,
}
@group(1) @binding(0) var<uniform> pulse: PulseParams;

@fragment
fn fs_pulse(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv);
    let raw = sin(pulse.time * pulse.speed + pulse.phase - 1.5708) * 0.5 + 0.5;
    let p = pow(smoothstep(0.0, 1.0, raw) * pulse.amount, pulse.power);
    let tint = mix(pulse.tint_low, pulse.tint_high, p);
    return vec4(s.rgb * tint, s.a);
}

// ── Scroll ───────────────────────────────────────────────────────────────────

struct ScrollParams {
    time: f32,
    speed_x: f32,
    speed_y: f32,
    scale_x: f32,
    scale_y: f32,
    _p1: f32,
    _p2: f32,
    _p3: f32,
}
@group(1) @binding(0) var<uniform> scroll: ScrollParams;

@fragment
fn fs_scroll(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let tc = fract((uv + vec2(scroll.speed_x, scroll.speed_y) * scroll.time) * vec2(scroll.scale_x, scroll.scale_y));
    return textureSample(src_tex, src_sampler, tc);
}

// ── Shake ────────────────────────────────────────────────────────────────────

struct ShakeParams {
    time: f32,
    speed: f32,
    strength: f32,
    _pad: f32,
}
@group(1) @binding(0) var<uniform> shake: ShakeParams;

@fragment
fn fs_shake(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = shake.speed * shake.time;
    var offset = sin(fract(t / 6.28318) * 6.28318);
    offset = offset * 0.498 + 0.5;
    let base = step(0.0, cos(t));
    offset = mix(1.0 - pow(max(1.0 - offset, 0.001), 2.0), pow(max(offset, 0.001), 2.0), base);
    offset = offset * 2.0 - 1.0;
    let tc = uv + vec2(offset * shake.strength * shake.strength, 0.0);
    return textureSample(src_tex, src_sampler, tc);
}

// ── Tint ─────────────────────────────────────────────────────────────────────

struct TintParams {
    r: f32,
    g: f32,
    b: f32,
    alpha: f32,
}
@group(1) @binding(0) var<uniform> tint: TintParams;

@fragment
fn fs_tint(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv);
    let tinted = mix(s.rgb, s.rgb * vec3(tint.r, tint.g, tint.b), tint.alpha);
    return vec4(tinted, s.a);
}

// ── Opacity ──────────────────────────────────────────────────────────────────

struct OpacityParams {
    alpha: f32,
    _p1: f32,
    _p2: f32,
    _p3: f32,
}
@group(1) @binding(0) var<uniform> opacity: OpacityParams;

@fragment
fn fs_opacity(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv);
    return vec4(s.rgb, s.a * opacity.alpha);
}

// ── Water Ripple ─────────────────────────────────────────────────────────────

struct RippleParams {
    time: f32,
    strength: f32,
    speed: f32,
    scale: f32,
}
@group(1) @binding(0) var<uniform> ripple: RippleParams;

@fragment
fn fs_waterripple(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s1 = sin((uv.x * ripple.scale + ripple.time * ripple.speed) * 6.28318);
    let s2 = sin((uv.y * ripple.scale * 0.7 + ripple.time * ripple.speed * 0.8) * 6.28318);
    let n = s1 * s2;
    let tc = uv + vec2(n, n * 0.7) * ripple.strength * 0.01;
    return textureSample(src_tex, src_sampler, tc);
}

// ── Water Waves ──────────────────────────────────────────────────────────────

struct WavesParams {
    time: f32,
    speed: f32,
    scale: f32,
    strength: f32,
    dir_x: f32,
    dir_y: f32,
    _p1: f32,
    _p2: f32,
}
@group(1) @binding(0) var<uniform> waves: WavesParams;

@fragment
fn fs_waterwaves(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dir = normalize(vec2(waves.dir_x, waves.dir_y));
    let perp = vec2(dir.y, -dir.x);
    let d = waves.time * waves.speed + dot(uv, dir) * waves.scale;
    let wave = sin(d) * waves.strength * waves.strength;
    let tc = uv + perp * wave;
    return textureSample(src_tex, src_sampler, tc);
}

// ── Spin ─────────────────────────────────────────────────────────────────────

struct SpinParams {
    time: f32,
    speed: f32,
    center_x: f32,
    center_y: f32,
}
@group(1) @binding(0) var<uniform> spin: SpinParams;

@fragment
fn fs_spin(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let center = vec2(spin.center_x, spin.center_y);
    let delta = uv - center;
    let angle = spin.speed * spin.time;
    let cs = vec2(cos(angle), sin(angle));
    let rotated = vec2(delta.x * cs.x - delta.y * cs.y, delta.x * cs.y + delta.y * cs.x);
    let tc = fract(rotated + center);
    return textureSample(src_tex, src_sampler, tc);
}
