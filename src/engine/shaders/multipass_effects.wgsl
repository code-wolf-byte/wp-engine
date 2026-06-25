// Multi-pass effects: godrays (5 passes), shine (5 passes), blur (3 passes)
// Each pass reads from a source texture and writes to the output buffer.
// The pipeline chains: source -> downsample -> process -> gaussian -> combine

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

// ── Shared: Downsample (quarter resolution) ──────────────────────────────────

@compute @workgroup_size(16, 16)
fn pass_downsample(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let texel = 1.0 / vec2(globals.tex_w, globals.tex_h);

    var color = textureSampleLevel(g_Texture0, g_Sampler, uv + vec2(-texel.x, -texel.y), 0.0);
    color += textureSampleLevel(g_Texture0, g_Sampler, uv + vec2(texel.x, -texel.y), 0.0);
    color += textureSampleLevel(g_Texture0, g_Sampler, uv + vec2(-texel.x, texel.y), 0.0);
    color += textureSampleLevel(g_Texture0, g_Sampler, uv + vec2(texel.x, texel.y), 0.0);
    color *= 0.25;

    dst[gid.y * u32(globals.tex_w) + gid.x] = color;
}

// ── Shared: Gaussian blur (horizontal or vertical) ───────────────────────────

struct GaussianParams {
    direction_x: f32,
    direction_y: f32,
}
@group(1) @binding(0) var<uniform> gaussian_params: GaussianParams;

@compute @workgroup_size(16, 16)
fn pass_gaussian(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);
    let dir = vec2(gaussian_params.direction_x, gaussian_params.direction_y) / vec2(globals.tex_w, globals.tex_h);

    let weights = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);

    var color = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0) * weights[0];
    for (var i = 1; i < 5; i++) {
        let offset = dir * f32(i);
        color += textureSampleLevel(g_Texture0, g_Sampler, uv + offset, 0.0) * weights[i];
        color += textureSampleLevel(g_Texture0, g_Sampler, uv - offset, 0.0) * weights[i];
    }

    dst[gid.y * u32(globals.tex_w) + gid.x] = color;
}

// ── Godrays: cast (radial light extraction) ──────────────────────────────────

struct GodraysCastParams {
    threshold: f32,
    center_x: f32,
    center_y: f32,
}
@group(1) @binding(0) var<uniform> godrays_cast_params: GodraysCastParams;

@compute @workgroup_size(16, 16)
fn pass_godrays_cast(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let center = vec2(godrays_cast_params.center_x, godrays_cast_params.center_y);
    let delta = uv - center;
    let dir = normalize(delta);
    let dist = length(delta);

    var color = vec4(0.0);
    let samples = 16;
    for (var i = 0; i < samples; i++) {
        let t = f32(i) / f32(samples) * 0.1;
        let sample_uv = uv - dir * t;
        let s = textureSampleLevel(g_Texture0, g_Sampler, sample_uv, 0.0);
        let lum = dot(s.rgb, vec3(0.299, 0.587, 0.114));
        let bright = max(lum - godrays_cast_params.threshold, 0.0);
        color += vec4(s.rgb * bright, s.a);
    }
    color /= f32(samples);

    dst[gid.y * u32(globals.tex_w) + gid.x] = color;
}

// ── Godrays/Shine: combine (add processed result to original) ────────────────

struct CombineParams {
    strength: f32,
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
}
@group(1) @binding(0) var<uniform> combine_params: CombineParams;
@group(1) @binding(1) var g_BlurredTex: texture_2d<f32>;

@compute @workgroup_size(16, 16)
fn pass_combine(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let original = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let blurred = textureSampleLevel(g_BlurredTex, g_Sampler, uv, 0.0);
    let tint = vec3(combine_params.tint_r, combine_params.tint_g, combine_params.tint_b);

    let result = original.rgb + blurred.rgb * combine_params.strength * tint;
    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(min(result, vec3(1.0)), original.a);
}

// ── Shine: cast (brightness extraction) ──────────────────────────────────────

struct ShineCastParams {
    threshold: f32,
    power: f32,
}
@group(1) @binding(0) var<uniform> shine_cast_params: ShineCastParams;

@compute @workgroup_size(16, 16)
fn pass_shine_cast(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let sample = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let lum = dot(sample.rgb, vec3(0.299, 0.587, 0.114));
    let bright = pow(max(lum - shine_cast_params.threshold, 0.0), shine_cast_params.power);

    dst[gid.y * u32(globals.tex_w) + gid.x] = vec4(sample.rgb * bright, sample.a);
}

// ── Blur: combine (mix blurred with original) ────────────────────────────────

struct BlurCombineParams {
    strength: f32,
}
@group(1) @binding(0) var<uniform> blur_combine_params: BlurCombineParams;
@group(1) @binding(1) var g_BlurSrc: texture_2d<f32>;

@compute @workgroup_size(16, 16)
fn pass_blur_combine(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= u32(globals.tex_w) || gid.y >= u32(globals.tex_h) { return; }
    let uv = (vec2<f32>(vec2(gid.xy)) + 0.5) / vec2(globals.tex_w, globals.tex_h);

    let original = textureSampleLevel(g_Texture0, g_Sampler, uv, 0.0);
    let blurred = textureSampleLevel(g_BlurSrc, g_Sampler, uv, 0.0);

    let result = mix(original, blurred, blur_combine_params.strength);
    dst[gid.y * u32(globals.tex_w) + gid.x] = result;
}
