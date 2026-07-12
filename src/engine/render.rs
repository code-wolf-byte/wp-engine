use anyhow::{Context, Result};
use image::RgbaImage;
use std::path::Path;

use super::particle;
use super::pkg::Package;
use super::properties::SceneProperties;
use super::scene::{Scene, SceneObject};
use super::tex::TexFile;

/// Parse scene.json, first resolving `{"user": ...}` references against the
/// wallpaper's project.json properties (plus any `--set-property` overrides).
fn parse_scene_with_properties(scene_json: &str, project_dir: Option<&Path>) -> Result<Scene> {
    let mut value: serde_json::Value = serde_json::from_str(scene_json).with_context(|| {
        let preview = if scene_json.len() > 200 {
            &scene_json[..200]
        } else {
            scene_json
        };
        format!("failed to parse scene.json: {preview}...")
    })?;
    if let Some(dir) = project_dir {
        let props = SceneProperties::from_project_dir(dir);
        props.resolve_scene_json(&mut value);
    }
    Scene::from_value(value)
}

/// A fully resolved scene ready to composite into a single frame.
pub struct ResolvedScene {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub particle_layers: Vec<ParticleLayer>,
    pub scene: Scene,
}

/// A scene object whose `particle` field references a particle preset JSON
/// (e.g. `"particles/presets/fog1.json"`). Holds enough static data for any
/// renderer to build its own `particle::ParticleSystem` (each render path
/// owns independent simulation state, so this is config, not a live system).
pub struct ParticleLayer {
    pub name: String,
    /// WE origin: absolute scene coordinates, Y-up, (0,0) at bottom-left —
    /// same convention as `Layer::origin`; callers convert to pixel space at
    /// render time (`px = origin.x`, `py = height - origin.y`).
    pub origin: [f64; 3],
    pub parallax_depth: [f64; 2],
    pub config: particle::ParticleConfig,
    pub overrides: Option<particle::InstanceOverride>,
    /// Resolved once from `config.material` (if present) via the same
    /// model/material→texture chain image layers use, keeping every
    /// sprite-sheet frame. `None` falls back to `render_onto`'s flat-color
    /// circle draw.
    pub sprite_texture: Option<particle::ParticleSprite>,
    /// True when the material's own pass declares `"blending":"additive"`
    /// (the overwhelming majority of real particle materials — fog, smoke,
    /// embers, rain, lightning). Composited additively onto the scene
    /// instead of the default alpha-over, which otherwise makes a sprite's
    /// near-black background visibly darken/box the scene behind it.
    pub additive_blend: bool,
    /// This object's raw index in `scene.objects` (already the scene's
    /// topological/declaration render order) — lets render loops interleave
    /// particle systems with image layers in true scene z-order, and lets
    /// effect instances (which record the same raw object index) find their
    /// owning layer regardless of how many particles/skipped/invisible
    /// objects precede it.
    pub order_index: usize,
}

pub struct Layer {
    pub name: String,
    pub image: RgbaImage,
    /// Additional animation frames (frame 0 is `image`). Empty = not animated.
    pub extra_frames: Vec<RgbaImage>,
    /// Duration of each frame in milliseconds. 0 = not animated / unknown.
    pub frame_duration_ms: u32,
    pub origin: [f64; 3],
    pub size: [f64; 3],
    pub scale: [f64; 3], // WE scale multiplier (default 1.0), separate from size
    pub parallax_depth: [f64; 2],
    /// z-rotation in radians (WE `angles.z`; the JSON value is already in radians).
    pub angle: f32,
    pub blend_mode: u32,
    pub alpha: f32,
    pub color: [f32; 3],
    pub brightness: f32,
    /// True for WE "copybackground" layers — source is the rendered canvas so far.
    pub copybackground: bool,
    /// `.tex` `NoInterpolation` flag: sample nearest instead of linear.
    pub no_interpolation: bool,
    /// `.tex` `ClampUVs`/`ClampUVsBorder` flag: clamp-to-edge instead of repeat.
    pub clamp_uvs: bool,
    /// Animated puppet runtime (mesh + skeleton + MDLA animations) — the
    /// live GPU path re-poses and re-rasterizes `image` from this.
    pub puppet: Option<std::sync::Arc<crate::engine::puppet::PuppetRuntime>>,
    /// This object's raw index in `scene.objects` — see
    /// `ParticleLayer::order_index`.
    pub order_index: usize,
}

/// A loaded texture plus any additional animation frames (e.g. from a `.tex`
/// TEXS spritesheet table or a multi-image container).
struct LoadedImage {
    image: RgbaImage,
    extra_frames: Vec<RgbaImage>,
    frame_duration_ms: u32,
    no_interpolation: bool,
    clamp_uvs: bool,
    /// Present when the model is an animated puppet (mesh + skeleton +
    /// MDLA animations): `image` holds the rest pose, and the live GPU
    /// path re-poses/re-rasterizes from this over time.
    puppet: Option<std::sync::Arc<crate::engine::puppet::PuppetRuntime>>,
}

impl LoadedImage {
    fn single(image: RgbaImage) -> Self {
        Self {
            image,
            extra_frames: Vec::new(),
            frame_duration_ms: 0,
            no_interpolation: false,
            clamp_uvs: false,
            puppet: None,
        }
    }

    /// Build from a parsed `.tex`, pulling out extra animation frames if any
    /// and carrying over its `NoInterpolation`/`ClampUVs` sampler flags.
    fn from_tex(tex: &TexFile) -> Result<Self> {
        let no_interpolation = tex.no_interpolation();
        let clamp_uvs = tex.clamp_uvs();
        if tex.is_animated() {
            if let Ok(mut frames) = tex.to_rgba_frames() {
                if frames.len() > 1 {
                    let image = frames.remove(0);
                    let frame_duration_ms = tex
                        .frames()
                        .first()
                        .map(|f| ((f.frametime * 1000.0) as u32).max(1))
                        .unwrap_or(100);
                    return Ok(Self {
                        image,
                        extra_frames: frames,
                        frame_duration_ms,
                        no_interpolation,
                        clamp_uvs,
                        puppet: None,
                    });
                }
            }
        }
        Ok(Self {
            no_interpolation,
            clamp_uvs,
            ..Self::single(tex_to_rgba(tex)?)
        })
    }
}

impl ResolvedScene {
    /// Load a scene wallpaper from a directory.
    ///
    /// Automatically detects whether assets are loose files or packed in a
    /// `.pkg` archive and loads accordingly. The scene file is usually
    /// `scene.json`/`scene.pkg`, but project.json's `file` field can name a
    /// different one — GIF-converted wallpapers ship
    /// `gifscene.json`/`gifscene.pkg` (e.g. workshop item 2036522973).
    pub fn from_directory(dir: &Path) -> Result<Self> {
        let scene_name = std::fs::read_to_string(dir.join("project.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|p| p.get("file")?.as_str().map(str::to_string))
            .filter(|f| f.ends_with(".json"))
            .unwrap_or_else(|| "scene.json".to_string());
        let pkg_name = format!("{}.pkg", scene_name.trim_end_matches(".json"));

        let scene_json_path = dir.join(&scene_name);
        let scene_pkg_path = dir.join(&pkg_name);

        if scene_pkg_path.exists() {
            let pkg = Package::from_file(&scene_pkg_path)?;

            let scene_json = if scene_json_path.exists() {
                std::fs::read_to_string(&scene_json_path)
                    .with_context(|| format!("reading {}", scene_json_path.display()))?
            } else if let Some(data) = pkg.get(&scene_name).or_else(|| pkg.get("scene.json")) {
                String::from_utf8(data.to_vec())
                    .context("scene json inside PKG is not valid UTF-8")?
            } else {
                anyhow::bail!("no {scene_name} found in directory or PKG archive");
            };

            return Self::from_package_with_dir(&pkg, &scene_json, dir);
        }

        let scene_json = std::fs::read_to_string(&scene_json_path)
            .with_context(|| format!("reading {}", scene_json_path.display()))?;
        let scene = parse_scene_with_properties(&scene_json, Some(dir))?;

        let mut layers = Vec::new();
        let mut solid_indices = Vec::new();
        let mut particle_layers = Vec::new();
        // Raw `scene.objects` index (not a visible-only count): effect
        // instances record the same raw index, so layers and effects agree
        // on object identity no matter what gets skipped in between.
        for (obj_index, obj) in scene.objects.iter().enumerate() {
            if !obj.is_visible() {
                continue;
            }
            if obj.particle.is_some() {
                if let Some(mut pl) = particle_layer_from_object(obj, Some(dir), None) {
                    pl.order_index = obj_index;
                    particle_layers.push(pl);
                }
                continue;
            }
            if obj.text.is_some() {
                if let Some(mut layer) = text_layer_from_object(obj, Some(dir), None) {
                    layer.order_index = obj_index;
                    layers.push(layer);
                }
                continue;
            }
            // Note: `copybackground` objects still carry a real image/model —
            // the reference engine ignores the flag and draws them normally.
            let loaded = match obj.image.as_deref() {
                None => {
                    if obj.effects.is_empty() {
                        continue;
                    }
                    LoadedImage::single(placeholder_for(obj))
                }
                Some(path) if is_solid_layer_path(path) => {
                    solid_indices.push(layers.len());
                    LoadedImage::single(white_pixel())
                }
                Some(path) => {
                    if is_special_layer(path) {
                        continue;
                    }
                    match load_texture_from_dir(dir, path) {
                        Ok(loaded) => loaded,
                        Err(e) => {
                            eprintln!("[scene] texture load failed for '{path}': {e:#}");
                            if obj.effects.is_empty() {
                                continue;
                            }
                            LoadedImage::single(placeholder_for(obj))
                        }
                    }
                }
            };
            let mut layer = layer_from_object(obj, loaded, None);
            layer.order_index = obj_index;
            layers.push(layer);
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        fill_solid_layer_sizes(&mut layers, &solid_indices, width, height);
        Ok(Self {
            width,
            height,
            layers,
            particle_layers,
            scene,
        })
    }

    /// Load from a PKG archive, falling back to the directory for loose files.
    fn from_package_with_dir(pkg: &Package, scene_json: &str, dir: &Path) -> Result<Self> {
        let scene = parse_scene_with_properties(scene_json, Some(dir))?;

        let mut layers = Vec::new();
        let mut solid_indices = Vec::new();
        let mut particle_layers = Vec::new();
        // Raw `scene.objects` index (not a visible-only count): effect
        // instances record the same raw index, so layers and effects agree
        // on object identity no matter what gets skipped in between.
        for (obj_index, obj) in scene.objects.iter().enumerate() {
            if !obj.is_visible() {
                continue;
            }
            if obj.particle.is_some() {
                if let Some(mut pl) = particle_layer_from_object(obj, Some(dir), Some(pkg)) {
                    pl.order_index = obj_index;
                    particle_layers.push(pl);
                }
                continue;
            }
            if obj.text.is_some() {
                if let Some(mut layer) = text_layer_from_object(obj, Some(dir), Some(pkg)) {
                    layer.order_index = obj_index;
                    layers.push(layer);
                }
                continue;
            }
            let loaded = match obj.image.as_deref() {
                None => {
                    if obj.effects.is_empty() {
                        continue;
                    }
                    LoadedImage::single(placeholder_for(obj))
                }
                Some(path) if is_solid_layer_path(path) => {
                    solid_indices.push(layers.len());
                    LoadedImage::single(white_pixel())
                }
                Some(path) => {
                    if is_special_layer(path) {
                        continue;
                    }
                    match load_texture_from_pkg(pkg, path).or_else(|pkg_err| {
                        load_texture_from_dir(dir, path)
                            .map_err(|dir_err| anyhow::anyhow!("pkg: {pkg_err}; dir: {dir_err}"))
                    }) {
                        Ok(loaded) => loaded,
                        Err(e) => {
                            eprintln!("[scene] texture load failed for '{path}': {e:#}");
                            if obj.effects.is_empty() {
                                continue;
                            }
                            LoadedImage::single(placeholder_for(obj))
                        }
                    }
                }
            };
            let mut layer = layer_from_object(obj, loaded, None);
            layer.order_index = obj_index;
            layers.push(layer);
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        fill_solid_layer_sizes(&mut layers, &solid_indices, width, height);
        Ok(Self {
            width,
            height,
            layers,
            particle_layers,
            scene,
        })
    }

    /// Load a scene wallpaper from a PKG archive + scene.json string.
    /// No project.json is available here, so user properties keep defaults.
    pub fn from_package(pkg: &Package, scene_json: &str) -> Result<Self> {
        let scene = parse_scene_with_properties(scene_json, None)?;

        let mut layers = Vec::new();
        let mut solid_indices = Vec::new();
        let mut particle_layers = Vec::new();
        // Raw `scene.objects` index (not a visible-only count): effect
        // instances record the same raw index, so layers and effects agree
        // on object identity no matter what gets skipped in between.
        for (obj_index, obj) in scene.objects.iter().enumerate() {
            if !obj.is_visible() {
                continue;
            }
            if obj.particle.is_some() {
                if let Some(mut pl) = particle_layer_from_object(obj, None, Some(pkg)) {
                    pl.order_index = obj_index;
                    particle_layers.push(pl);
                }
                continue;
            }
            if obj.text.is_some() {
                if let Some(mut layer) = text_layer_from_object(obj, None, Some(pkg)) {
                    layer.order_index = obj_index;
                    layers.push(layer);
                }
                continue;
            }
            let loaded = match obj.image.as_deref() {
                None => {
                    if obj.effects.is_empty() {
                        continue;
                    }
                    LoadedImage::single(placeholder_for(obj))
                }
                Some(path) if is_solid_layer_path(path) => {
                    solid_indices.push(layers.len());
                    LoadedImage::single(white_pixel())
                }
                Some(path) => {
                    if is_special_layer(path) {
                        continue;
                    }
                    match load_texture_from_pkg(pkg, path) {
                        Ok(loaded) => loaded,
                        Err(_) => {
                            if obj.effects.is_empty() {
                                continue;
                            }
                            LoadedImage::single(placeholder_for(obj))
                        }
                    }
                }
            };
            let mut layer = layer_from_object(obj, loaded, None);
            layer.order_index = obj_index;
            layers.push(layer);
        }

        let (width, height) = guess_scene_dimensions(&scene, &layers);
        fill_solid_layer_sizes(&mut layers, &solid_indices, width, height);
        Ok(Self {
            width,
            height,
            layers,
            particle_layers,
            scene,
        })
    }

    /// Composite all layers into a single RGBA image, in true scene z-order
    /// (image and particle layers interleaved by `order_index`, matching the
    /// reference's single shared per-object render order — CScene.cpp's
    /// `m_objectsByRenderOrder` — instead of drawing all particles after all
    /// images).
    pub fn render(&self) -> RgbaImage {
        let mut canvas = RgbaImage::new(self.width, self.height);

        enum DrawItem {
            Image(usize),
            Particle(usize),
        }
        let mut items: Vec<(usize, DrawItem)> = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (l.order_index, DrawItem::Image(i)))
            .chain(
                self.particle_layers
                    .iter()
                    .enumerate()
                    .map(|(i, pl)| (pl.order_index, DrawItem::Particle(i))),
            )
            .collect();
        items.sort_by_key(|(order, _)| *order);

        for (_, item) in items {
            match item {
                DrawItem::Image(i) => self.draw_image_layer(&self.layers[i], &mut canvas),
                // This is a single-shot preview render, so seed the
                // simulation forward a few seconds instead of drawing an
                // empty, freshly-spawned system.
                DrawItem::Particle(i) => {
                    let pl = &self.particle_layers[i];
                    let spawn_center =
                        [pl.origin[0] as f32, self.height as f32 - pl.origin[1] as f32];
                    let mut system = particle::ParticleSystem::from_config(
                        &pl.config,
                        spawn_center,
                        pl.overrides.as_ref(),
                    );
                    if let Some(sprite) = &pl.sprite_texture {
                        system.set_sprite_frames(sprite.frames.len(), sprite.duration);
                    }
                    for _ in 0..150 {
                        system.step(1.0 / 30.0);
                    }
                    // The canvas here *is* the opaque scene, so additive
                    // accumulation directly implements the reference's
                    // GL_SRC_ALPHA/GL_ONE quad blending against it.
                    system.render_onto_blended(
                        &mut canvas,
                        pl.sprite_texture.as_ref(),
                        [0.0, 0.0],
                        pl.additive_blend,
                    );
                }
            }
        }

        canvas
    }

    fn draw_image_layer(&self, layer: &Layer, canvas: &mut RgbaImage) {
        let draw_w = if layer.size[0] != 0.0 {
            (layer.size[0] * layer.scale[0]) as u32
        } else {
            layer.image.width()
        };
        let draw_h = if layer.size[1] != 0.0 {
            (layer.size[1] * layer.scale[1]) as u32
        } else {
            layer.image.height()
        };
        if draw_w == 0 || draw_h == 0 {
            return;
        }

        let resized = if layer.image.width() != draw_w || layer.image.height() != draw_h {
            image::imageops::resize(
                &layer.image,
                draw_w,
                draw_h,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            layer.image.clone()
        };

        // WE origin is the object's center in absolute scene coordinates
        // (Y-up, (0,0) at bottom-left); convert to top-left pixel coords.
        let px = (layer.origin[0] - draw_w as f64 / 2.0) as i64;
        let py = (self.height as f64 - layer.origin[1] - draw_h as f64 / 2.0) as i64;

        let cw = canvas.width() as i64;
        let ch = canvas.height() as i64;

        let mut blend_pixel = |cx: i64, cy: i64, s: image::Rgba<u8>| {
            if cx < 0 || cx >= cw || cy < 0 || cy >= ch {
                return;
            }
            let src_rgb = [
                (s[0] as f32 / 255.0) * layer.color[0] * layer.brightness,
                (s[1] as f32 / 255.0) * layer.color[1] * layer.brightness,
                (s[2] as f32 / 255.0) * layer.color[2] * layer.brightness,
            ];
            let src_a = (s[3] as f32 / 255.0) * layer.alpha;
            if src_a <= 0.0 {
                return;
            }
            let dst = canvas.get_pixel_mut(cx as u32, cy as u32);
            let dest_rgb = [
                dst[0] as f32 / 255.0,
                dst[1] as f32 / 255.0,
                dst[2] as f32 / 255.0,
            ];
            let out = crate::engine::blend::apply_blending(
                layer.blend_mode,
                dest_rgb,
                src_rgb,
                src_a,
            );
            dst[0] = (out[0].clamp(0.0, 1.0) * 255.0) as u8;
            dst[1] = (out[1].clamp(0.0, 1.0) * 255.0) as u8;
            dst[2] = (out[2].clamp(0.0, 1.0) * 255.0) as u8;
            let dst_a = dst[3] as f32 / 255.0;
            dst[3] = ((dst_a + src_a * (1.0 - dst_a)).clamp(0.0, 1.0) * 255.0) as u8;
        };

        if layer.angle == 0.0 {
            for y in 0..draw_h {
                for x in 0..draw_w {
                    blend_pixel(px + x as i64, py + y as i64, *resized.get_pixel(x, y));
                }
            }
        } else {
            // Rotated layer: walk the canvas-space bounding box of the
            // rotated rect and inverse-sample back into the unrotated
            // `resized` image (nearest-neighbor).
            let ccx = px as f64 + draw_w as f64 / 2.0;
            let ccy = py as f64 + draw_h as f64 / 2.0;
            let half_diag =
                ((draw_w as f64 / 2.0).powi(2) + (draw_h as f64 / 2.0).powi(2)).sqrt();
            let (sin_a, cos_a) = (layer.angle as f64).sin_cos();
            let x0 = ((ccx - half_diag).floor() as i64).max(0);
            let x1 = ((ccx + half_diag).ceil() as i64).min(cw);
            let y0 = ((ccy - half_diag).floor() as i64).max(0);
            let y1 = ((ccy + half_diag).ceil() as i64).min(ch);
            for cy in y0..y1 {
                for cx in x0..x1 {
                    let fx = cx as f64 + 0.5 - ccx;
                    let fy = cy as f64 + 0.5 - ccy;
                    // Inverse rotation (screen → unrotated local space).
                    let lx = cos_a * fx + sin_a * fy;
                    let ly = -sin_a * fx + cos_a * fy;
                    let sx = (lx + draw_w as f64 / 2.0).floor();
                    let sy = (ly + draw_h as f64 / 2.0).floor();
                    if sx < 0.0 || sy < 0.0 || sx >= draw_w as f64 || sy >= draw_h as f64 {
                        continue;
                    }
                    blend_pixel(cx, cy, *resized.get_pixel(sx as u32, sy as u32));
                }
            }
        }
    }
}

/// WE `parallaxDepth` is always a vec2 (independent x/y depth), but a bare
/// number in scene.json means "same depth on both axes".
fn parse_parallax_depth(v: &serde_json::Value) -> [f64; 2] {
    use crate::engine::model::DynamicValue;
    match crate::engine::model::json_to_animated(v).value {
        DynamicValue::Float(f) => [f as f64; 2],
        DynamicValue::Vec3([x, y, _]) => [x as f64, y as f64],
        _ => [0.0, 0.0],
    }
}

/// WE origin convention: Y-up, (0,0) at bottom-left (same as `Layer::origin`).
/// "top"/"bottom" shift the object vertically by half its scaled height so
/// the origin becomes that edge instead of the center; "left"/"right" do the
/// same horizontally. Matches CImage.cpp lines 242-256 exactly (each pair is
/// mutually exclusive — "top bottom" together, like the reference, only
/// honors "top").
fn alignment_offset(alignment: Option<&str>, scaled_size: [f64; 2]) -> [f64; 2] {
    let a = alignment.unwrap_or("").to_lowercase();
    let mut offset = [0.0, 0.0];
    if a.contains("top") {
        offset[1] -= scaled_size[1] / 2.0;
    } else if a.contains("bottom") {
        offset[1] += scaled_size[1] / 2.0;
    }
    if a.contains("left") {
        offset[0] += scaled_size[0] / 2.0;
    } else if a.contains("right") {
        offset[0] -= scaled_size[0] / 2.0;
    }
    offset
}

/// `alignment_override`, when given, replaces `obj.alignment` (text objects
/// combine separate `horizontalalign`/`verticalalign` fields into one string
/// before calling this, rather than reading `obj.alignment` directly).
fn layer_from_object(
    obj: &SceneObject,
    loaded: LoadedImage,
    alignment_override: Option<&str>,
) -> Layer {
    let parallax = obj
        .parallax_depth
        .as_ref()
        .map(parse_parallax_depth)
        .unwrap_or([0.0, 0.0]);

    // WE stores angles already in radians; only the z component (2D roll)
    // applies to a flat layer quad. The reference negates it (rotate(-angle,
    // ...)) when building the object's screen transform.
    let angle = obj
        .angles
        .as_ref()
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.as_vec3())
        .map(|v| -v[2])
        .unwrap_or(0.0);

    let scale = match &obj.scale {
        Some(v) => {
            let s = crate::engine::scene::parse_value_vec3(v).unwrap_or([1.0, 1.0, 1.0]);
            s
        }
        None => [1.0, 1.0, 1.0],
    };

    let alpha = obj
        .alpha
        .as_ref()
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.as_float())
        .unwrap_or(1.0);
    let color = obj
        .color
        .as_ref()
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.as_vec3())
        .unwrap_or([1.0, 1.0, 1.0]);
    let brightness = obj
        .brightness
        .as_ref()
        .map(crate::engine::model::json_to_animated)
        .and_then(|v| v.as_float())
        .unwrap_or(1.0);

    // Alignment shifts the object so its origin becomes an edge rather than
    // its center (CImage.cpp lines 242-256), expressed here as an offset
    // folded directly into `origin` — every consumer of `Layer::origin`
    // (both render paths) then gets alignment for free.
    let raw_size = obj.parsed_size();
    let effective_size = if raw_size[0] > 0.0 && raw_size[1] > 0.0 {
        [raw_size[0], raw_size[1]]
    } else {
        [loaded.image.width() as f64, loaded.image.height() as f64]
    };
    let scaled_size = [effective_size[0] * scale[0], effective_size[1] * scale[1]];
    let alignment = alignment_override.or(obj.alignment.as_deref());
    let align_offset = alignment_offset(alignment, scaled_size);
    let origin = {
        let o = obj.parsed_origin();
        [o[0] + align_offset[0], o[1] + align_offset[1], o[2]]
    };

    Layer {
        name: obj.name.clone().unwrap_or_default(),
        image: loaded.image,
        extra_frames: loaded.extra_frames,
        frame_duration_ms: loaded.frame_duration_ms,
        origin,
        size: obj.parsed_size(),
        scale,
        parallax_depth: parallax,
        angle,
        blend_mode: obj.color_blend_mode,
        alpha,
        color,
        brightness,
        copybackground: obj.copybackground,
        no_interpolation: loaded.no_interpolation,
        clamp_uvs: loaded.clamp_uvs,
        puppet: loaded.puppet,
        order_index: 0,
    }
}

fn is_video_path(p: &str) -> bool {
    let p = p.to_lowercase();
    p.ends_with(".mp4") || p.ends_with(".webm") || p.ends_with(".mkv") || p.ends_with(".avi")
}

/// Resolve a particle preset's JSON text, checking the wallpaper's own
/// directory, then its `scene.pkg` (if given), then the global Steam assets
/// dir — mirroring the same wallpaper-first priority used for shaders/effects
/// (`shaders::resolver::AssetResolver`).
fn read_particle_json(dir: Option<&Path>, pkg: Option<&Package>, rel_path: &str) -> Option<String> {
    if let Some(dir) = dir {
        if let Ok(s) = std::fs::read_to_string(dir.join(rel_path)) {
            return Some(s);
        }
    }
    if let Some(pkg) = pkg {
        if let Some(data) = pkg.get(rel_path) {
            if let Ok(s) = String::from_utf8(data.to_vec()) {
                return Some(s);
            }
        }
    }
    if let Some(assets_dir) = super::shaders::loader::find_we_assets_dir() {
        if let Ok(s) = std::fs::read_to_string(assets_dir.join(rel_path)) {
            return Some(s);
        }
    }
    None
}

/// Build a `ParticleLayer` from a scene object whose `particle` field is a
/// preset path, resolving and parsing the referenced JSON. Returns `None`
/// (silently skipped, like a missing texture) if the preset can't be found
/// or parsed.
fn particle_layer_from_object(
    obj: &SceneObject,
    dir: Option<&Path>,
    pkg: Option<&Package>,
) -> Option<ParticleLayer> {
    let particle_ref = match &obj.particle {
        Some(serde_json::Value::String(s)) => s.as_str(),
        _ => return None,
    };
    let json = read_particle_json(dir, pkg, particle_ref)?;
    let config: particle::ParticleConfig = serde_json::from_str(&json)
        .inspect_err(|e| eprintln!("[particle] failed to parse '{particle_ref}': {e}"))
        .ok()?;
    let overrides = obj
        .instanceoverride
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let parallax_depth = obj
        .parallax_depth
        .as_ref()
        .map(parse_parallax_depth)
        .unwrap_or([0.0, 0.0]);

    let resolved_sprite = config.material.as_deref().and_then(|mat_path| {
        if let Some(pkg) = pkg {
            resolve_particle_sprite_pkg(pkg, mat_path).ok()
        } else {
            dir.and_then(|dir| resolve_particle_sprite_dir(dir, mat_path).ok())
        }
    });
    let additive_blend = resolved_sprite
        .as_ref()
        .is_some_and(|(_, blending)| blending.as_deref() == Some("additive"));
    let sprite_texture = resolved_sprite.map(|(sprite, _)| sprite);

    Some(ParticleLayer {
        name: obj.name.clone().unwrap_or_default(),
        origin: obj.parsed_origin(),
        parallax_depth,
        config,
        overrides,
        sprite_texture,
        additive_blend,
        order_index: 0,
    })
}

/// `text` is either a plain JSON string or a `{"user": ..., "value": "..."}`
/// wrapper (scene.json's usual user-setting shape) — deliberately NOT routed
/// through `json_to_animated`, which treats 2-3 whitespace-separated numbers
/// as a vector; that heuristic is wrong for text content that happens to be
/// purely numeric (e.g. a countdown reading "12 34").
fn extract_text_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map.get("value").and_then(extract_text_string),
        _ => None,
    }
}

/// Build a text object's rendered bitmap as a normal `Layer` (see
/// `engine::text` module docs for why: no scripting engine means text never
/// changes after this one rasterization, so it can reuse 100% of the
/// existing image-layer pipeline in both render paths for free).
fn text_layer_from_object(
    obj: &SceneObject,
    dir: Option<&Path>,
    pkg: Option<&Package>,
) -> Option<Layer> {
    let text_str = obj.text.as_ref().and_then(extract_text_string)?;
    if text_str.is_empty() {
        return None;
    }
    let point_size = obj
        .pointsize
        .as_ref()
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.get("value").and_then(|v| v.as_f64()))
        })
        .unwrap_or(32.0) as f32;
    let font_data = super::text::resolve_font_data(obj.font.as_deref(), dir, pkg)?;
    let image = super::text::rasterize(&font_data, &text_str, point_size)?;

    let halign = obj
        .horizontalalign
        .as_deref()
        .or(obj.alignment.as_deref())
        .unwrap_or("center");
    let valign = obj.verticalalign.as_deref().unwrap_or("center");
    let combined = format!("{halign} {valign}");

    Some(layer_from_object(
        obj,
        LoadedImage::single(image),
        Some(&combined),
    ))
}

/// Decode a parsed .tex to RGBA, routing embedded-video payloads (mp4
/// inside the container, e.g. 2914504963's Taj Mahal backdrop) through
/// ffmpeg's first-frame decode instead of the pixel path — a static frame
/// of the right content beats the gray placeholder the layer otherwise
/// falls back to. (Streaming playback of embedded videos is future work.)
fn tex_to_rgba(tex: &TexFile) -> Result<RgbaImage> {
    if let Some(bytes) = tex.video_bytes() {
        let tmp = std::env::temp_dir().join(format!(
            "we_embvid_{}.mp4",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, bytes)
            .with_context(|| format!("writing embedded video temp file: {}", tmp.display()))?;
        let result = crate::render::ffmpeg::decode_first_frame(&tmp);
        let _ = std::fs::remove_file(&tmp);
        return result;
    }
    tex.to_rgba()
}

fn load_texture_from_dir(dir: &Path, image_path: &str) -> Result<LoadedImage> {
    // If it's a .json reference, resolve the model/material chain
    if image_path.ends_with(".json") {
        return resolve_model_chain_dir(dir, image_path);
    }

    let full_path = dir.join(image_path);

    // Video file — extract first frame as static thumbnail
    if is_video_path(image_path) && full_path.exists() {
        return crate::render::ffmpeg::decode_first_frame(&full_path).map(LoadedImage::single);
    }

    // Try as .tex first
    let tex_path = full_path.with_extension("tex");
    if tex_path.exists() {
        let data =
            std::fs::read(&tex_path).with_context(|| format!("reading {}", tex_path.display()))?;
        let tex =
            TexFile::parse(&data).with_context(|| format!("parsing {}", tex_path.display()))?;
        return LoadedImage::from_tex(&tex);
    }

    if full_path.exists() {
        return image::open(&full_path)
            .map(|i| LoadedImage::single(i.into_rgba8()))
            .with_context(|| format!("loading {}", full_path.display()));
    }

    let tex_path_str = format!("{}.tex", full_path.display());
    let tex_appended = Path::new(&tex_path_str);
    if tex_appended.exists() {
        let data = std::fs::read(tex_appended)
            .with_context(|| format!("reading {}", tex_appended.display()))?;
        let tex = TexFile::parse(&data)?;
        return LoadedImage::from_tex(&tex);
    }

    anyhow::bail!("texture not found: {image_path}")
}

fn load_texture_from_pkg(pkg: &Package, image_path: &str) -> Result<LoadedImage> {
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
        let tex =
            TexFile::parse(data).with_context(|| format!("parsing .tex from pkg: {tex_name}"))?;
        return LoadedImage::from_tex(&tex);
    }

    // Try original path (png/jpg or video embedded in pkg)
    if let Some(data) = pkg.get(image_path) {
        if is_video_path(image_path) {
            let ext = std::path::Path::new(image_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mp4");
            let tmp = std::env::temp_dir().join(format!(
                "we_vidtex_{}.{ext}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(&tmp, data)
                .with_context(|| format!("writing video temp file: {}", tmp.display()))?;
            let result = crate::render::ffmpeg::decode_first_frame(&tmp);
            let _ = std::fs::remove_file(&tmp);
            return result.map(LoadedImage::single);
        }
        return image::load_from_memory(data)
            .map(|i| LoadedImage::single(i.into_rgba8()))
            .with_context(|| format!("loading image from pkg: {image_path}"));
    }

    anyhow::bail!("texture not found in package: {image_path}")
}

/// Follow the model -> material -> texture reference chain in a PKG archive.
/// Shared global WE assets install, used as a fallback when a model/material/
/// texture isn't bundled with this specific wallpaper. Built-in particle
/// presets (e.g. "fog1") ship their own `particles/presets/*.json` per
/// wallpaper but rely on the shared `materials/presets/*.json` + `.tex`
/// living only in the global assets install, never copied into the
/// wallpaper's own directory/pkg — see [[wp_engine_project]] memory.
fn read_from_global_assets(rel_path: &str) -> Option<Vec<u8>> {
    let dir = crate::engine::shaders::loader::find_we_assets_dir()?;
    std::fs::read(dir.join(rel_path)).ok()
}

/// Returns the resolved texture plus the material pass's own `"blending"`
/// string (e.g. `"additive"`), if any — particles need this to composite
/// correctly (see `resolve_particle_sprite_{pkg,dir}`); plain image layers
/// just discard it.
fn find_model_chain_tex_pkg(pkg: &Package, json_path: &str) -> Result<(TexFile, Option<String>)> {
    let data = pkg
        .get(json_path)
        .map(|d| d.to_vec())
        .or_else(|| read_from_global_assets(json_path))
        .with_context(|| format!("model/material not found in pkg or global assets: {json_path}"))?;
    let val: serde_json::Value =
        serde_json::from_slice(&data).with_context(|| format!("parsing {json_path}"))?;

    if let Some(mat_path) = val.get("material").and_then(|v| v.as_str()) {
        return find_model_chain_tex_pkg(pkg, mat_path);
    }

    if let Some(passes) = val.get("passes").and_then(|v| v.as_array()) {
        for pass in passes {
            let blending = pass
                .get("blending")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(textures) = pass.get("textures").and_then(|v| v.as_array()) {
                for tex_ref in textures {
                    if let Some(tex_name) = tex_ref.as_str() {
                        let tex_path = format!("materials/{tex_name}.tex");
                        if let Some(tex_data) = pkg
                            .get(&tex_path)
                            .map(|d| d.to_vec())
                            .or_else(|| read_from_global_assets(&tex_path))
                        {
                            let tex = TexFile::parse(&tex_data)
                                .with_context(|| format!("parsing {tex_path}"))?;
                            return Ok((tex, blending));
                        }

                        let alt_path = format!("{tex_name}.tex");
                        if let Some(tex_data) = pkg
                            .get(&alt_path)
                            .map(|d| d.to_vec())
                            .or_else(|| read_from_global_assets(&alt_path))
                        {
                            return Ok((TexFile::parse(&tex_data)?, blending));
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("could not resolve texture from {json_path}")
}

/// Follow the model -> material -> texture chain for loose files on disk.
fn find_model_chain_tex_dir(dir: &Path, json_path: &str) -> Result<(TexFile, Option<String>)> {
    let data = std::fs::read(dir.join(json_path))
        .ok()
        .or_else(|| read_from_global_assets(json_path))
        .with_context(|| format!("model/material not found in {} or global assets: {json_path}", dir.display()))?;
    let val: serde_json::Value =
        serde_json::from_slice(&data).with_context(|| format!("parsing {json_path}"))?;

    if let Some(mat_path) = val.get("material").and_then(|v| v.as_str()) {
        return find_model_chain_tex_dir(dir, mat_path);
    }

    if let Some(passes) = val.get("passes").and_then(|v| v.as_array()) {
        for pass in passes {
            let blending = pass
                .get("blending")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(textures) = pass.get("textures").and_then(|v| v.as_array()) {
                for tex_ref in textures {
                    if let Some(tex_name) = tex_ref.as_str() {
                        let tex_path = format!("materials/{tex_name}.tex");
                        if let Some(tex_data) = std::fs::read(dir.join(&tex_path))
                            .ok()
                            .or_else(|| read_from_global_assets(&tex_path))
                        {
                            return Ok((TexFile::parse(&tex_data)?, blending));
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("could not resolve texture from {json_path}")
}

fn resolve_model_chain_pkg(pkg: &Package, json_path: &str) -> Result<LoadedImage> {
    let atlas = tex_to_rgba(&find_model_chain_tex_pkg(pkg, json_path)?.0)?;
    Ok(apply_puppet_mesh(atlas, json_path, |rel| {
        pkg.get(rel).map(|d| d.to_vec()).or_else(|| read_from_global_assets(rel))
    }))
}

fn resolve_model_chain_dir(dir: &Path, json_path: &str) -> Result<LoadedImage> {
    let atlas = tex_to_rgba(&find_model_chain_tex_dir(dir, json_path)?.0)?;
    Ok(apply_puppet_mesh(atlas, json_path, |rel| {
        std::fs::read(dir.join(rel))
            .ok()
            .or_else(|| read_from_global_assets(rel))
    }))
}

/// If the model JSON names a `"puppet"` mesh, its texture is a packed UV
/// atlas — reassemble it by rasterizing the rest-pose mesh (see
/// `engine::puppet`), and keep the parsed model + atlas around as a
/// `PuppetRuntime` so the live render loop can re-pose it over time when
/// the model carries MDLA animations. On any parse failure the decoded
/// texture passes through unchanged, so a malformed .mdl degrades to the
/// old scrambled-quad behavior instead of dropping the layer.
fn apply_puppet_mesh(
    atlas: RgbaImage,
    model_json_path: &str,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> LoadedImage {
    let plain = |atlas: RgbaImage| LoadedImage::single(atlas);
    let Some(model_bytes) = read(model_json_path) else {
        return plain(atlas);
    };
    let Ok(model_json) = serde_json::from_slice::<serde_json::Value>(&model_bytes) else {
        return plain(atlas);
    };
    let Some(puppet_path) = model_json.get("puppet").and_then(|v| v.as_str()) else {
        return plain(atlas);
    };
    let Some(mdl_bytes) = read(puppet_path) else {
        eprintln!("[scene] puppet mesh '{puppet_path}' not found — drawing raw atlas");
        return plain(atlas);
    };
    let Some(model) = crate::engine::puppet::parse_model(&mdl_bytes) else {
        eprintln!("[scene] puppet mesh '{puppet_path}' unparsable — drawing raw atlas");
        return plain(atlas);
    };
    eprintln!(
        "[scene] puppet '{puppet_path}': {} vertices, {} triangles, {} bones, {} animations",
        model.mesh.positions.len(),
        model.mesh.indices.len() / 3,
        model.bones.len(),
        model.animations.len()
    );
    // With a skeleton + animation, frame 0 of the animation IS the
    // assembled pose (skin = worldAnim * inverse(atlas-space bind)); the
    // raw mesh alone only reproduces the packed atlas layout.
    let runtime = crate::engine::puppet::PuppetRuntime { model, atlas };
    let assembled = if runtime.model.has_animation() {
        runtime.render_at(0.0, runtime.atlas.width(), runtime.atlas.height())
    } else {
        crate::engine::puppet::rasterize(
            &runtime.model.mesh,
            &runtime.atlas,
            runtime.atlas.width(),
            runtime.atlas.height(),
        )
    };
    let mut loaded = LoadedImage::single(assembled);
    if runtime.model.has_animation() {
        loaded.puppet = Some(std::sync::Arc::new(runtime));
    }
    loaded
}

/// Same model -> material -> texture chain as `resolve_model_chain_{pkg,dir}`,
/// but for particle sprites: keeps every sprite-sheet frame (sliced from the
/// `.tex`'s TEXS table, if any) instead of collapsing to one flat image, plus
/// the animation's total loop duration so `ParticleSystem::set_sprite_frames`
/// can drive per-particle frame advance, and the material pass's own
/// `"blending"` string so the caller can composite additive-glow presets
/// (fog/smoke/embers/rain/lightning — the overwhelming majority of real
/// particle materials) correctly instead of always alpha-blending, which
/// makes a sprite's near-black background visibly darken the scene instead
/// of contributing nothing.
fn resolve_particle_sprite_pkg(
    pkg: &Package,
    json_path: &str,
) -> Result<(particle::ParticleSprite, Option<String>)> {
    let (tex, blending) = find_model_chain_tex_pkg(pkg, json_path)?;
    let duration: f32 = tex.frames().iter().map(|f| f.frametime).sum();
    Ok((
        particle::ParticleSprite {
            frames: tex.to_particle_rgba_frames()?,
            duration,
        },
        blending,
    ))
}

fn resolve_particle_sprite_dir(
    dir: &Path,
    json_path: &str,
) -> Result<(particle::ParticleSprite, Option<String>)> {
    let (tex, blending) = find_model_chain_tex_dir(dir, json_path)?;
    let duration: f32 = tex.frames().iter().map(|f| f.frametime).sum();
    Ok((
        particle::ParticleSprite {
            frames: tex.to_particle_rgba_frames()?,
            duration,
        },
        blending,
    ))
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

/// For model/copybackground layers we can't render, create a solid gray placeholder
/// so any screen-space effects applied to them have a visible base to work with.
fn placeholder_for(obj: &SceneObject) -> RgbaImage {
    let size = obj.parsed_size();
    let w = if size[0] > 0.0 { size[0] as u32 } else { 1920 };
    let h = if size[1] > 0.0 { size[1] as u32 } else { 1080 };
    RgbaImage::from_pixel(w, h, image::Rgba([140u8, 140, 140, 255]))
}

fn is_special_layer(image_path: &str) -> bool {
    let p = image_path.to_lowercase();
    p.contains("projectlayer") || p.contains("composelayer") || p.contains("fullscreenlayer")
}

/// WE's solid-color model bundles (`models/util/solidlayer.json`,
/// `solid_instance_model_*.json`) reference a "flat" shader that ignores its
/// texture entirely and just outputs `vec4(g_Color, g_Alpha)` — i.e. the
/// object's own color/alpha, no image. A 1×1 white texture reproduces that
/// exactly once tinted by `Layer::color`/`alpha`/`brightness`.
fn is_solid_layer_path(image_path: &str) -> bool {
    let p = image_path.to_lowercase();
    p.contains("solidlayer") || p.contains("solid_instance_model")
}

fn white_pixel() -> RgbaImage {
    RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]))
}

/// WE sizes a solid layer to the full scene when it has no explicit size
/// (`CImage::setupUniforms`: `if (solidlayer && size == 0) size = sceneSize`).
fn fill_solid_layer_sizes(layers: &mut [Layer], solid_indices: &[usize], width: u32, height: u32) {
    for &i in solid_indices {
        if layers[i].size[0] == 0.0 && layers[i].size[1] == 0.0 {
            layers[i].size = [width as f64, height as f64, 0.0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_particle_config(color: &str) -> particle::ParticleConfig {
        let json = format!(
            r#"{{
                "maxcount": 2,
                "emitter": [{{"name":"box","rate":1000}}],
                "initializer": [
                    {{"id":1,"name":"lifetimerandom","min":100,"max":100}},
                    {{"id":2,"name":"sizerandom","min":300,"max":300}},
                    {{"id":3,"name":"velocityrandom","min":"0 0 0","max":"0 0 0"}},
                    {{"id":4,"name":"colorrandom","min":"{color}","max":"{color}"}},
                    {{"id":5,"name":"alpharandom","min":1,"max":1}}
                ]
            }}"#
        );
        serde_json::from_str(&json).expect("should parse")
    }

    /// Particles should interleave with image layers by `order_index` in
    /// true scene z-order, not always draw on top of every image — build a
    /// scene with [particle(order 0, blue), image(order 1, opaque red),
    /// particle(order 2, green)] and confirm the final canvas shows the
    /// blue particle fully hidden under the opaque red layer, with the green
    /// particle visible on top of it.
    #[test]
    fn particles_and_images_interleave_by_order_index() {
        let scene: Scene = serde_json::from_str("{}").unwrap();
        let red_image = RgbaImage::from_pixel(100, 100, image::Rgba([255, 0, 0, 255]));

        let layers = vec![Layer {
            name: "red".to_string(),
            image: red_image,
            extra_frames: Vec::new(),
            frame_duration_ms: 0,
            origin: [50.0, 50.0, 0.0],
            size: [100.0, 100.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            parallax_depth: [0.0, 0.0],
            angle: 0.0,
            blend_mode: 0,
            alpha: 1.0,
            color: [1.0, 1.0, 1.0],
            brightness: 1.0,
            copybackground: false,
            no_interpolation: true,
            clamp_uvs: false,
            puppet: None,
            order_index: 1,
        }];

        let particle_layers = vec![
            ParticleLayer {
                name: "blue_particles".to_string(),
                origin: [50.0, 50.0, 0.0],
                parallax_depth: [0.0, 0.0],
                config: solid_particle_config("0 0 255"),
                overrides: None,
                sprite_texture: None,
                additive_blend: false,
                order_index: 0,
            },
            ParticleLayer {
                name: "green_particles".to_string(),
                origin: [50.0, 50.0, 0.0],
                parallax_depth: [0.0, 0.0],
                config: solid_particle_config("0 255 0"),
                overrides: None,
                sprite_texture: None,
                additive_blend: false,
                order_index: 2,
            },
        ];

        let resolved = ResolvedScene {
            width: 100,
            height: 100,
            layers,
            particle_layers,
            scene,
        };

        let canvas = resolved.render();
        let px = canvas.get_pixel(50, 50);
        assert!(
            px[2] < 20,
            "blue particle (order 0) should be fully hidden under the opaque red layer (order 1), got {px:?}"
        );
        assert!(
            px[1] > px[0],
            "green particle (order 2) should be visible on top of the red layer, got {px:?}"
        );
    }

    #[test]
    fn parallax_depth_number_broadcasts_to_both_axes() {
        let v = serde_json::json!(0.5);
        assert_eq!(parse_parallax_depth(&v), [0.5, 0.5]);
    }

    #[test]
    fn parallax_depth_vec2_string_keeps_axes_independent() {
        let v = serde_json::json!("0.5 0.0");
        assert_eq!(parse_parallax_depth(&v), [0.5, 0.0]);
    }

    #[test]
    fn parallax_depth_user_wrapped_value_unwraps() {
        let v = serde_json::json!({"user": "ui_editor_properties_depth", "value": "0.25 0.75"});
        assert_eq!(parse_parallax_depth(&v), [0.25, 0.75]);
    }

    #[test]
    fn alignment_none_produces_no_offset() {
        assert_eq!(alignment_offset(None, [100.0, 200.0]), [0.0, 0.0]);
    }

    #[test]
    fn alignment_top_shifts_down_by_half_height() {
        assert_eq!(alignment_offset(Some("top"), [100.0, 200.0]), [0.0, -100.0]);
    }

    #[test]
    fn alignment_bottom_shifts_up_by_half_height() {
        assert_eq!(
            alignment_offset(Some("bottom"), [100.0, 200.0]),
            [0.0, 100.0]
        );
    }

    #[test]
    fn alignment_left_shifts_right_by_half_width() {
        assert_eq!(alignment_offset(Some("left"), [100.0, 200.0]), [50.0, 0.0]);
    }

    #[test]
    fn alignment_right_shifts_left_by_half_width() {
        assert_eq!(
            alignment_offset(Some("right"), [100.0, 200.0]),
            [-50.0, 0.0]
        );
    }

    #[test]
    fn alignment_combines_vertical_and_horizontal() {
        assert_eq!(
            alignment_offset(Some("top left"), [100.0, 200.0]),
            [50.0, -100.0]
        );
    }

    #[test]
    fn alignment_top_and_bottom_together_only_honors_top() {
        // Matches the reference's if/else-if: mutually exclusive per axis.
        assert_eq!(
            alignment_offset(Some("top bottom"), [100.0, 200.0]),
            [0.0, -100.0]
        );
    }

    #[test]
    fn alignment_is_case_insensitive() {
        assert_eq!(alignment_offset(Some("TOP"), [100.0, 200.0]), [0.0, -100.0]);
    }
}
