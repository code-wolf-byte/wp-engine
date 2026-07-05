//! Wallpaper Engine scene rendering engine.
//!
//! This module currently contains both the existing Engine-branch GPU scene
//! renderer and the newer typed asset/material/render-graph porting layer.

pub mod animated;
pub mod assets;
pub mod blend;
pub mod camera;
pub mod camera_dynamics;
pub mod effect;
pub mod effect_diagnostics;
pub mod engine;
pub mod engine_object;
pub mod fbo;
pub mod frame_context;
pub mod gpu_renderer;
pub mod graph;
pub mod material;
pub mod model;
pub mod particle;
pub mod pass;
pub mod pkg;
pub mod properties;
pub mod render;
pub mod render_graph;
pub mod resource;
pub mod scene;
pub mod script;
pub mod shader;
pub mod shaders;
pub mod tex;
pub mod text;

// Convenience re-exports.
pub use camera::SceneCamera;
pub use camera_dynamics::{CameraDynamics, CameraFrameDynamics};
pub use engine::SceneEngine;
pub use engine_object::{SceneObject, SceneObjectBuilder};
pub use fbo::{RenderTarget, RenderTargetPool};
pub use frame_context::FrameContext;
pub use graph::SceneGraph;
pub use pass::{ScenePass, UniformValue};
pub use pkg::Package;
pub use properties::SceneProperties;
pub use render::ResolvedScene;
pub use render_graph::SceneRenderGraph;
pub use resource::ResourceManager;
pub use scene::Scene;
pub use tex::TexFile;
