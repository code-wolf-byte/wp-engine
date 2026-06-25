// UV-based effects: scroll, spin, shake, swing, skew, transform, twirl

struct Globals {
    time: f32,
    frametime: f32,
    tex_w: f32,
    tex_h: f32,
}

@group(0) @binding(0) var g_Texture0: texture_2d<f32>;
@group(0) @binding(1) var g_Sampler: sampler;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> globals: Globals;

// ── Scroll ───────────────────────────────────────────────────────────────────

struct ScrollParams {
    speed_x: f32,
    speed_y: f32,
    scale_x: f32,
    scale_y: f32,
}

@compute @workgroup_size(16, 16)
fn effect_scroll(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let scroll = vec2(scroll_params.speed_x, scroll_params.speed_y) * globals.time;
    let tc = fract((uv + scroll) * vec2(scroll_params.scale_x, scroll_params.scale_y));
    let color = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
    dst[gid.y * u32(globals.tex_w) + gid.x] = color;
}

// ── Spin ─────────────────────────────────────────────────────────────────────

struct SpinParams {
    speed: f32,
    center_x: f32,
    center_y: f32,
    size: f32,
    feather: f32,
}

@compute @workgroup_size(16, 16)
fn effect_spin(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let center = vec2(spin_params.center_x, spin_params.center_y);
    let delta = uv - center;
    let angle = spin_params.speed * globals.time;
    let cs = vec2(cos(angle), sin(angle));
    let rotated = vec2(delta.x * cs.x - delta.y * cs.y, delta.x * cs.y + delta.y * cs.x);
    let tc = fract(rotated + center);
    let dist = length(uv - center);
    let mask = smoothstep(spin_params.size + spin_params.feather + 0.00001,
                          spin_params.size - spin_params.feather, dist);
    let original = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let spun = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
    dst[gid.y * u32(globals.tex_w) + gid.x] = mix(original, spun, mask);
}

// ── Shake ────────────────────────────────────────────────────────────────────

struct ShakeParams {
    speed: f32,
    strength: f32,
    friction_x: f32,
    friction_y: f32,
}

@compute @workgroup_size(16, 16)
fn effect_shake(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let time = shake_params.speed * globals.time;
    var offset = sin(fract(time / 6.28318) * 6.28318);
    offset = offset * 0.498 + 0.5;
    let base = step(0.0, cos(time));
    offset = mix(1.0 - pow(1.0 - offset, shake_params.friction_x),
                 pow(offset, shake_params.friction_y), base);
    offset = offset * 2.0 - 1.0;
    let tc = uv + vec2(offset * shake_params.strength * shake_params.strength, 0.0);
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Swing ────────────────────────────────────────────────────────────────────

struct SwingParams {
    speed: f32,
    amount: f32,
}

@compute @workgroup_size(16, 16)
fn effect_swing(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let angle = sin(globals.time * swing_params.speed) * swing_params.amount;
    let center = vec2(0.5, 1.0);
    let delta = uv - center;
    let cs = vec2(cos(angle), sin(angle));
    let tc = vec2(delta.x * cs.x - delta.y * cs.y, delta.x * cs.y + delta.y * cs.x) + center;
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Skew ─────────────────────────────────────────────────────────────────────

struct SkewParams {
    amount_x: f32,
    amount_y: f32,
    speed: f32,
}

@compute @workgroup_size(16, 16)
fn effect_skew(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let t = sin(globals.time * skew_params.speed);
    let tc = vec2(uv.x + (uv.y - 0.5) * skew_params.amount_x * t,
                  uv.y + (uv.x - 0.5) * skew_params.amount_y * t);
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Transform ────────────────────────────────────────────────────────────────

struct TransformParams {
    scale_x: f32,
    scale_y: f32,
    offset_x: f32,
    offset_y: f32,
    angle: f32,
}

@compute @workgroup_size(16, 16)
fn effect_transform(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let center = vec2(0.5, 0.5);
    var tc = uv - center;
    let cs = vec2(cos(transform_params.angle), sin(transform_params.angle));
    tc = vec2(tc.x * cs.x - tc.y * cs.y, tc.x * cs.y + tc.y * cs.x);
    tc = tc * vec2(transform_params.scale_x, transform_params.scale_y) + center;
    tc += vec2(transform_params.offset_x, transform_params.offset_y);
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Twirl ────────────────────────────────────────────────────────────────────

struct TwirlParams {
    strength: f32,
    radius: f32,
    center_x: f32,
    center_y: f32,
}

@compute @workgroup_size(16, 16)
fn effect_twirl(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let center = vec2(twirl_params.center_x, twirl_params.center_y);
    let delta = uv - center;
    let dist = length(delta);
    let factor = max(0.0, 1.0 - dist / twirl_params.radius);
    let angle = twirl_params.strength * factor * factor * globals.time;
    let cs = vec2(cos(angle), sin(angle));
    let tc = vec2(delta.x * cs.x - delta.y * cs.y, delta.x * cs.y + delta.y * cs.x) + center;
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}
