// Nature effects: foliagesway, clouds, cloudmotion, fire, reflection

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

// ── Foliage Sway ─────────────────────────────────────────────────────────────

struct FoliageSwayParams {
    speed: f32,
    amount: f32,
    direction: f32,
    scale: f32,
}
@group(1) @binding(0) var<uniform> foliage_params: FoliageSwayParams;

@compute @workgroup_size(16, 16)
fn effect_foliagesway(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let phase = uv.x * foliage_params.scale + uv.y * foliage_params.scale * 0.7;
    let sway = sin(globals.time * foliage_params.speed + phase) * foliage_params.amount * 0.01;
    let sway2 = cos(globals.time * foliage_params.speed * 0.7 + phase * 1.3) * foliage_params.amount * 0.005;

    let dir = vec2(cos(foliage_params.direction), sin(foliage_params.direction));
    let perp = vec2(-dir.y, dir.x);

    let y_factor = 1.0 - uv.y;
    let tc = uv + (dir * sway + perp * sway2) * y_factor;
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Clouds ───────────────────────────────────────────────────────────────────

struct CloudsParams {
    speed_x: f32,
    speed_y: f32,
    scale: f32,
    brightness: f32,
    opacity: f32,
    power: f32,
}
@group(1) @binding(0) var<uniform> clouds_params: CloudsParams;
@group(1) @binding(1) var g_CloudTex: texture_2d<f32>;

fn noise_hash(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3(p.xyx) * 0.13);
    let dot_val = dot(p3, p3.yzx + 3.333);
    return fract((p3.x + p3.y) * p3.z * dot_val);
}

fn fbm(uv_in: vec2<f32>) -> f32 {
    var uv = uv_in;
    var val: f32 = 0.0;
    var amp: f32 = 0.5;
    for (var i = 0; i < 5; i++) {
        val += amp * noise_hash(uv);
        uv *= 2.0;
        amp *= 0.5;
    }
    return val;
}

@compute @workgroup_size(16, 16)
fn effect_clouds(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let base = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);

    let scroll = vec2(globals.time * clouds_params.speed_x, globals.time * clouds_params.speed_y);
    let cloud_uv = uv * clouds_params.scale + scroll;

    var cloud = fbm(cloud_uv) * 2.0 - 0.5;
    cloud = pow(clamp(cloud, 0.0, 1.0), clouds_params.power) * clouds_params.brightness;

    let result = mix(base.rgb, base.rgb + vec3(cloud), clouds_params.opacity);
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(result, base.a);
}

// ── Cloud Motion ─────────────────────────────────────────────────────────────

struct CloudMotionParams {
    speed_x: f32,
    speed_y: f32,
    scale: f32,
    mask_strength: f32,
}
@group(1) @binding(0) var<uniform> cloudmotion_params: CloudMotionParams;

@compute @workgroup_size(16, 16)
fn effect_cloudmotion(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let scroll = vec2(globals.time * cloudmotion_params.speed_x, globals.time * cloudmotion_params.speed_y);
    let tc = fract(uv + scroll * 0.01);
    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Fire ─────────────────────────────────────────────────────────────────────

struct FireParams {
    speed: f32,
    strength: f32,
    scale: f32,
}
@group(1) @binding(0) var<uniform> fire_params: FireParams;

@compute @workgroup_size(16, 16)
fn effect_fire(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let base = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);

    let fire_uv = vec2(uv.x * fire_params.scale, uv.y * fire_params.scale - globals.time * fire_params.speed);
    let n1 = fbm(fire_uv);
    let n2 = fbm(fire_uv * 2.0 + 0.5);
    let fire = pow(clamp(n1 * n2 * 2.0, 0.0, 1.0), 1.5) * fire_params.strength;

    let fire_color = vec3(1.0, 0.5, 0.1) * fire;
    let result = base.rgb + fire_color * base.a;
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(min(result, vec3(1.0)), base.a);
}

// ── Reflection ───────────────────────────────────────────────────────────────

struct ReflectionParams {
    strength: f32,
    offset_y: f32,
}
@group(1) @binding(0) var<uniform> reflection_params: ReflectionParams;

@compute @workgroup_size(16, 16)
fn effect_reflection(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let base = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);

    let mirror_uv = vec2(uv.x, 1.0 - uv.y + reflection_params.offset_y);
    let reflected = textureSampleLevel(g_Texture0, g_Sampler, mirror_uv, 0.0);
    let blend = step(0.5 + reflection_params.offset_y * 0.5, uv.y) * reflection_params.strength;
    let result = mix(base, reflected, blend);
    dst[gid.y * u32(globals.tex_w) + gid.x] = result;
}
