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

// CompositeParams: position/scale/rotation are baked into quad vertices on the CPU
// (matching linux-wallpaperengine's vertex-buffer approach). Only per-layer
// blending parameters remain here.
struct CompositeParams {
    opacity: f32,
    _p1: f32, _p2: f32, _p3: f32,  // bytes 4-15 padding
    color: vec3<f32>,               // byte 16 (vec3 alignment=16)
    _p4: f32,                       // byte 28
}
@group(0) @binding(2) var<uniform> composite: CompositeParams;

@fragment
fn fs_composite(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_sampler, uv);
    return vec4(s.rgb * composite.color, s.a * composite.opacity);
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
