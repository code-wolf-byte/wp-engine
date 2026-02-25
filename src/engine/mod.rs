//! Wallpaper Engine scene rendering engine.
//!
//! Phases:
//!   [`pkg`]  — PKG archive extractor      (Phase 1) ✓
//!   `tex`    — .tex texture decoder        (Phase 2, TODO)
//!   `scene`  — scene.json parser           (Phase 3, TODO)
//!   `render` — wgpu image-layer renderer   (Phase 4, TODO)

pub mod pkg;
pub use pkg::Package;
