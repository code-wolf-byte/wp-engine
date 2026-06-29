//! GPU-side scene object (mirrors CImage from linux-wallpaperengine).
//! Replaces the CPU-side Layer struct with a GPU-ready representation that holds
//! per-object FBOs for ping-pong effect chaining.

#![allow(dead_code)]

use std::sync::Arc;

use crate::engine::fbo::RenderTarget;
use crate::engine::pass::ScenePass;

/// One scene object ready for GPU rendering.
pub struct SceneObject {
    pub id: i32,
    pub name: String,
    pub render_order: i32,
    pub is_visible: bool,
    pub blend_mode: u32,
    pub opacity: f32,
    /// Object center in WE world coords (Y-up, origin at scene center).
    pub world_origin: [f32; 3],
    /// Width and height in world units.
    pub world_size: [f32; 2],
    pub angle_degrees: f32,
    pub copybackground: bool,

    /// Animation frames. len==1 → static; len>1 → animated via `current_frame`.
    pub base_frames: Vec<Arc<wgpu::Texture>>,
    /// Duration of each frame in milliseconds. 0 = static.
    pub frame_duration_ms: u32,
    pub current_frame: usize,

    /// Ping-pong render targets for effect pass chaining.
    pub primary_fbo: RenderTarget,
    pub sub_fbo: RenderTarget,

    /// Effect passes to apply after the base texture is composited.
    pub passes: Vec<ScenePass>,
}

impl SceneObject {
    /// Advance animation by `dt_ms` milliseconds.
    pub fn tick(&mut self, _dt_ms: f64) {
        if self.frame_duration_ms == 0 || self.base_frames.len() <= 1 { return; }
        self.current_frame = (self.current_frame + 1) % self.base_frames.len();
    }

    /// Return the texture for the current animation frame.
    pub fn current_texture(&self) -> &wgpu::Texture {
        &self.base_frames[self.current_frame.min(self.base_frames.len().saturating_sub(1))]
    }

    pub fn has_effects(&self) -> bool { !self.passes.is_empty() }

    pub fn is_animated(&self) -> bool {
        self.base_frames.len() > 1 && self.frame_duration_ms > 0
    }
}

/// Builder for constructing [`SceneObject`]s from parsed scene data.
pub struct SceneObjectBuilder {
    pub id: i32,
    pub name: String,
    pub render_order: i32,
    pub is_visible: bool,
    pub blend_mode: u32,
    pub opacity: f32,
    pub world_origin: [f32; 3],
    pub world_size: [f32; 2],
    pub angle_degrees: f32,
    pub copybackground: bool,
    pub base_frames: Vec<Arc<wgpu::Texture>>,
    pub frame_duration_ms: u32,
    pub passes: Vec<ScenePass>,
}

impl SceneObjectBuilder {
    pub fn new(id: i32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            render_order: 0,
            is_visible: true,
            blend_mode: 0,
            opacity: 1.0,
            world_origin: [0.0, 0.0, 0.0],
            world_size: [0.0, 0.0],
            angle_degrees: 0.0,
            copybackground: false,
            base_frames: vec![],
            frame_duration_ms: 0,
            passes: vec![],
        }
    }

    /// Allocate per-object FBOs at scene dimensions and assemble the [`SceneObject`].
    pub fn build(self, device: &wgpu::Device, w: u32, h: u32) -> SceneObject {
        let primary_fbo = RenderTarget::new(device, format!("{}_primary", self.name), w, h);
        let sub_fbo     = RenderTarget::new(device, format!("{}_sub",     self.name), w, h);
        SceneObject {
            id: self.id,
            name: self.name,
            render_order: self.render_order,
            is_visible: self.is_visible,
            blend_mode: self.blend_mode,
            opacity: self.opacity,
            world_origin: self.world_origin,
            world_size: self.world_size,
            angle_degrees: self.angle_degrees,
            copybackground: self.copybackground,
            base_frames: self.base_frames,
            frame_duration_ms: self.frame_duration_ms,
            current_frame: 0,
            primary_fbo,
            sub_fbo,
            passes: self.passes,
        }
    }
}
