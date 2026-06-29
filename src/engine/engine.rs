//! Top-level scene engine (mirrors CScene from linux-wallpaperengine).
//! Owns all scene-level GPU state and drives the per-frame render loop.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::engine::{
    camera::SceneCamera,
    engine_object::{SceneObject, SceneObjectBuilder},
    fbo::{RenderTarget, RenderTargetPool},
    pass::ScenePass,
    render::ResolvedScene,
    resource::ResourceManager,
};

#[allow(dead_code)]
pub struct SceneEngine {
    pub resource_manager: Arc<ResourceManager>,
    camera: SceneCamera,
    fbo_pool: RenderTargetPool,
    objects: Vec<SceneObject>,    // sorted by render_order ascending
    scene_fbo: RenderTarget,      // final composited output
    clear_color: [f64; 3],
    global_time_secs: f64,
    scene_width: u32,
    scene_height: u32,
}

impl SceneEngine {
    /// Load a scene from a wallpaper directory (scene.json or scene.pkg).
    pub fn from_directory(
        dir: &Path,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self> {
        let rm = Arc::new(ResourceManager::new(device.clone(), queue.clone()));

        // ResolvedScene handles pkg/loose-file detection and texture loading.
        let resolved = ResolvedScene::from_directory(dir)?;
        let w = resolved.width;
        let h = resolved.height;

        let camera = SceneCamera::from_scene(&resolved.scene, (w, h));
        let scene_fbo = RenderTarget::new(rm.device(), "scene_output", w, h);

        let mut objects = Vec::new();
        for (i, layer) in resolved.layers.iter().enumerate() {
            let mut builder = SceneObjectBuilder::new(i as i32, layer.name.clone());
            builder.render_order = i as i32;
            builder.blend_mode   = layer.blend_mode;
            builder.copybackground = layer.copybackground;
            builder.world_origin = [
                layer.origin[0] as f32,
                layer.origin[1] as f32,
                layer.origin[2] as f32,
            ];
            builder.world_size = [layer.size[0] as f32, layer.size[1] as f32];

            // Upload base texture + animation frames.
            builder.base_frames.push(Arc::new(rm.upload_rgba(&layer.image)));
            for frame in &layer.extra_frames {
                builder.base_frames.push(Arc::new(rm.upload_rgba(frame)));
            }
            builder.frame_duration_ms = layer.frame_duration_ms;

            // Map scene effects to ScenePass descriptors.
            // Match by name to find the corresponding scene object's effects.
            if let Some(scene_obj) = resolved.scene.objects.iter().find(|o| {
                o.name.as_deref() == Some(layer.name.as_str())
            }) {
                for eff in &scene_obj.effects {
                    let effect_key = eff.file.as_deref().unwrap_or("unknown");
                    let pass = ScenePass::new(effect_key, vec![0u8; 16], vec![]);
                    builder.passes.push(pass);
                }
            }

            objects.push(builder.build(rm.device(), w, h));
        }

        objects.sort_by_key(|o| o.render_order);

        let clear_color = parse_clear_color(&resolved.scene);

        Ok(Self {
            resource_manager: rm,
            camera,
            fbo_pool: RenderTargetPool::new(),
            objects,
            scene_fbo,
            clear_color,
            global_time_secs: 0.0,
            scene_width: w,
            scene_height: h,
        })
    }

    /// Advance scene time and animation frames.
    pub fn tick(&mut self, dt_secs: f64) {
        self.global_time_secs += dt_secs;
        let dt_ms = dt_secs * 1000.0;
        for obj in &mut self.objects {
            obj.tick(dt_ms);
        }
    }

    /// Render one frame into `scene_fbo`. Caller is responsible for submitting the queue.
    ///
    /// Current state: scaffold — clears the scene FBO and per-object primary FBOs.
    /// Full implementation (Phase 7) will wire up GpuSceneRenderer pipelines for
    /// base-texture compositing, effect ping-pong, and blend-mode compositing.
    pub fn render_frame(&self, encoder: &mut wgpu::CommandEncoder) {
        // 1. Clear scene output.
        {
            let view = self.scene_fbo.view();
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene_clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.clear_color[0],
                            g: self.clear_color[1],
                            b: self.clear_color[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }

        // 2. Per-object passes.
        // TODO (Phase 7): blit current_texture() → primary_fbo, run effect passes
        // (ping-pong primary_fbo ↔ sub_fbo), composite primary_fbo → scene_fbo.
        for obj in &self.objects {
            if !obj.is_visible { continue; }
            let view = obj.primary_fbo.view();
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("obj_primary_clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
    }

    pub fn output_texture(&self) -> &wgpu::Texture { &self.scene_fbo.texture }
    pub fn scene_width(&self)  -> u32  { self.scene_width }
    pub fn scene_height(&self) -> u32  { self.scene_height }
    pub fn global_time(&self)  -> f64  { self.global_time_secs }
    pub fn object_count(&self) -> usize { self.objects.len() }

    /// Quad transform for a scene object: (offset_norm, size_norm).
    pub fn object_quad(&self, obj: &SceneObject) -> ([f32; 2], [f32; 2]) {
        self.camera.object_to_quad(obj.world_origin, obj.world_size)
    }
}

fn parse_clear_color(scene: &crate::engine::scene::Scene) -> [f64; 3] {
    if let Some(cc) = scene.general.as_ref().and_then(|g| g.clear_color.as_ref()) {
        if let Some(s) = cc.as_str() {
            let parts: Vec<f64> = s.split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if parts.len() >= 3 {
                return [parts[0], parts[1], parts[2]];
            }
        }
    }
    [0.0, 0.0, 0.0]
}
