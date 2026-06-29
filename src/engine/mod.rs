//! Wallpaper Engine scene rendering engine.
//!
//! Core modules:
//!   [`pkg`]           — PKG archive extractor
//!   [`tex`]           — .tex texture decoder
//!   [`scene`]         — scene.json parser
//!   [`render`]        — CPU-side scene resolver (Layer)
//!   [`resource`]      — Central GPU resource cache (ResourceManager)
//!   [`fbo`]           — Named GPU render targets (RenderTarget / CFBO)
//!   [`camera`]        — Orthogonal scene camera (SceneCamera)
//!   [`pass`]          — Single render pass descriptor (ScenePass / CPass)
//!   [`engine_object`] — GPU scene object (SceneObject / CImage)
//!   [`engine`]        — Top-level scene engine (SceneEngine / CScene)
//!   [`gpu_renderer`]  — Low-level wgpu pipeline executor
//!   [`particle`]      — Particle system
//!   [`effect`]        — Shader effect definitions
//!   [`shaders`]       — GLSL → WGSL transpiler + loader

pub mod animated;
pub mod camera;
pub mod effect;
pub mod engine;
pub mod engine_object;
pub mod fbo;
pub mod gpu_renderer;
pub mod model;
pub mod particle;
pub mod pass;
pub mod pkg;
pub mod render;
pub mod resource;
pub mod scene;
pub mod script;
pub mod shaders;
pub mod tex;

// Convenience re-exports.
pub use camera::SceneCamera;
pub use engine::SceneEngine;
pub use engine_object::{SceneObject, SceneObjectBuilder};
pub use fbo::{RenderTarget, RenderTargetPool};
pub use pass::{ScenePass, UniformValue};
pub use pkg::Package;
pub use render::ResolvedScene;
pub use resource::ResourceManager;
pub use scene::Scene;
pub use tex::TexFile;

