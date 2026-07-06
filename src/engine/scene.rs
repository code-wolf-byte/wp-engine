use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level scene.json structure for Wallpaper Engine scene wallpapers.
///
/// Fields use `serde_json::Value` or `Option` liberally because Wallpaper
/// Engine's format varies wildly across versions and creators.
#[derive(Debug, Deserialize)]
pub struct Scene {
    pub camera: Option<Camera>,
    pub general: Option<General>,
    #[serde(default)]
    pub objects: Vec<SceneObject>,
}

#[derive(Debug, Deserialize)]
pub struct Camera {
    pub center: Option<serde_json::Value>,
    pub eye: Option<serde_json::Value>,
    #[serde(rename = "parallaxamount")]
    pub parallax_amount: Option<f64>,
    #[serde(rename = "orthogonalprojection")]
    pub orthogonal_projection: Option<OrthogonalProjection>,
}

impl Camera {
    pub fn parsed_eye(&self) -> Option<[f64; 3]> {
        self.eye.as_ref().and_then(parse_value_vec3)
    }

    pub fn parsed_center(&self) -> Option<[f64; 3]> {
        self.center.as_ref().and_then(parse_value_vec3)
    }
}

#[derive(Debug, Deserialize)]
pub struct OrthogonalProjection {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct General {
    #[serde(rename = "supportsaudioprocessing")]
    pub supports_audio_processing: Option<bool>,
    pub speed: Option<f64>,
    #[serde(rename = "orthogonalprojection")]
    pub orthogonal_projection: Option<OrthogonalProjection>,
    #[serde(rename = "clearcolor")]
    pub clear_color: Option<serde_json::Value>,
    // Camera dynamics — all user-settable via project.json properties
    // (values arrive as {"user":..., "value":...} and are resolved by the
    // property pass before deserialization).
    #[serde(rename = "camerafade")]
    pub camera_fade: Option<serde_json::Value>,
    #[serde(rename = "cameraparallax")]
    pub camera_parallax: Option<serde_json::Value>,
    #[serde(rename = "cameraparallaxamount")]
    pub camera_parallax_amount: Option<serde_json::Value>,
    #[serde(rename = "cameraparallaxdelay")]
    pub camera_parallax_delay: Option<serde_json::Value>,
    #[serde(rename = "cameraparallaxmouseinfluence")]
    pub camera_parallax_mouse_influence: Option<serde_json::Value>,
    #[serde(rename = "camerashake")]
    pub camera_shake: Option<serde_json::Value>,
    #[serde(rename = "camerashakeamplitude")]
    pub camera_shake_amplitude: Option<serde_json::Value>,
    #[serde(rename = "camerashakeroughness")]
    pub camera_shake_roughness: Option<serde_json::Value>,
    #[serde(rename = "camerashakespeed")]
    pub camera_shake_speed: Option<serde_json::Value>,
    // Scene-level bloom (user-settable)
    pub bloom: Option<serde_json::Value>,
    #[serde(rename = "bloomstrength")]
    pub bloom_strength: Option<serde_json::Value>,
    #[serde(rename = "bloomthreshold")]
    pub bloom_threshold: Option<serde_json::Value>,
}

/// A single object/layer in the scene.
///
/// Uses `serde_json::Value` for fields that have inconsistent types across
/// different Wallpaper Engine versions (e.g., `visible` can be bool or object).
#[derive(Debug, Deserialize)]
pub struct SceneObject {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub visible: Option<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
    pub angles: Option<serde_json::Value>,
    pub size: Option<serde_json::Value>,
    pub scale: Option<serde_json::Value>,
    pub alpha: Option<serde_json::Value>,
    pub color: Option<serde_json::Value>,
    #[serde(default)]
    pub brightness: Option<serde_json::Value>,
    #[serde(rename = "parallaxDepth")]
    pub parallax_depth: Option<serde_json::Value>,
    pub image: Option<String>,
    /// "top"/"bottom"/"left"/"right" (any combination, substring-matched like
    /// the reference) — shifts the object so its origin becomes an edge
    /// rather than its center. See `CImage.cpp` lines 242-256.
    #[serde(default)]
    pub alignment: Option<String>,
    pub particle: Option<serde_json::Value>,
    /// Per-instance overrides for a referenced particle preset (alpha, color,
    /// rate, size, speed) — e.g. `{"alpha":0.5,"color":"192 192 192","rate":0.16}`.
    #[serde(default)]
    pub instanceoverride: Option<serde_json::Value>,
    /// Presence of this field marks a text object (CText.cpp); the value is
    /// the initial/placeholder string (user-settable, possibly scripted —
    /// we only support the static case, matching the reference's own
    /// "Phase 1" text scope of no live script updates... actually the
    /// reference *does* support scripted text; we don't have a JS engine, so
    /// we render whatever the initial value is and leave it static).
    #[serde(default)]
    pub text: Option<serde_json::Value>,
    /// Font reference: a path into the wallpaper's own assets
    /// (`"fonts/VCR_OSD_MONO.ttf"`) or `"systemfont_*"` for a system font.
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default)]
    pub pointsize: Option<serde_json::Value>,
    /// Vertical text alignment: "top"/"center"/"bottom". Horizontal alignment
    /// reuses `alignment`/`horizontalalign` — same field the reference falls
    /// back through for text objects (ObjectParser.cpp's `parseText`).
    #[serde(default)]
    pub verticalalign: Option<String>,
    #[serde(default)]
    pub horizontalalign: Option<String>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(rename = "colorBlendMode", default)]
    pub color_blend_mode: u32,
    #[serde(default)]
    pub copybackground: bool,
    // WE uses either integer indices or string names depending on wallpaper version.
    #[serde(default)]
    pub parent: Option<serde_json::Value>,
    #[serde(default)]
    pub dependencies: Vec<serde_json::Value>,
}

impl SceneObject {
    pub fn parsed_origin(&self) -> [f64; 3] {
        self.origin
            .as_ref()
            .and_then(|v| parse_value_vec3(v))
            .unwrap_or([0.0; 3])
    }

    pub fn parsed_size(&self) -> [f64; 3] {
        self.size
            .as_ref()
            .and_then(|v| parse_value_vec3(v))
            .unwrap_or([0.0; 3])
    }

    pub fn is_visible(&self) -> bool {
        match &self.visible {
            None => true,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Object(m)) => {
                m.get("value").and_then(|v| v.as_bool()).unwrap_or(true)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Effect {
    pub id: Option<i64>,
    pub file: Option<String>,
    pub name: Option<serde_json::Value>,
    pub visible: Option<serde_json::Value>,
    #[serde(default)]
    pub passes: Vec<Pass>,
}

impl Effect {
    pub fn name_string(&self) -> Option<String> {
        match &self.name {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(value) if !value.is_null() => Some(value.to_string()),
            _ => None,
        }
    }

    pub fn is_visible(&self) -> bool {
        match &self.visible {
            None => true,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Object(m)) => {
                m.get("value").and_then(|v| v.as_bool()).unwrap_or(true)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Pass {
    pub material: Option<String>,
    #[serde(default)]
    pub combos: std::collections::HashMap<String, i32>,
    /// Positional: index N is the texture bound to shader slot N (`null`
    /// entries are real gaps, e.g. "leave this slot at its material
    /// default"), preserved as `None` rather than dropped — texture slots
    /// are matched to shader uniforms by array position, so collapsing
    /// `[null, "a", "b"]` down to `["a", "b"]` silently shifts every texture
    /// after a gap into the wrong slot (see [[wp_engine_project]] memory,
    /// nitro effect mask bug).
    #[serde(default, deserialize_with = "deserialize_textures")]
    pub textures: Vec<Option<TextureRef>>,
    #[serde(default)]
    pub constantshadervalues: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TextureRef {
    pub file: Option<String>,
    pub name: Option<String>,
}

fn deserialize_textures<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<Option<TextureRef>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let items: Vec<Option<serde_json::Value>> = Vec::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .map(|v| {
            let v = v?;
            if v.is_null() {
                return None;
            }
            if let Some(s) = v.as_str() {
                return Some(TextureRef {
                    file: Some(s.to_string()),
                    name: Some(s.to_string()),
                });
            }
            serde_json::from_value(v).ok()
        })
        .collect())
}

impl Scene {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).with_context(|| {
            let preview = if json.len() > 200 { &json[..200] } else { json };
            format!("failed to parse scene.json: {preview}...")
        })
    }

    /// Parse from an already-loaded JSON tree (used after user-property
    /// resolution has rewritten `{"user":...}` values in place).
    pub fn from_value(value: serde_json::Value) -> Result<Self> {
        serde_json::from_value(value).context("failed to parse scene.json value tree")
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(data).context("scene.json is not valid UTF-8")?;
        Self::from_json(text)
    }

    pub fn visible_objects(&self) -> impl Iterator<Item = &SceneObject> {
        self.objects.iter().filter(|o| o.is_visible())
    }

    pub fn image_paths(&self) -> Vec<&str> {
        self.objects
            .iter()
            .filter_map(|o| o.image.as_deref())
            .collect()
    }

    pub fn topological_render_order_indices(&self) -> Vec<usize> {
        let mut name_to_index = std::collections::HashMap::new();
        for (i, obj) in self.objects.iter().enumerate() {
            if let Some(name) = &obj.name {
                name_to_index.insert(name.clone(), i);
            }
        }
        let mut visited = vec![false; self.objects.len()];
        let mut result = Vec::with_capacity(self.objects.len());
        for i in 0..self.objects.len() {
            if !visited[i] {
                Self::topo_dfs_indices(i, &self.objects, &mut visited, &mut result, &name_to_index);
            }
        }
        result
    }

    pub fn topological_render_order(&self) -> Vec<&SceneObject> {
        self.topological_render_order_indices()
            .into_iter()
            .map(|index| &self.objects[index])
            .collect()
    }

    fn topo_dfs_indices(
        index: usize,
        objects: &[SceneObject],
        visited: &mut Vec<bool>,
        result: &mut Vec<usize>,
        name_to_index: &std::collections::HashMap<String, usize>,
    ) {
        if visited[index] {
            return;
        }
        visited[index] = true;
        let obj = &objects[index];
        for dep in &obj.dependencies {
            let di = if let Some(n) = dep.as_u64() {
                // integer index
                Some(n as usize).filter(|&i| i < objects.len())
            } else if let Some(s) = dep.as_str() {
                // string name
                name_to_index.get(s).copied()
            } else {
                None
            };
            if let Some(di) = di {
                Self::topo_dfs_indices(di, objects, visited, result, name_to_index);
            }
        }
        if let Some(parent) = &obj.parent {
            let pi = if let Some(n) = parent.as_u64() {
                Some(n as usize).filter(|&i| i < objects.len())
            } else if let Some(s) = parent.as_str() {
                name_to_index.get(s).copied()
            } else {
                None
            };
            if let Some(pi) = pi {
                Self::topo_dfs_indices(pi, objects, visited, result, name_to_index);
            }
        }
        result.push(index);
    }

    pub fn texture_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.image_paths();
        for obj in &self.objects {
            for effect in &obj.effects {
                for pass in &effect.passes {
                    for tex in pass.textures.iter().flatten() {
                        if let Some(f) = tex.file.as_deref() {
                            paths.push(f);
                        }
                    }
                }
            }
        }
        paths
    }
}

fn parse_vec3(s: Option<&str>) -> [f64; 3] {
    let Some(s) = s else { return [0.0; 3] };
    let parts: Vec<f64> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    [
        parts.first().copied().unwrap_or(0.0),
        parts.get(1).copied().unwrap_or(0.0),
        parts.get(2).copied().unwrap_or(0.0),
    ]
}

/// Unwrap a possibly `{"value": ...}`-wrapped JSON value to f32.
pub fn parse_value_f32(v: &serde_json::Value) -> Option<f32> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().map(|f| f as f32),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::Object(m) => m.get("value").and_then(parse_value_f32),
        _ => None,
    }
}

/// Unwrap a possibly `{"value": ...}`-wrapped JSON value to bool.
pub fn parse_value_bool(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_f64().unwrap_or(0.0) != 0.0),
        serde_json::Value::String(s) => match s.trim() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        serde_json::Value::Object(m) => m.get("value").and_then(parse_value_bool),
        _ => None,
    }
}

pub fn parse_value_vec3(v: &serde_json::Value) -> Option<[f64; 3]> {
    match v {
        serde_json::Value::String(s) => {
            let r = parse_vec3(Some(s));
            Some(r)
        }
        serde_json::Value::Array(arr) if arr.len() >= 3 => Some([
            arr[0].as_f64().unwrap_or(0.0),
            arr[1].as_f64().unwrap_or(0.0),
            arr[2].as_f64().unwrap_or(0.0),
        ]),
        serde_json::Value::Object(m) => {
            // SceneScript animated property: {"value": "x y z", "script": "..."}
            // Use the static default; runtime scripting is not yet supported.
            m.get("value").and_then(|inner| parse_value_vec3(inner))
        }
        _ => None,
    }
}
