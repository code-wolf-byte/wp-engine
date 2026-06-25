// Water effects: waterripple, waterwaves, waterflow

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

// ── Water Ripple ─────────────────────────────────────────────────────────────

struct WaterRippleParams {
    strength: f32,
    animation_speed: f32,
    scale: f32,
    scroll_speed: f32,
    direction: f32,
    ratio: f32,
}
@group(1) @binding(0) var<uniform> ripple_params: WaterRippleParams;
@group(1) @binding(1) var g_NormalMap: texture_2d<f32>;

@compute @workgroup_size(16, 16)
fn effect_waterripple(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let aspect = globals.tex_w / globals.tex_h;
    let scroll_dir = vec2(-sin(ripple_params.direction), cos(ripple_params.direction));
    let scroll = scroll_dir * ripple_params.scroll_speed * ripple_params.scroll_speed * globals.time;

    var rc1 = uv + globals.time * ripple_params.animation_speed * ripple_params.animation_speed + scroll;
    var rc2 = uv * 1.333 - globals.time * ripple_params.animation_speed * ripple_params.animation_speed + scroll;
    rc1 *= ripple_params.scale;
    rc2 *= ripple_params.scale;
    rc1.x *= aspect;
    rc2.x *= aspect;
    rc1.y *= ripple_params.ratio;
    rc2.y *= ripple_params.ratio;

    let n1 = textureSampleLevel(g_NormalMap, g_Sampler, rc1, 0.0).xyz * 2.0 - 1.0;
    let n2 = textureSampleLevel(g_NormalMap, g_Sampler, rc2, 0.0).xyz * 2.0 - 1.0;
    let normal = normalize(vec3(n1.xy + n2.xy, n1.z));

    let tc = uv + normal.xy * ripple_params.strength * ripple_params.strength;
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Water Waves ──────────────────────────────────────────────────────────────

struct WaterWavesParams {
    speed: f32,
    scale: f32,
    strength: f32,
    exponent: f32,
    direction_x: f32,
    direction_y: f32,
}
@group(1) @binding(0) var<uniform> waves_params: WaterWavesParams;

@compute @workgroup_size(16, 16)
fn effect_waterwaves(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let dir = normalize(vec2(waves_params.direction_x, waves_params.direction_y));
    let distance = globals.time * waves_params.speed + dot(uv, dir) * waves_params.scale;
    let strength = waves_params.strength * waves_params.strength;

    let perp = vec2(dir.y, -dir.x);
    let val = sin(distance);
    let s = sign(val);
    let wave = pow(abs(val), waves_params.exponent);

    let tc = uv + wave * s * perp * strength;
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Water Flow ───────────────────────────────────────────────────────────────

struct WaterFlowParams {
    strength: f32,
    speed: f32,
    phase_scale: f32,
}
@group(1) @binding(0) var<uniform> flow_params: WaterFlowParams;
@group(1) @binding(1) var g_FlowMap: texture_2d<f32>;

@compute @workgroup_size(16, 16)
fn effect_waterflow(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let flow_colors = textureSampleLevel(g_FlowMap, g_Sampler, uv, 0.0).rg;
    let flow_mask = (flow_colors - vec2(0.498, 0.498)) * 2.0;
    let flow_amount = length(flow_mask);

    let cycle = fract(globals.time * flow_params.speed);
    let cycle2 = fract(globals.time * flow_params.speed + 0.5);

    let offset1 = flow_mask * flow_params.strength * 0.1 * cycle;
    let offset2 = flow_mask * flow_params.strength * 0.1 * cycle2;

    let blend = abs(cycle * 2.0 - 1.0);
    let flow1 = textureSampleLevel(g_Texture0, g_Sampler, uv + offset1, 0.0);
    let flow2 = textureSampleLevel(g_Texture0, g_Sampler, uv + offset2, 0.0);
    let flow_color = mix(flow1, flow2, blend);

    let original = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    dst[gid.y * u32(globals.tex_w) + gid.x] = mix(original, flow_color, flow_amount);
}
