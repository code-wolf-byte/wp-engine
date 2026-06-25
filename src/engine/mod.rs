//! Wallpaper Engine scene rendering engine.
//!
//! Phases:
//!   [`pkg`]    — PKG archive extractor      (Phase 1) ✓
//!   [`tex`]    — .tex texture decoder        (Phase 2) ✓
//!   [`scene`]  — scene.json parser           (Phase 3) ✓
//!   [`render`] — image-layer compositor      (Phase 4) ✓

pub mod pkg;
pub mod render;
pub mod scene;
pub mod tex;

pub use pkg::Package;
pub use render::ResolvedScene;
pub use scene::Scene;
pub use tex::TexFile;
