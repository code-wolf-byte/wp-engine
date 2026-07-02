use std::collections::BTreeSet;

use super::effect::HandwrittenEffectFallback;
use super::graph::{ImageNode, SceneGraph};
use super::material::{EffectPass, Material, MaterialPass};
use super::shader::{ShaderDiagnosticSeverity, ShaderTranslator};

#[derive(Debug, Clone, Default)]
pub struct EffectDiagnosticsReport {
    pub effects: Vec<EffectDiagnostics>,
}

impl EffectDiagnosticsReport {
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for effect in &self.effects {
            lines.push(format!(
                "effect file={} object_index={} object='{}' name='{}' visible={} passes={} fbos={} fallback={}",
                effect.file,
                effect.object_index,
                if effect.object_name.is_empty() {
                    "<unnamed>"
                } else {
                    &effect.object_name
                },
                effect.name.as_deref().unwrap_or("<unnamed>"),
                effect.visible,
                effect.pass_count,
                effect.fbo_count,
                effect
                    .fallback
                    .map(HandwrittenEffectFallback::name)
                    .unwrap_or("none")
            ));

            lines.push(format!(
                "effect file={} uniforms={}",
                effect.file,
                comma_list(&effect.uniforms)
            ));
            lines.push(format!(
                "effect file={} samplers={}",
                effect.file,
                comma_list(&effect.samplers)
            ));
            lines.push(format!(
                "effect file={} render-targets={}",
                effect.file,
                comma_list(&effect.render_targets)
            ));

            for diagnostic in &effect.shader_diagnostics {
                lines.push(format!(
                    "effect file={} shader {} {:?}: {}",
                    effect.file, diagnostic.label, diagnostic.stage, diagnostic.message
                ));
            }
        }

        lines
    }

    pub fn missing_feature_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for effect in &self.effects {
            if effect.fallback.is_none() {
                lines.push(format!(
                    "missing effect fallback: file={} name='{}'",
                    effect.file,
                    effect.name.as_deref().unwrap_or("<unnamed>")
                ));
            }

            for diagnostic in &effect.shader_diagnostics {
                if diagnostic.is_error {
                    lines.push(format!(
                        "unsupported shader feature: file={} shader={} {:?}: {}",
                        effect.file, diagnostic.label, diagnostic.stage, diagnostic.message
                    ));
                }
            }
        }

        lines
    }
}

#[derive(Debug, Clone)]
pub struct EffectDiagnostics {
    pub object_index: usize,
    pub object_name: String,
    pub file: String,
    pub name: Option<String>,
    pub visible: bool,
    pub pass_count: usize,
    pub fbo_count: usize,
    pub uniforms: Vec<String>,
    pub samplers: Vec<String>,
    pub render_targets: Vec<String>,
    pub fallback: Option<HandwrittenEffectFallback>,
    pub shader_diagnostics: Vec<EffectShaderDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct EffectShaderDiagnostic {
    pub label: String,
    pub stage: super::shader::ShaderStage,
    pub message: String,
    pub is_error: bool,
}

pub fn collect_effect_diagnostics(graph: &SceneGraph) -> EffectDiagnosticsReport {
    let translator = ShaderTranslator;
    let mut report = EffectDiagnosticsReport::default();

    for node in &graph.images {
        for effect in &node.effects {
            let mut uniforms = BTreeSet::new();
            let mut samplers = BTreeSet::new();
            let mut render_targets = BTreeSet::new();
            let mut shader_diagnostics = Vec::new();

            for fbo in &effect.definition.fbos {
                render_targets.insert(format!(
                    "fbo:{} scale={} format={} unique={}",
                    fbo.name, fbo.scale, fbo.format, fbo.unique
                ));
            }

            for (pass_index, pass) in effect.definition.passes.iter().enumerate() {
                collect_effect_pass_usage(pass_index, pass, &mut samplers, &mut render_targets);
                if let Some(material) = &pass.material {
                    collect_material_usage(material, &mut uniforms, &mut samplers);
                    collect_shader_diagnostics(
                        graph,
                        material,
                        &translator,
                        &mut shader_diagnostics,
                    );
                }
            }

            collect_scene_override_uniforms(graph, node, effect.id, &mut uniforms);

            let fallback = HandwrittenEffectFallback::from_label(&effect.definition.filename)
                .or_else(|| {
                    effect
                        .name
                        .as_deref()
                        .and_then(HandwrittenEffectFallback::from_label)
                })
                .or_else(|| HandwrittenEffectFallback::from_label(&effect.definition.name));

            report.effects.push(EffectDiagnostics {
                object_index: node.object_index,
                object_name: node.name.clone(),
                file: effect.definition.filename.clone(),
                name: effect.name.clone().or_else(|| {
                    (!effect.definition.name.is_empty()).then(|| effect.definition.name.clone())
                }),
                visible: effect.visible,
                pass_count: effect.definition.passes.len(),
                fbo_count: effect.definition.fbos.len(),
                uniforms: uniforms.into_iter().collect(),
                samplers: samplers.into_iter().collect(),
                render_targets: render_targets.into_iter().collect(),
                fallback,
                shader_diagnostics,
            });
        }
    }

    report
}

fn collect_effect_pass_usage(
    pass_index: usize,
    pass: &EffectPass,
    samplers: &mut BTreeSet<String>,
    render_targets: &mut BTreeSet<String>,
) {
    for (slot, name) in &pass.binds {
        samplers.insert(format!("pass{pass_index}:bind{slot}={name}"));
        if is_render_target_name(name) {
            render_targets.insert(format!("pass{pass_index}:bind{slot}={name}"));
        }
    }

    if let Some(source) = &pass.source {
        render_targets.insert(format!("pass{pass_index}:source={source}"));
    }
    if let Some(target) = &pass.target {
        render_targets.insert(format!("pass{pass_index}:target={target}"));
    }
    if let Some(command) = pass.command {
        render_targets.insert(format!("pass{pass_index}:command={command:?}"));
    }
}

fn collect_material_usage(
    material: &Material,
    uniforms: &mut BTreeSet<String>,
    samplers: &mut BTreeSet<String>,
) {
    for (pass_index, pass) in material.passes.iter().enumerate() {
        collect_material_pass_usage(pass_index, pass, uniforms, samplers);
    }
}

fn collect_material_pass_usage(
    pass_index: usize,
    pass: &MaterialPass,
    uniforms: &mut BTreeSet<String>,
    samplers: &mut BTreeSet<String>,
) {
    for key in pass.constants.keys() {
        uniforms.insert(format!("pass{pass_index}:constant:{key}"));
    }
    for key in pass.combos.keys() {
        uniforms.insert(format!("pass{pass_index}:combo:{key}"));
    }
    for (slot, name) in &pass.textures {
        samplers.insert(format!("pass{pass_index}:texture{slot}={name}"));
    }
    for (slot, name) in &pass.usertextures {
        samplers.insert(format!("pass{pass_index}:usertexture{slot}={name}"));
    }
}

fn collect_shader_diagnostics(
    graph: &SceneGraph,
    material: &Material,
    translator: &ShaderTranslator,
    shader_diagnostics: &mut Vec<EffectShaderDiagnostic>,
) {
    for pass in &material.passes {
        for translation in translator.translate_material_pass(&graph.assets, pass) {
            if !translation.succeeded() {
                shader_diagnostics.push(EffectShaderDiagnostic {
                    label: translation.label.clone(),
                    stage: translation.stage,
                    message:
                        "translation failed; using handwritten/builtin fallback when available"
                            .to_string(),
                    is_error: true,
                });
            }

            for diagnostic in translation.diagnostics {
                let is_error = diagnostic.severity == ShaderDiagnosticSeverity::Error;
                shader_diagnostics.push(EffectShaderDiagnostic {
                    label: translation.label.clone(),
                    stage: translation.stage,
                    message: diagnostic.message,
                    is_error,
                });
            }
        }
    }
}

fn collect_scene_override_uniforms(
    graph: &SceneGraph,
    node: &ImageNode,
    effect_id: Option<i64>,
    uniforms: &mut BTreeSet<String>,
) {
    let Some(object) = graph.scene.objects.get(node.object_index) else {
        return;
    };

    for effect in &object.effects {
        if effect_id.is_some() && effect.id != effect_id {
            continue;
        }

        for (pass_index, pass) in effect.passes.iter().enumerate() {
            for key in pass.constantshadervalues.keys() {
                uniforms.insert(format!("scene-pass{pass_index}:constant:{key}"));
            }
            for (texture_index, texture) in pass.textures.iter().enumerate() {
                if let Some(name) = texture.file.as_deref().or(texture.name.as_deref()) {
                    uniforms.insert(format!("scene-pass{pass_index}:texture-ref{texture_index}"));
                    if is_render_target_name(name) {
                        uniforms.insert(format!(
                            "scene-pass{pass_index}:render-target-texture:{name}"
                        ));
                    }
                }
            }
        }
    }
}

fn is_render_target_name(name: &str) -> bool {
    name.starts_with("_rt_") || name.starts_with("_alias_")
}

fn comma_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(",")
    }
}
