#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::BTreeMap;

use anyhow::Result;

use super::assets::AssetStore;
use super::material::MaterialPass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderLanguage {
    Glsl,
    Wgsl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

impl ShaderStage {
    fn naga(self) -> naga::ShaderStage {
        match self {
            Self::Vertex => naga::ShaderStage::Vertex,
            Self::Fragment => naga::ShaderStage::Fragment,
            Self::Compute => naga::ShaderStage::Compute,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Vertex => "vert",
            Self::Fragment => "frag",
            Self::Compute => "comp",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShaderTranslationRequest<'a> {
    pub label: Cow<'a, str>,
    pub language: ShaderLanguage,
    pub stage: ShaderStage,
    pub source: Cow<'a, str>,
    pub defines: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ShaderTranslation {
    pub label: String,
    pub language: ShaderLanguage,
    pub stage: ShaderStage,
    pub wgsl: Option<String>,
    pub diagnostics: Vec<ShaderDiagnostic>,
}

impl ShaderTranslation {
    pub fn succeeded(&self) -> bool {
        self.wgsl.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct ShaderDiagnostic {
    pub severity: ShaderDiagnosticSeverity,
    pub message: String,
}

impl ShaderDiagnostic {
    fn info(message: impl Into<String>) -> Self {
        Self {
            severity: ShaderDiagnosticSeverity::Info,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: ShaderDiagnosticSeverity::Error,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderDiagnosticSeverity {
    Info,
    Error,
}

#[derive(Debug, Default)]
pub struct ShaderTranslator;

impl ShaderTranslator {
    pub fn translate(&self, request: ShaderTranslationRequest<'_>) -> ShaderTranslation {
        let mut diagnostics = Vec::new();
        let wgsl = match request.language {
            ShaderLanguage::Glsl => self.glsl_to_wgsl(&request, &mut diagnostics),
            ShaderLanguage::Wgsl => self.validate_wgsl(&request, &mut diagnostics),
        };

        ShaderTranslation {
            label: request.label.into_owned(),
            language: request.language,
            stage: request.stage,
            wgsl,
            diagnostics,
        }
    }

    pub fn translate_material_pass(
        &self,
        assets: &AssetStore,
        pass: &MaterialPass,
    ) -> Vec<ShaderTranslation> {
        let Some(shader) = pass.shader.as_deref() else {
            return vec![missing_shader(
                "<material pass>",
                "pass has no shader field",
            )];
        };

        [ShaderStage::Vertex, ShaderStage::Fragment]
            .into_iter()
            .map(|stage| self.translate_shader_ref(assets, shader, stage, &pass.combos))
            .collect()
    }

    pub fn translate_shader_ref(
        &self,
        assets: &AssetStore,
        shader: &str,
        stage: ShaderStage,
        defines: &BTreeMap<String, i32>,
    ) -> ShaderTranslation {
        let mut diagnostics = Vec::new();
        for candidate in shader_candidates(shader, stage) {
            match assets.read_string(&candidate) {
                Ok(source) => {
                    let request = ShaderTranslationRequest {
                        label: Cow::Owned(candidate),
                        language: shader_language_from_path(shader),
                        stage,
                        source: Cow::Owned(source),
                        defines: defines
                            .iter()
                            .map(|(key, value)| (key.clone(), value.to_string()))
                            .collect(),
                    };
                    return self.translate(request);
                }
                Err(err) => diagnostics.push(ShaderDiagnostic::info(format!(
                    "candidate not found: {candidate}: {err}"
                ))),
            }
        }

        ShaderTranslation {
            label: shader.to_string(),
            language: ShaderLanguage::Glsl,
            stage,
            wgsl: None,
            diagnostics: {
                diagnostics.push(ShaderDiagnostic::error(format!(
                    "could not resolve shader source for {shader}.{}",
                    stage.extension()
                )));
                diagnostics
            },
        }
    }

    fn glsl_to_wgsl(
        &self,
        request: &ShaderTranslationRequest<'_>,
        diagnostics: &mut Vec<ShaderDiagnostic>,
    ) -> Option<String> {
        let mut frontend = naga::front::glsl::Frontend::default();
        let mut options = naga::front::glsl::Options::from(request.stage.naga());
        for (key, value) in &request.defines {
            options.defines.insert(key.clone(), value.clone());
        }

        let source = ensure_glsl_version(&request.source);
        let module = match frontend.parse(&options, &source) {
            Ok(module) => module,
            Err(err) => {
                diagnostics.push(ShaderDiagnostic::error(format!(
                    "GLSL parse failed: {err:?}"
                )));
                return None;
            }
        };

        self.module_to_wgsl(&module, diagnostics)
    }

    fn validate_wgsl(
        &self,
        request: &ShaderTranslationRequest<'_>,
        diagnostics: &mut Vec<ShaderDiagnostic>,
    ) -> Option<String> {
        let module = match naga::front::wgsl::parse_str(&request.source) {
            Ok(module) => module,
            Err(err) => {
                diagnostics.push(ShaderDiagnostic::error(format!(
                    "WGSL parse failed: {err:?}"
                )));
                return None;
            }
        };

        self.module_to_wgsl(&module, diagnostics)
    }

    fn module_to_wgsl(
        &self,
        module: &naga::Module,
        diagnostics: &mut Vec<ShaderDiagnostic>,
    ) -> Option<String> {
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        );
        let info = match validator.validate(module) {
            Ok(info) => info,
            Err(err) => {
                diagnostics.push(ShaderDiagnostic::error(format!(
                    "Naga validation failed: {err:?}"
                )));
                return None;
            }
        };

        match naga::back::wgsl::write_string(module, &info, naga::back::wgsl::WriterFlags::empty())
        {
            Ok(wgsl) => Some(wgsl),
            Err(err) => {
                diagnostics.push(ShaderDiagnostic::error(format!(
                    "WGSL emission failed: {err:?}"
                )));
                None
            }
        }
    }
}

pub fn textured_quad_wgsl() -> &'static str {
    r#"
struct FrameUniforms {
    resolution_time_delta: vec4<f32>,
    frame_mouse: vec4<f32>,
    camera_eye_parallax: vec4<f32>,
    camera_center_audio: vec4<f32>,
    object_origin_alpha: vec4<f32>,
    object_size_flags: vec4<f32>,
    object_color_tint: vec4<f32>,
    audio_bands: vec4<f32>,
    model: mat4x4<f32>,
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: FrameUniforms;

@group(1) @binding(0)
var base_texture: texture_2d<f32>;

@group(1) @binding(1)
var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(base_texture, base_sampler, input.uv);
}

@fragment
fn fs_effect_tint(input: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(base_texture, base_sampler, input.uv);
    return vec4<f32>(color.rgb * frame.object_color_tint.rgb, color.a * frame.object_color_tint.a);
}

@fragment
fn fs_effect_opacity(input: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(base_texture, base_sampler, input.uv);
    color.a = color.a * frame.object_origin_alpha.w;
    return color;
}

@fragment
fn fs_effect_pulse(input: VertexOutput) -> @location(0) vec4<f32> {
    let time = frame.resolution_time_delta.z;
    let pulse = 1.0 + sin(time * 2.0) * 0.08;
    let color = textureSample(base_texture, base_sampler, input.uv);
    return vec4<f32>(color.rgb * pulse, color.a);
}

@fragment
fn fs_effect_shake(input: VertexOutput) -> @location(0) vec4<f32> {
    let time = frame.resolution_time_delta.z;
    let px = vec2<f32>(1.0 / max(frame.resolution_time_delta.x, 1.0), 1.0 / max(frame.resolution_time_delta.y, 1.0));
    let offset = vec2<f32>(sin(time * 19.0), cos(time * 17.0)) * px * 4.0;
    return textureSample(base_texture, base_sampler, input.uv + offset);
}
"#
}

pub fn validate_builtin_textured_quad() -> Result<()> {
    let translator = ShaderTranslator;
    let translation = translator.translate(ShaderTranslationRequest {
        label: Cow::Borrowed("builtin/textured_quad.wgsl"),
        language: ShaderLanguage::Wgsl,
        stage: ShaderStage::Fragment,
        source: Cow::Borrowed(textured_quad_wgsl()),
        defines: BTreeMap::new(),
    });

    if translation.succeeded() {
        Ok(())
    } else {
        anyhow::bail!(
            "builtin textured quad shader failed validation: {:?}",
            translation.diagnostics
        )
    }
}

fn shader_candidates(shader: &str, stage: ShaderStage) -> Vec<String> {
    let normalized = shader
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string();
    let extension = stage.extension();
    let mut candidates = Vec::new();

    if normalized.starts_with("shaders/") {
        candidates.push(replace_extension(&normalized, extension));
    } else {
        candidates.push(format!(
            "shaders/{}",
            replace_extension(&normalized, extension)
        ));
        candidates.push(replace_extension(&normalized, extension));
    }

    candidates.dedup();
    candidates
}

fn replace_extension(path: &str, extension: &str) -> String {
    match path.rsplit_once('.') {
        Some((base, _)) => format!("{base}.{extension}"),
        None => format!("{path}.{extension}"),
    }
}

fn shader_language_from_path(path: &str) -> ShaderLanguage {
    if path.ends_with(".wgsl") {
        ShaderLanguage::Wgsl
    } else {
        ShaderLanguage::Glsl
    }
}

fn ensure_glsl_version(source: &str) -> Cow<'_, str> {
    if source
        .lines()
        .any(|line| line.trim_start().starts_with("#version"))
    {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(format!("#version 450 core\n{source}"))
    }
}

fn missing_shader(label: &str, message: &str) -> ShaderTranslation {
    ShaderTranslation {
        label: label.to_string(),
        language: ShaderLanguage::Glsl,
        stage: ShaderStage::Fragment,
        wgsl: None,
        diagnostics: vec![ShaderDiagnostic::error(message)],
    }
}
