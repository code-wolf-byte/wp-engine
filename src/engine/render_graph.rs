#![allow(dead_code)]

use std::collections::BTreeMap;

use super::graph::{ImageNode, SceneGraph};
use super::material::{MaterialPass, PassCommand};

#[derive(Debug, Clone)]
pub struct SceneRenderGraph {
    pub size: [u32; 2],
    pub objects: Vec<RenderObjectNode>,
    pub passes: Vec<RenderPassNode>,
    pub targets: BTreeMap<String, RenderTargetDesc>,
    pub final_pass: FinalCompositePass,
    pub diagnostics: Vec<RenderGraphDiagnostic>,
}

impl SceneRenderGraph {
    pub fn from_scene_graph(graph: &SceneGraph) -> Self {
        let (width, height) = graph.render_size();
        let mut builder = RenderGraphBuilder {
            size: [width, height],
            objects: Vec::new(),
            passes: Vec::new(),
            targets: BTreeMap::new(),
            diagnostics: Vec::new(),
        };

        for node in &graph.images {
            builder.add_image_node(graph, node);
        }

        Self {
            size: builder.size,
            objects: builder.objects,
            passes: builder.passes,
            targets: builder.targets,
            final_pass: FinalCompositePass {
                inputs: vec![RenderTargetRef::Backbuffer],
                target: RenderTargetRef::Output,
            },
            diagnostics: builder.diagnostics,
        }
    }

    pub fn first_textured_object(&self) -> Option<&RenderObjectNode> {
        self.objects
            .iter()
            .find(|object| object.base_texture.is_some() && object.model.is_some())
    }
}

struct RenderGraphBuilder {
    size: [u32; 2],
    objects: Vec<RenderObjectNode>,
    passes: Vec<RenderPassNode>,
    targets: BTreeMap<String, RenderTargetDesc>,
    diagnostics: Vec<RenderGraphDiagnostic>,
}

impl RenderGraphBuilder {
    fn add_image_node(&mut self, graph: &SceneGraph, node: &ImageNode) {
        let object = graph.object(node);
        let object_id = self.objects.len();
        self.objects.push(RenderObjectNode {
            id: object_id,
            scene_object_index: node.object_index,
            scene_object_id: node.id,
            name: if node.name.is_empty() {
                object.name.clone().unwrap_or_default()
            } else {
                node.name.clone()
            },
            model: node.model.as_ref().map(|model| model.filename.clone()),
            base_texture: node.base_texture.clone(),
            fullscreen: node.is_fullscreen_layer(),
            render_target_layer: node.is_render_target_layer(),
        });

        if node.base_texture.is_none() {
            self.diagnostics
                .push(RenderGraphDiagnostic::missing_texture(
                    object_id,
                    "image node has no resolved base texture",
                ));
        }

        if let Some(model) = &node.model {
            for (pass_index, pass) in model.material.passes.iter().enumerate() {
                self.passes.push(RenderPassNode::from_material_pass(
                    object_id,
                    pass_index,
                    &model.material.filename,
                    pass,
                ));
            }
        } else if !node.is_render_target_layer() {
            self.diagnostics.push(RenderGraphDiagnostic::missing_model(
                object_id,
                "image node has no model; WGPU path currently expects model-backed layers",
            ));
        }

        for effect in &node.effects {
            for fbo in &effect.definition.fbos {
                self.targets.insert(
                    fbo.name.clone(),
                    RenderTargetDesc {
                        name: fbo.name.clone(),
                        scale: fbo.scale,
                        format: fbo.format.clone(),
                        unique: fbo.unique,
                    },
                );
            }

            for (pass_index, pass) in effect.definition.passes.iter().enumerate() {
                self.passes.push(RenderPassNode {
                    id: self.passes.len(),
                    object_id: Some(object_id),
                    kind: match pass.command {
                        Some(PassCommand::Copy) => RenderPassKind::Copy,
                        Some(PassCommand::Swap) => RenderPassKind::Swap,
                        None => RenderPassKind::Effect,
                    },
                    label: format!(
                        "{}:{}",
                        effect
                            .name
                            .as_deref()
                            .unwrap_or(&effect.definition.filename),
                        pass_index
                    ),
                    material: pass
                        .material
                        .as_ref()
                        .map(|material| material.filename.clone()),
                    shader: pass
                        .material
                        .as_ref()
                        .and_then(|material| material.passes.first())
                        .and_then(|pass| pass.shader.clone()),
                    textures: pass
                        .binds
                        .iter()
                        .map(|(slot, name)| TextureBinding {
                            slot: *slot,
                            name: name.clone(),
                            source: texture_source(name),
                        })
                        .collect(),
                    target: pass
                        .target
                        .as_ref()
                        .map(|name| RenderTargetRef::Named(name.clone()))
                        .unwrap_or(RenderTargetRef::Backbuffer),
                    source: pass
                        .source
                        .as_ref()
                        .map(|name| RenderTargetRef::Named(name.clone())),
                    blending: None,
                    depth_test: None,
                    depth_write: None,
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderObjectNode {
    pub id: usize,
    pub scene_object_index: usize,
    pub scene_object_id: Option<i64>,
    pub name: String,
    pub model: Option<String>,
    pub base_texture: Option<String>,
    pub fullscreen: bool,
    pub render_target_layer: bool,
}

#[derive(Debug, Clone)]
pub struct RenderPassNode {
    pub id: usize,
    pub object_id: Option<usize>,
    pub kind: RenderPassKind,
    pub label: String,
    pub material: Option<String>,
    pub shader: Option<String>,
    pub textures: Vec<TextureBinding>,
    pub source: Option<RenderTargetRef>,
    pub target: RenderTargetRef,
    pub blending: Option<String>,
    pub depth_test: Option<String>,
    pub depth_write: Option<String>,
}

impl RenderPassNode {
    fn from_material_pass(
        object_id: usize,
        pass_index: usize,
        material_filename: &str,
        pass: &MaterialPass,
    ) -> Self {
        let mut textures: Vec<TextureBinding> = pass
            .textures
            .iter()
            .map(|(slot, name)| TextureBinding {
                slot: *slot,
                name: name.clone(),
                source: texture_source(name),
            })
            .collect();

        textures.extend(pass.usertextures.iter().map(|(slot, name)| TextureBinding {
            slot: *slot,
            name: name.clone(),
            source: TextureSource::UserTexture,
        }));

        Self {
            id: pass_index,
            object_id: Some(object_id),
            kind: RenderPassKind::Material,
            label: format!("{material_filename}:{pass_index}"),
            material: Some(material_filename.to_string()),
            shader: pass.shader.clone(),
            textures,
            source: None,
            target: RenderTargetRef::Backbuffer,
            blending: Some(pass.blending.clone()),
            depth_test: Some(pass.depthtest.clone()),
            depth_write: Some(pass.depthwrite.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPassKind {
    Material,
    Effect,
    Copy,
    Swap,
}

#[derive(Debug, Clone)]
pub struct TextureBinding {
    pub slot: u32,
    pub name: String,
    pub source: TextureSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSource {
    MaterialTexture,
    UserTexture,
    RenderTarget,
    EngineRuntime,
}

#[derive(Debug, Clone)]
pub struct RenderTargetDesc {
    pub name: String,
    pub scale: f32,
    pub format: String,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderTargetRef {
    Backbuffer,
    Output,
    Named(String),
}

#[derive(Debug, Clone)]
pub struct FinalCompositePass {
    pub inputs: Vec<RenderTargetRef>,
    pub target: RenderTargetRef,
}

#[derive(Debug, Clone)]
pub struct RenderGraphDiagnostic {
    pub object_id: Option<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

impl RenderGraphDiagnostic {
    fn missing_model(object_id: usize, message: impl Into<String>) -> Self {
        Self {
            object_id: Some(object_id),
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
        }
    }

    fn missing_texture(object_id: usize, message: impl Into<String>) -> Self {
        Self {
            object_id: Some(object_id),
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

fn texture_source(name: &str) -> TextureSource {
    if name.starts_with("_rt_") || name.starts_with("_alias_") {
        TextureSource::RenderTarget
    } else if name.starts_with('$') {
        TextureSource::EngineRuntime
    } else {
        TextureSource::MaterialTexture
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::engine::{SceneGraph, SceneRenderGraph};

    #[test]
    fn builds_graph_from_minimal_scene_fixture() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "wp-engine-render-graph-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models"))?;
        std::fs::create_dir_all(root.join("materials"))?;
        std::fs::write(
            root.join("scene.json"),
            r#"{
                "general": {"orthogonalprojection": {"width": 64, "height": 32}},
                "objects": [{
                    "id": 7,
                    "name": "Fixture Image",
                    "visible": true,
                    "image": "models/image.json"
                }]
            }"#,
        )?;
        std::fs::write(
            root.join("models/image.json"),
            r#"{
                "material": "materials/image.json",
                "fullscreen": true,
                "width": 64,
                "height": 32
            }"#,
        )?;
        std::fs::write(
            root.join("materials/image.json"),
            r#"{
                "passes": [{
                    "shader": "genericimage2",
                    "textures": ["sample"]
                }]
            }"#,
        )?;

        let graph = SceneGraph::from_directory(&root)?;
        let render_graph = SceneRenderGraph::from_scene_graph(&graph);
        let object = render_graph
            .first_textured_object()
            .expect("fixture should produce a textured object");

        assert_eq!(render_graph.size, [64, 32]);
        assert_eq!(object.name, "Fixture Image");
        assert_eq!(object.base_texture.as_deref(), Some("sample"));
        assert_eq!(render_graph.passes.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
