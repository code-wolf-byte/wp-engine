use anyhow::{Context, Result};
use image::RgbaImage;
use std::path::Path;

use super::pkg::Package;
use super::scene::{Scene, SceneObject};
use super::tex::TexFile;

/// A fully resolved scene ready to composite into a single frame.
pub struct ResolvedScene {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub scene: Scene,
}

pub struct Layer {
    pub name: String,
    pub image: RgbaImage,
    pub origin: [f64; 3],
    pub size: [f64; 3],
    pub parallax_depth: f64,
    pub blend_mode: u32,
}

impl ResolvedScene {
    /// Load a scene wallpaper from a directory.
    ///
    /// Automatically detects whether assets are loose files or packed in a
    /// `.pkg` archive and loads accordingly.
    pub fn from_directory(dir: &Path) -> Result<Self> {
        let scene_json_path = dir.join("scene.json");
        let scene_pkg_path = dir.join("scene.pkg");

        // If assets are packed in scene.pkg, load via the PKG path.
        if scene_pkg_path.exists() {
            let pkg = Package::from_file(&scene_pkg_path)?;

            let scene_json = if scene_json_path.exists() {
                std::fs::read_to_string(&scene_json_path)
                    .with_context(|| format!("reading {}", scene_json_path.display()))?
            } else if let Some(data) = pkg.get("scene.json") {
                String::from_utf8(data.to_vec())
                    .context("scene.json inside PKG is not valid UTF-8")?
            } else {
                anyhow::bail!("no scene.json found in directory or PKG archive");
            };

            return Self::from_package_with_dir(&pkg, &scene_json, dir);
        }

        // Loose-file scene: scene.json + textures alongside it.
        let scene_json = std::fs::read_to_string(&scene_json_path)
            .with_context(|| format!("reading {}", scene_json_path.display()))?;
        let scene = Scene::from_json(&scene_json)?;

        let mut layers = Vec::new();
        for obj in scene.visible_objects() {
            let Some(image_path) = obj.image.as_deref() else { continue };
            if is_special_layer(image_path) || obj.particle.is_some() { continue; }
            match load_texture_from_dir(dir, image_path) {
                Ok(img) => layers.push(layer_from_object(obj, img)),
                Err(e) => eprintln!("warn: skipping layer {:?}: {e}", obj.name),
            }
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        Ok(Self { width, height, layers, scene })
    }

    /// Load from a PKG archive, falling back to the directory for loose files.
    fn from_package_with_dir(pkg: &Package, scene_json: &str, dir: &Path) -> Result<Self> {
        let scene = Scene::from_json(scene_json)?;

        let mut layers = Vec::new();
        for obj in scene.visible_objects() {
            let Some(image_path) = obj.image.as_deref() else { continue };
            if is_special_layer(image_path) || obj.particle.is_some() { continue; }
            let img = load_texture_from_pkg(pkg, image_path)
                .or_else(|pkg_err| {
                    load_texture_from_dir(dir, image_path)
                        .map_err(|dir_err| anyhow::anyhow!("pkg: {pkg_err}; dir: {dir_err}"))
                });
            match img {
                Ok(img) => layers.push(layer_from_object(obj, img)),
                Err(e) => eprintln!("warn: skipping layer {:?}: {e}", obj.name),
            }
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        Ok(Self { width, height, layers, scene })
    }

    /// Load a scene wallpaper from a PKG archive + scene.json string.
    pub fn from_package(pkg: &Package, scene_json: &str) -> Result<Self> {
        let scene = Scene::from_json(scene_json)?;

        let mut layers = Vec::new();
        for obj in scene.visible_objects() {
            let Some(image_path) = obj.image.as_deref() else { continue };
            if is_special_layer(image_path) || obj.particle.is_some() { continue; }
            match load_texture_from_pkg(pkg, image_path) {
                Ok(img) => layers.push(layer_from_object(obj, img)),
                Err(e) => eprintln!("warn: skipping layer {:?}: {e}", obj.name),
            }
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        Ok(Self { width, height, layers, scene })
    }

    /// Composite all layers into a single RGBA image.
    pub fn render(&self) -> RgbaImage {
        let mut canvas = RgbaImage::new(self.width, self.height);

        for layer in &self.layers {
            let resized = if layer.image.width() != self.width || layer.image.height() != self.height {
                image::imageops::resize(
                    &layer.image,
                    self.width,
                    self.height,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                layer.image.clone()
            };

            image::imageops::overlay(&mut canvas, &resized, 0, 0);
        }

        canvas
    }
}

fn layer_from_object(obj: &SceneObject, img: RgbaImage) -> Layer {
    let parallax = match &obj.parallax_depth {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    };

    Layer {
        name: obj.name.clone().unwrap_or_default(),
        image: img,
        origin: obj.parsed_origin(),
        size: obj.parsed_size(),
        parallax_depth: parallax,
        blend_mode: obj.color_blend_mode,
    }
}

fn is_video_path(p: &str) -> bool {
    let p = p.to_lowercase();
    p.ends_with(".mp4") || p.ends_with(".webm") || p.ends_with(".mkv") || p.ends_with(".avi")
}

fn load_texture_from_dir(dir: &Path, image_path: &str) -> Result<RgbaImage> {
    // If it's a .json reference, resolve the model/material chain
    if image_path.ends_with(".json") {
        return resolve_model_chain_dir(dir, image_path);
    }

    let full_path = dir.join(image_path);

    // Video file — extract first frame as static thumbnail
    if is_video_path(image_path) && full_path.exists() {
        return crate::render::ffmpeg::decode_first_frame(&full_path);
    }

    // Try as .tex first
    let tex_path = full_path.with_extension("tex");
    if tex_path.exists() {
        let data = std::fs::read(&tex_path)
            .with_context(|| format!("reading {}", tex_path.display()))?;
        let tex = TexFile::parse(&data)
            .with_context(|| format!("parsing {}", tex_path.display()))?;
        return tex.to_rgba();
    }

    // Try the path as-is (png, jpg, etc.)
    if full_path.exists() {
        return image::open(&full_path)
            .map(|i| i.into_rgba8())
            .with_context(|| format!("loading {}", full_path.display()));
    }

    // Try with .tex extension appended
    let tex_path_str = format!("{}.tex", full_path.display());
    let tex_appended = Path::new(&tex_path_str);
    if tex_appended.exists() {
        let data = std::fs::read(tex_appended)
            .with_context(|| format!("reading {}", tex_appended.display()))?;
        let tex = TexFile::parse(&data)?;
        return tex.to_rgba();
    }

    anyhow::bail!("texture not found: {image_path}")
}

fn load_texture_from_pkg(pkg: &Package, image_path: &str) -> Result<RgbaImage> {
    // If the image path points to a .json model/material, resolve the chain.
    if image_path.ends_with(".json") {
        return resolve_model_chain_pkg(pkg, image_path);
    }

    // Try as .tex
    let tex_name = if image_path.ends_with(".tex") {
        image_path.to_string()
    } else {
        format!("{image_path}.tex")
    };

    if let Some(data) = pkg.get(&tex_name) {
        let tex = TexFile::parse(data)
            .with_context(|| format!("parsing .tex from pkg: {tex_name}"))?;
        return tex.to_rgba();
    }

    // Try original path (png/jpg or video embedded in pkg)
    if let Some(data) = pkg.get(image_path) {
        if is_video_path(image_path) {
            let ext = std::path::Path::new(image_path)
                .extension().and_then(|e| e.to_str()).unwrap_or("mp4");
            let tmp = std::env::temp_dir().join(format!("we_vidtex_{}.{ext}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)));
            std::fs::write(&tmp, data)
                .with_context(|| format!("writing video temp file: {}", tmp.display()))?;
            let result = crate::render::ffmpeg::decode_first_frame(&tmp);
            let _ = std::fs::remove_file(&tmp);
            return result;
        }
        return image::load_from_memory(data)
            .map(|i| i.into_rgba8())
            .with_context(|| format!("loading image from pkg: {image_path}"));
    }

    anyhow::bail!("texture not found in package: {image_path}")
}

/// Follow the model → material → texture reference chain in a PKG archive.
fn resolve_model_chain_pkg(pkg: &Package, json_path: &str) -> Result<RgbaImage> {
    let data = pkg.get(json_path)
        .with_context(|| format!("model/material not found in pkg: {json_path}"))?;
    let val: serde_json::Value = serde_json::from_slice(data)
        .with_context(|| format!("parsing {json_path}"))?;

    // Model file: { "material": "materials/X.json" }
    if let Some(mat_path) = val.get("material").and_then(|v| v.as_str()) {
        return resolve_model_chain_pkg(pkg, mat_path);
    }

    // Material file: { "passes": [{ "textures": ["name"] }] }
    if let Some(passes) = val.get("passes").and_then(|v| v.as_array()) {
        for pass in passes {
            if let Some(textures) = pass.get("textures").and_then(|v| v.as_array()) {
                for tex_ref in textures {
                    if let Some(tex_name) = tex_ref.as_str() {
                        let tex_path = format!("materials/{tex_name}.tex");
                        if let Some(tex_data) = pkg.get(&tex_path) {
                            let tex = TexFile::parse(tex_data)
                                .with_context(|| format!("parsing {tex_path}"))?;
                            return tex.to_rgba();
                        }
                        // Try without materials/ prefix
                        let alt_path = format!("{tex_name}.tex");
                        if let Some(tex_data) = pkg.get(&alt_path) {
                            let tex = TexFile::parse(tex_data)?;
                            return tex.to_rgba();
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("could not resolve texture from {json_path}")
}

/// Follow the model → material → texture chain for loose files on disk.
fn resolve_model_chain_dir(dir: &Path, json_path: &str) -> Result<RgbaImage> {
    let full = dir.join(json_path);
    let data = std::fs::read_to_string(&full)
        .with_context(|| format!("reading {}", full.display()))?;
    let val: serde_json::Value = serde_json::from_str(&data)
        .with_context(|| format!("parsing {}", full.display()))?;

    if let Some(mat_path) = val.get("material").and_then(|v| v.as_str()) {
        return resolve_model_chain_dir(dir, mat_path);
    }

    if let Some(passes) = val.get("passes").and_then(|v| v.as_array()) {
        for pass in passes {
            if let Some(textures) = pass.get("textures").and_then(|v| v.as_array()) {
                for tex_ref in textures {
                    if let Some(tex_name) = tex_ref.as_str() {
                        let tex_path = dir.join(format!("materials/{tex_name}.tex"));
                        if tex_path.exists() {
                            let tex_data = std::fs::read(&tex_path)?;
                            let tex = TexFile::parse(&tex_data)?;
                            return tex.to_rgba();
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("could not resolve texture from {json_path}")
}

fn guess_scene_dimensions(scene: &Scene, layers: &[Layer]) -> (u32, u32) {
    // 1. Try orthogonal projection dimensions from camera or general settings
    if let Some(cam) = &scene.camera {
        if let Some(proj) = &cam.orthogonal_projection {
            if let (Some(w), Some(h)) = (proj.width, proj.height) {
                if w > 0 && h > 0 {
                    return (w, h);
                }
            }
        }
    }
    if let Some(gen) = &scene.general {
        if let Some(proj) = &gen.orthogonal_projection {
            if let (Some(w), Some(h)) = (proj.width, proj.height) {
                if w > 0 && h > 0 {
                    return (w, h);
                }
            }
        }
    }

    // 2. Use the largest layer dimensions
    let mut max_w = 0u32;
    let mut max_h = 0u32;
    for layer in layers {
        max_w = max_w.max(layer.image.width());
        max_h = max_h.max(layer.image.height());
    }
    if max_w > 0 && max_h > 0 {
        return (max_w, max_h);
    }

    // 3. Fallback
    (1920, 1080)
}

fn is_special_layer(image_path: &str) -> bool {
    let p = image_path.to_lowercase();
    p.contains("projectlayer")
        || p.contains("composelayer")
        || p.contains("fullscreenlayer")
        || p.contains("solidlayer")
        || p.contains("solid_instance_model")
}
