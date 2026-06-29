//! Wallpaper Engine scene rendering engine.
//!
//! Phases:
//!   [`pkg`]      — PKG archive extractor        (Phase 1) ✓
//!   [`tex`]      — .tex texture decoder          (Phase 2) ✓
//!   [`resource`] — Central resource cache and management (NEW) (Phase 0) ✅
//!   [`scene`]    — scene.json parser             (Phase 3) ✓
//!   [`render`]   — structured GPU rendering pipeline (Phase 4) ✓
//!   [`particle`] — particle system               (Phase 5) ✓
//!   [`effect`]   — Shader effects computation       (Phase 6) ✓

pub mod animated;
pub mod stubs;
pub mod gpu_renderer;
pub mod effect;
pub mod model;
pub mod particle;
pub mod pkg;
pub mod render;
pub mod scene;
pub mod script;
pub mod shaders;
pub mod tex;

// New central resource management module: handles textures, shaders, meshes.
pub mod resource;

// Exporting the central resource manager.
pub use resource::ResourceManager;
pub use pkg::Package;
pub use render::ResolvedScene;
pub use scene::Scene;
pub use tex::TexFile;

/**
 * Global access point for the engine's core functionality. 
 * This should be initialized once at application startup.
 */
pub type GlobalResourceManager = Arc<ResourceManager>;


fn main_engine_startup() {
    // Placeholder function demonstrating where the resource manager is instantiated and passed to all modules.
    let _resource_manager = Arc::new(ResourceManager::new());

    println!("Engine initialized with global Resource Manager.");
}
