// Post-processing effects: filmgrain, vhs, chromatic_aberration, fisheye, shimmer, nitro

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

fn hash22(p: vec2<f32>) -> vec2<f32> {
    var p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

// ── Film Grain ───────────────────────────────────────────────────────────────

struct FilmGrainParams {
    strength: f32,
    power: f32,
}
@group(1) @binding(0) var<uniform> grain_params: FilmGrainParams;

@compute @workgroup_size(16, 16)
fn effect_filmgrain(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    var albedo = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);

    let noise = hash22(uv * 4.0 + vec2(globals.time * 0.7, globals.time * 0.3));
    let noise2 = hash22(uv * 4.0 + vec2(globals.time * -0.5, globals.time * 0.8));
    var grain = vec3(clamp(noise.x * noise2.x, 0.0, 1.0));
    grain = pow(grain, vec3(grain_params.power));

    albedo = vec4(mix(albedo.rgb, albedo.rgb + albedo.rgb * (grain - 0.5), grain_params.strength), albedo.a);
    dst[gid.y * u32(globals.tex_w) + gid.x] = albedo;
}

// ── VHS ──────────────────────────────────────────────────────────────────────

struct VhsParams {
    noise_scale: f32,
    noise_alpha: f32,
    distortion_strength: f32,
    distortion_speed: f32,
    distortion_width: f32,
    chromatic: f32,
    tracking: f32,
    artifacts: f32,
}
@group(1) @binding(0) var<uniform> vhs_params: VhsParams;

@compute @workgroup_size(16, 16)
fn effect_vhs(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let dblend = sin(globals.time);
    let dblend_shaped = sign(dblend) * pow(abs(max(0.00001, dblend)), 4.0);
    let dist_line = fract(globals.time * vhs_params.distortion_speed);
    let dist_mask = smoothstep(0.01 * vhs_params.distortion_width, 0.0, abs(dist_line - uv.y));
    let distortion = vec2(dblend_shaped * vhs_params.distortion_strength * 0.02 * dist_mask * vhs_params.noise_alpha, 0.0);

    let noise_seed = hash22(vec2(globals.time, uv.y * 100.0));
    let line_offset = step(0.9, noise_seed.x) * 0.005 * vhs_params.tracking;
    let x_offset = vhs_params.noise_alpha * vhs_params.chromatic * 0.1 + line_offset;

    var tc = uv;
    tc.x += x_offset;
    let orig = textureSampleLevel(g_Texture0, g_Sampler, tc + distortion, 0.0);

    var albedo = orig;
    albedo.g = textureSampleLevel(g_Texture0, g_Sampler, vec2(tc.x + x_offset * 0.5, tc.y) + distortion, 0.0).g;
    albedo.b = textureSampleLevel(g_Texture0, g_Sampler, vec2(tc.x - x_offset * 0.5, tc.y) + distortion, 0.0).b;

    let grain = hash22(uv * 8.0 + globals.time);
    albedo = vec4(mix(albedo.rgb, albedo.rgb + (vec3(grain.x) - 0.5) * 0.1, vhs_params.noise_alpha * 0.3), albedo.a);

    dst[gid.y * u32(globals.tex_w) + gid.x] = mix(orig, albedo, vhs_params.noise_alpha);
}

// ── Chromatic Aberration ─────────────────────────────────────────────────────

struct ChromaticParams {
    strength: f32,
    center_falloff: f32,
    center_x: f32,
    center_y: f32,
}
@group(1) @binding(0) var<uniform> chromatic_params: ChromaticParams;

@compute @workgroup_size(16, 16)
fn effect_chromatic_aberration(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let center = vec2(chromatic_params.center_x, chromatic_params.center_y);
    let delta = uv - center;
    let falloff = mix(0.5 / (length(delta) + 0.0001), 1.0, chromatic_params.center_falloff);
    let offset = delta * chromatic_params.strength * 0.01 * falloff;

    let sc = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let s0 = textureSampleLevel(g_Texture0, g_Sampler, uv + offset, 0.0);
    let s1 = textureSampleLevel(g_Texture0, g_Sampler, uv - offset, 0.0);

    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(s0.r, sc.g, s1.b, sc.a);
}

// ── Fisheye ──────────────────────────────────────────────────────────────────

struct FisheyeParams {
    amount: f32,
}
@group(1) @binding(0) var<uniform> fisheye_params: FisheyeParams;

@compute @workgroup_size(16, 16)
fn effect_fisheye(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    var coords = uv * 2.0 - 1.0;
    let v = coords.x * coords.x + coords.y * coords.y;
    coords *= 1.0 + fisheye_params.amount * v;
    let tc = coords * 0.5 + 0.5;

    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Shimmer ──────────────────────────────────────────────────────────────────

struct ShimmerParams {
    strength: f32,
    speed: f32,
    scale: f32,
}
@group(1) @binding(0) var<uniform> shimmer_params: ShimmerParams;

@compute @workgroup_size(16, 16)
fn effect_shimmer(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let offset_x = sin(uv.y * shimmer_params.scale + globals.time * shimmer_params.speed) * shimmer_params.strength * 0.01;
    let offset_y = cos(uv.x * shimmer_params.scale * 0.7 + globals.time * shimmer_params.speed * 0.8) * shimmer_params.strength * 0.01;
    let tc = uv + vec2(offset_x, offset_y);

    dst[gid.y * u32(globals.tex_w) + gid.x] = textureSampleLevel(g_Texture0, g_Sampler, tc, 0.0);
}

// ── Nitro (radial blur) ──────────────────────────────────────────────────────

struct NitroParams {
    strength: f32,
    speed: f32,
    spread: f32,
}
@group(1) @binding(0) var<uniform> nitro_params: NitroParams;

@compute @workgroup_size(16, 16)
fn effect_nitro(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let center = vec2(0.5, 0.5);
    let delta = uv - center;
    let dist = length(delta);
    let dir = delta / max(dist, 0.0001);
    let blur_amount = dist * nitro_params.strength * 0.01;

    var color = vec4(0.0);
    for (var i = 0; i < 8; i++) {
        let t = f32(i) / 8.0;
        let offset = dir * blur_amount * t;
        color += textureSampleLevel(g_Texture0, g_Sampler, uv - offset, 0.0);
    }
    color /= 8.0;

    dst[gid.y * u32(globals.tex_w) + gid.x] = color;
}
