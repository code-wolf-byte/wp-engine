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

impl Scene {
    /// True for genuine 3D scenes: no usable orthogonal projection anywhere
    /// (the key may be absent, JSON `null`, or missing width/height — WE
    /// writes `"orthogonalprojection": null` for perspective scenes) and a
    /// camera with parseable eye+center. The reference has no perspective
    /// path (CScene.cpp always calls setOrthogonalProjection), so these
    /// scenes are broken there too; we project them ourselves.
    pub fn is_perspective(&self) -> bool {
        let usable = |o: &Option<OrthogonalProjection>| {
            o.as_ref()
                .is_some_and(|p| (p.width.is_some() && p.height.is_some()) || p.auto == Some(true))
        };
        let has_ortho = self
            .camera
            .as_ref()
            .is_some_and(|c| usable(&c.orthogonal_projection))
            || self
                .general
                .as_ref()
                .is_some_and(|g| usable(&g.orthogonal_projection));
        !has_ortho
            && self
                .camera
                .as_ref()
                .is_some_and(|c| c.parsed_eye().is_some() && c.parsed_center().is_some())
    }
}

#[derive(Debug, Deserialize)]
pub struct Camera {
    pub center: Option<serde_json::Value>,
    pub eye: Option<serde_json::Value>,
    pub up: Option<serde_json::Value>,
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

    pub fn parsed_up(&self) -> Option<[f64; 3]> {
        self.up.as_ref().and_then(parse_value_vec3)
    }
}

#[derive(Debug, Deserialize)]
pub struct OrthogonalProjection {
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `{"auto": true}` — ortho sized automatically (gifscenes): still an
    /// orthogonal projection even with null width/height.
    pub auto: Option<bool>,
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
    // Perspective projection (3D scenes; vertical FOV in degrees)
    pub fov: Option<serde_json::Value>,
    pub nearz: Option<serde_json::Value>,
    pub farz: Option<serde_json::Value>,
    // Scene-level bloom (user-settable)
    pub bloom: Option<serde_json::Value>,
    #[serde(rename = "bloomstrength")]
    pub bloom_strength: Option<serde_json::Value>,
    #[serde(rename = "bloomthreshold")]
    pub bloom_threshold: Option<serde_json::Value>,
    // HDR bloom (99/195 wallpapers). We run one SDR bloom chain, so when `hdr`
    // is on we feed it the HDR-tuned strength/threshold instead.
    pub hdr: Option<serde_json::Value>,
    #[serde(rename = "bloomhdrstrength")]
    pub bloom_hdr_strength: Option<serde_json::Value>,
    #[serde(rename = "bloomhdrthreshold")]
    pub bloom_hdr_threshold: Option<serde_json::Value>,
    // Camera zoom (105/195) — scales the whole scene about its center.
    pub zoom: Option<serde_json::Value>,
    // Whether the frame is cleared to clearcolor each frame (else transparent).
    pub clearenabled: Option<serde_json::Value>,
    // Scene ambient / sky lighting — applied as a global additive/multiplicative
    // tint (a minimal lighting approximation, no per-normal shading).
    pub ambientcolor: Option<serde_json::Value>,
    pub skylightcolor: Option<serde_json::Value>,
    // Bloom color tint.
    pub bloomtint: Option<serde_json::Value>,
    #[serde(rename = "bloomhdrfeather")]
    pub bloom_hdr_feather: Option<serde_json::Value>,
    #[serde(rename = "bloomhdrscatter")]
    pub bloom_hdr_scatter: Option<serde_json::Value>,
    #[serde(rename = "bloomhdriterations")]
    pub bloom_hdr_iterations: Option<serde_json::Value>,
    // Global particle forces (12/195).
    pub winddirection: Option<serde_json::Value>,
    pub windstrength: Option<serde_json::Value>,
    pub windenabled: Option<serde_json::Value>,
    pub gravitydirection: Option<serde_json::Value>,
    pub gravitystrength: Option<serde_json::Value>,
    // Per-scene FOV override (16/195).
    pub perspectiveoverridefov: Option<serde_json::Value>,
    // Transparency sort mode (2/195).
    pub transparentsorting: Option<serde_json::Value>,
}

impl General {
    /// Combined global particle acceleration in particle space (screen y-down):
    /// `gravitydirection*gravitystrength + winddirection*windstrength`. WE's
    /// directions are y-up, so y is negated. Returns `[0,0]` when unset.
    pub fn particle_force(&self) -> [f32; 2] {
        let term = |dir: &Option<serde_json::Value>, str: &Option<serde_json::Value>| {
            let d = dir.as_ref().and_then(parse_value_vec3).unwrap_or([0.0; 3]);
            let s = str.as_ref().and_then(parse_value_f32).unwrap_or(0.0);
            [d[0] as f32 * s, d[1] as f32 * s]
        };
        let wind_on = self
            .windenabled
            .as_ref()
            .and_then(|v| {
                v.as_bool()
                    .or_else(|| v.get("value").and_then(|i| i.as_bool()))
            })
            .unwrap_or(true);
        let g = term(&self.gravitydirection, &self.gravitystrength);
        let w = if wind_on {
            term(&self.winddirection, &self.windstrength)
        } else {
            [0.0, 0.0]
        };
        [g[0] + w[0], -(g[1] + w[1])]
    }
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
    // Text background box (rendered behind the glyphs when opaquebackground).
    #[serde(default)]
    pub opaquebackground: Option<serde_json::Value>,
    #[serde(default)]
    pub backgroundcolor: Option<serde_json::Value>,
    #[serde(default)]
    pub backgroundbrightness: Option<serde_json::Value>,
    #[serde(default)]
    pub padding: Option<serde_json::Value>,
    #[serde(default)]
    pub anchor: Option<serde_json::Value>,
    // Text wrapping / row limits.
    #[serde(default)]
    pub maxwidth: Option<serde_json::Value>,
    #[serde(default)]
    pub maxrows: Option<serde_json::Value>,
    #[serde(default)]
    pub blockalign: Option<serde_json::Value>,
    // Sound objects (audio playback).
    #[serde(default)]
    pub sound: Option<serde_json::Value>,
    #[serde(default)]
    pub volume: Option<serde_json::Value>,
    #[serde(default)]
    pub playbackmode: Option<serde_json::Value>,
    #[serde(default)]
    pub startsilent: Option<serde_json::Value>,
    // Object-level texture clamp override.
    #[serde(default)]
    pub clampuvs: Option<serde_json::Value>,
    // Light objects (radial glow approximation).
    #[serde(default)]
    pub light: Option<serde_json::Value>,
    #[serde(default)]
    pub radius: Option<serde_json::Value>,
    #[serde(default)]
    pub intensity: Option<serde_json::Value>,
    // Puppet animation layers (blended each frame).
    #[serde(default)]
    pub animationlayers: Option<serde_json::Value>,
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

    /// Inline SceneScript source driving `visible`, if the property was
    /// authored as `{"value": …, "script": "…"}`. An object with a visible
    /// script must not be dropped by the `is_visible()` load-time filter even
    /// when its authored value is `false` — the script may turn it on.
    pub fn visible_script(&self) -> Option<String> {
        match &self.visible {
            Some(serde_json::Value::Object(m)) => {
                m.get("script").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
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
