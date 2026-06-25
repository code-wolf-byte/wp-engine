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
}

pub struct Layer {
    pub name: String,
    pub image: RgbaImage,
    pub origin: [f64; 3],
    pub size: [f64; 3],
    pub parallax_depth: f64,
}

impl ResolvedScene {
    /// Load a scene wallpaper from a directory containing scene.json and assets.
    pub fn from_directory(dir: &Path) -> Result<Self> {
        let scene_path = dir.join("scene.json");
        let scene_json = std::fs::read_to_string(&scene_path)
            .with_context(|| format!("reading {}", scene_path.display()))?;
        let scene = Scene::from_json(&scene_json)?;

        let mut layers = Vec::new();

        for obj in scene.visible_objects() {
            let Some(image_path) = obj.image.as_deref() else {
                continue;
            };

            let img = load_texture_from_dir(dir, image_path)?;

            layers.push(layer_from_object(obj, img));
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);

        Ok(Self { width, height, layers })
    }

    /// Load a scene wallpaper from a PKG archive + scene.json.
    pub fn from_package(pkg: &Package, scene_json: &str) -> Result<Self> {
        let scene = Scene::from_json(scene_json)?;

        let mut layers = Vec::new();

        for obj in scene.visible_objects() {
            let Some(image_path) = obj.image.as_deref() else {
                continue;
            };

            let img = load_texture_from_pkg(pkg, image_path)?;

            layers.push(layer_from_object(obj, img));
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);

        Ok(Self { width, height, layers })
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
    }
}

fn load_texture_from_dir(dir: &Path, image_path: &str) -> Result<RgbaImage> {
    let full_path = dir.join(image_path);

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

    // Try original path (might be a png/jpg embedded in pkg)
    if let Some(data) = pkg.get(image_path) {
        return image::load_from_memory(data)
            .map(|i| i.into_rgba8())
            .with_context(|| format!("loading image from pkg: {image_path}"));
    }

    anyhow::bail!("texture not found in package: {image_path}")
}

fn guess_scene_dimensions(scene: &Scene, layers: &[Layer]) -> (u32, u32) {
    // Use the camera eye/center if available, otherwise use the largest layer
    if let Some(cam) = &scene.camera {
        if let Some(eye) = &cam.eye {
            if eye.len() >= 2 && eye[0] > 0.0 && eye[1] > 0.0 {
                return (eye[0] as u32, eye[1] as u32);
            }
        }
    }

    let mut max_w = 1920u32;
    let mut max_h = 1080u32;
    for layer in layers {
        max_w = max_w.max(layer.image.width());
        max_h = max_h.max(layer.image.height());
    }
    (max_w, max_h)
}
