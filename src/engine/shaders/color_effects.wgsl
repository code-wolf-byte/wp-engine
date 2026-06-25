// Color effects: pulse, tint, opacity, blend, colorkey

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

// ── Pulse ────────────────────────────────────────────────────────────────────

struct PulseParams {
    speed: f32,
    phase: f32,
    amount: f32,
    threshold_low: f32,
    threshold_high: f32,
    power: f32,
    tint_low_r: f32,
    tint_low_g: f32,
    tint_low_b: f32,
    tint_high_r: f32,
    tint_high_g: f32,
    tint_high_b: f32,
}
@group(1) @binding(0) var<uniform> pulse_params: PulseParams;

@compute @workgroup_size(16, 16)
fn effect_pulse(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let sample = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);

    var pulse = smoothstep(pulse_params.threshold_low, pulse_params.threshold_high,
        sin(globals.time * pulse_params.speed + (pulse_params.phase - 1.57079632679)) * 0.5 + 0.5)
        * pulse_params.amount;
    pulse = pow(pulse, pulse_params.power);

    let tint_low = vec3(pulse_params.tint_low_r, pulse_params.tint_low_g, pulse_params.tint_low_b);
    let tint_high = vec3(pulse_params.tint_high_r, pulse_params.tint_high_g, pulse_params.tint_high_b);
    let blended = mix(sample.rgb * tint_low, sample.rgb * tint_high, vec3(pulse));

    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(max(vec3(0.0), blended), sample.a);
}

// ── Tint ─────────────────────────────────────────────────────────────────────

struct TintParams {
    color_r: f32,
    color_g: f32,
    color_b: f32,
    alpha: f32,
}
@group(1) @binding(0) var<uniform> tint_params: TintParams;

@compute @workgroup_size(16, 16)
fn effect_tint(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let sample = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let tint = vec3(tint_params.color_r, tint_params.color_g, tint_params.color_b);
    let blended = mix(sample.rgb, sample.rgb * tint, tint_params.alpha);
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(blended, sample.a);
}

// ── Opacity ──────────────────────────────────────────────────────────────────

struct OpacityParams {
    alpha: f32,
}
@group(1) @binding(0) var<uniform> opacity_params: OpacityParams;

@compute @workgroup_size(16, 16)
fn effect_opacity(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let sample = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(sample.rgb, sample.a * opacity_params.alpha);
}

// ── Color Key ────────────────────────────────────────────────────────────────

struct ColorKeyParams {
    key_r: f32,
    key_g: f32,
    key_b: f32,
    threshold: f32,
    smoothness: f32,
}
@group(1) @binding(0) var<uniform> colorkey_params: ColorKeyParams;

@compute @workgroup_size(16, 16)
fn effect_colorkey(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let sample = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let key = vec3(colorkey_params.key_r, colorkey_params.key_g, colorkey_params.key_b);
    let diff = length(sample.rgb - key);
    let mask = smoothstep(colorkey_params.threshold - colorkey_params.smoothness,
                          colorkey_params.threshold + colorkey_params.smoothness, diff);
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(sample.rgb, sample.a * mask);
}
