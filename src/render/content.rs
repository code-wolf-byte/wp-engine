use anyhow::{anyhow, Result};
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::workshop::{Wallpaper, WallpaperType};

/// The resolved content of a wallpaper, ready to hand to the renderer.
///
/// This is the primary extension point as new wallpaper types are supported:
/// add a new variant here, implement it in `from_wallpaper`/`from_path`, and
/// add a corresponding `FrameSource` variant in `frame.rs`.
pub enum WallpaperContent {
    /// A pre-loaded static RGBA image (PNG, JPEG, …).
    Static(Arc<RgbaImage>),
    /// A video file to be decoded frame-by-frame with FFmpeg.
    Video { path: PathBuf },
    /// A scene wallpaper — rendered by compositing decoded .tex layers.
    Scene { dir: PathBuf },
    // Future variants (not yet implemented):
    // Web   { html: PathBuf },
    // Application { exe: PathBuf },
}

impl WallpaperContent {
    /// Parse a Workshop `Wallpaper` into the appropriate content variant.
    ///
    /// Returns `Err` for types that are not yet renderable (Scene, Web,
    /// Application) so callers get a clear diagnostic instead of a generic
    /// image-loading failure.
    pub fn from_wallpaper(w: &Wallpaper) -> Result<Self> {
        match w.wallpaper_type() {
            WallpaperType::Scene => {
                let dir = w.path.clone();
                if dir.join("scene.json").exists() || dir.join("scene.pkg").exists() {
                    Ok(WallpaperContent::Scene { dir })
                } else {
                    Err(anyhow!(
                        "scene wallpaper missing scene.json and scene.pkg in {}",
                        dir.display()
                    ))
                }
            }
            WallpaperType::Web => Err(anyhow!("web wallpapers are not yet supported")),
            WallpaperType::Application => Err(anyhow!("application wallpapers are Windows-only")),
            WallpaperType::Video | WallpaperType::Unknown => {
                let path = w
                    .wallpaper_file()
                    .ok_or_else(|| anyhow!("wallpaper has no file field in project.json"))?;
                if !path.exists() {
                    return Err(anyhow!("wallpaper file not found: {}", path.display()));
                }
                Self::from_path(&path)
            }
        }
    }

    /// Resolve content from any filesystem path: a wallpaper directory
    /// (scene.json / scene.pkg / project.json) or a plain image/video file.
    #[tracing::instrument(target = "content", level = "debug", fields(path = %path.display()))]
    pub fn from_any_path(path: &Path) -> Result<Self> {
        if path.is_dir() {
            if path.join("scene.json").exists() || path.join("scene.pkg").exists() {
                tracing::debug!(target: "content", "resolved as scene wallpaper");
                return Ok(WallpaperContent::Scene {
                    dir: path.to_owned(),
                });
            }
            // A wallpaper directory with project.json but no scene data:
            // resolve the `file` entry (video/image wallpapers).
            let project_path = path.join("project.json");
            if project_path.exists() {
                let data = std::fs::read_to_string(&project_path)?;
                let project: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(file) = project.get("file").and_then(|f| f.as_str()) {
                    return Self::from_path(&path.join(file));
                }
            }
            return Err(anyhow!(
                "directory {} contains no scene.json, scene.pkg, or project.json file entry",
                path.display()
            ));
        }
        Self::from_path(path)
    }

    /// Detect content type from a raw file path, guessing by extension.
    ///
    /// Video extensions are decoded with FFmpeg; everything else is opened
    /// by the `image` crate as a static image.
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "flv" | "wmv" => {
                tracing::debug!(target: "content", %ext, "resolved as video wallpaper");
                Ok(WallpaperContent::Video {
                    path: path.to_owned(),
                })
            }
            _ => {
                tracing::debug!(target: "content", %ext, "resolved as static image wallpaper");
                let img = image::open(path)
                    .map_err(|e| anyhow!("failed to load image {}: {}", path.display(), e))?
                    .into_rgba8();
                Ok(WallpaperContent::Static(Arc::new(img)))
            }
        }
    }
}
