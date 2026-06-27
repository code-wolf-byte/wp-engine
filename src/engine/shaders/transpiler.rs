// Agent output (qwen3-coder) — applied verbatim (clean output, no fixes needed).
// Requires naga = { version = "27", features = ["glsl-in", "wgsl-out"] } in Cargo.toml.
use anyhow::{Context, Result};
use std::collections::HashMap;
use crate::engine::model::ShaderModel;

#[derive(Debug)]
pub struct TranslatedShader {
    /// Translated WGSL source ready for wgpu::Device::create_shader_module.
    pub wgsl: String,
    /// Ordered list of (material_key, default_f32) matching the UBO slot layout.
    pub uniform_keys: Vec<(String, f32)>,
    /// Total number of texture bindings the shader expects.
    pub texture_count: usize,
}

pub fn translate(model: &ShaderModel) -> Result<TranslatedShader> {
    let preprocessed = preprocess(model);

    let mut frontend = naga::front::glsl::Frontend::default();
    let module = frontend.parse(
        &naga::front::glsl::Options {
            stage: naga::ShaderStage::Fragment,
            defines: naga::FastHashMap::default(),
        },
        &preprocessed,
    ).context("naga GLSL parse failed")?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    ).validate(&module).context("naga validation failed")?;

    let wgsl = naga::back::wgsl::write_string(
        &module,
        &info,
        naga::back::wgsl::WriterFlags::empty(),
    ).context("naga WGSL output failed")?;

    let uniform_keys = model.value_uniforms.iter()
        .map(|u| (u.material_key.clone(), u.default.as_float()))
        .collect();

    Ok(TranslatedShader {
        wgsl,
        uniform_keys,
        texture_count: model.texture_slots.len(),
    })
}

fn preprocess(model: &ShaderModel) -> String {
    let mut out = String::new();

    // Dialect compatibility preamble
    out.push_str("#version 450\n");
    out.push_str("#define frac(x) fract(x)\n");
    out.push_str("#define saturate(x) clamp((x), 0.0, 1.0)\n");
    out.push_str("#define CAST2(x) vec2(x)\n");
    out.push_str("#define CAST3(x) vec3(x)\n");
    out.push_str("#define CAST4(x) vec4(x)\n");
    out.push_str("#define texSample2D(s, uv) texture(s, uv)\n");
    out.push_str("#define lerp(a, b, t) mix(a, b, t)\n");
    out.push_str("#define mul(a, b) ((b) * (a))\n");
    out.push_str("layout(location=0) out vec4 fragColor;\n");

    // Combo #defines
    out.push_str(&combo_defines(&model.effective_combos()));

    // Process source line-by-line
    for line in model.frag_glsl.lines() {
        if line.trim_start().starts_with("// [COMBO]") {
            continue;
        }
        let processed = line
            .replace("varying ", "in ")
            .replace("gl_FragColor", "fragColor");
        out.push_str(&processed);
        out.push('\n');
    }

    out
}

pub fn combo_defines(combos: &HashMap<String, i32>) -> String {
    combos.iter()
        .map(|(name, value)| format!("#define {} {}\n", name, value))
        .collect()
}
