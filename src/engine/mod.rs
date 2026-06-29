//! Wallpaper Engine scene rendering engine.
//!
//! Phases:
//!   [`pkg`]      — PKG archive extractor        (Phase 1) ✓
//!   [`tex`]      — .tex texture decoder          (Phase 2) ✓
//!   [`scene`]    — scene.json parser             (Phase 3) ✓
//!   [`render`]   — image-layer compositor        (Phase 4) ✓
//!   [`particle`] — particle system               (Phase 5) ✓
//!   [`effect`]   — CPU-side shader effects       (Phase 6) ✓
//!   [`animated`] — real-time animated renderer   (Phase 7) ✓

pub mod animated;
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

pub use pkg::Package;
pub use render::ResolvedScene;
pub use scene::Scene;
pub use tex::TexFile;
