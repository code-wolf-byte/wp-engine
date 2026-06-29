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
        self.eye.as_ref().and_then(|v| parse_value_vec3(v))
    }
}

#[derive(Debug, Deserialize)]
pub struct OrthogonalProjection {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct General {
    #[serde(rename = "supportsaudioprocessing")]
    pub supports_audio_processing: Option<bool>,
    pub speed: Option<f64>,
    #[serde(rename = "orthogonalprojection")]
    pub orthogonal_projection: Option<OrthogonalProjection>,
    #[serde(rename = "clearcolor")]
    pub clear_color: Option<String>,
}

/// A single object/layer in the scene.
///
/// Uses `serde_json::Value` for fields that have inconsistent types across
/// different Wallpaper Engine versions (e.g., `visible` can be bool or object).
#[derive(Debug, Deserialize)]
pub struct SceneObject {
    pub name: Option<String>,
    pub visible: Option<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
    pub angles: Option<serde_json::Value>,
    pub size: Option<serde_json::Value>,
    pub scale: Option<serde_json::Value>,
    pub alpha: Option<serde_json::Value>,
    pub color: Option<serde_json::Value>,
    #[serde(rename = "parallaxDepth")]
    pub parallax_depth: Option<serde_json::Value>,
    pub image: Option<String>,
    pub particle: Option<serde_json::Value>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(rename = "colorBlendMode", default)]
    pub color_blend_mode: u32,
    #[serde(default)]
    pub copybackground: bool,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl SceneObject {
    pub fn parsed_origin(&self) -> [f64; 3] {
        self.origin.as_ref().and_then(|v| parse_value_vec3(v)).unwrap_or([0.0; 3])
    }

    pub fn parsed_size(&self) -> [f64; 3] {
        self.size.as_ref().and_then(|v| parse_value_vec3(v)).unwrap_or([0.0; 3])
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
    #[serde(rename = "file")]
    pub file: Option<String>,
    #[serde(default)]
    pub passes: Vec<Pass>,
}

#[derive(Debug, Deserialize)]
pub struct Pass {
    pub material: Option<String>,
    #[serde(default)]
    pub combos: std::collections::HashMap<String, i32>,
    #[serde(default, deserialize_with = "deserialize_textures")]
    pub textures: Vec<TextureRef>,
    #[serde(default)]
    pub constantshadervalues: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TextureRef {
    pub file: Option<String>,
}

fn deserialize_textures<'de, D>(deserializer: D) -> std::result::Result<Vec<TextureRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let items: Vec<Option<serde_json::Value>> = Vec::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .filter_map(|v| {
            let v = v?;
            if v.is_null() { return None; }
            if let Some(s) = v.as_str() {
                return Some(TextureRef { file: Some(s.to_string()) });
            }
            serde_json::from_value(v).ok()
        })
        .collect())
}

impl Scene {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .with_context(|| {
                let preview = if json.len() > 200 { &json[..200] } else { json };
                format!("failed to parse scene.json: {preview}...")
            })
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

    pub fn topological_render_order(&self) -> Vec<&SceneObject> {
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
                Self::topo_dfs(i, &self.objects, &mut visited, &mut result, &name_to_index);
            }
        }
        result
    }

    fn topo_dfs<'a>(
        index: usize,
        objects: &'a [SceneObject],
        visited: &mut Vec<bool>,
        result: &mut Vec<&'a SceneObject>,
        name_to_index: &std::collections::HashMap<String, usize>,
    ) {
        if visited[index] { return; }
        visited[index] = true;
        let obj = &objects[index];
        for dep in &obj.dependencies {
            if let Some(&di) = name_to_index.get(dep) {
                Self::topo_dfs(di, objects, visited, result, name_to_index);
            }
        }
        if let Some(parent) = &obj.parent {
            if let Some(&pi) = name_to_index.get(parent) {
                Self::topo_dfs(pi, objects, visited, result, name_to_index);
            }
        }
        result.push(obj);
    }

    pub fn texture_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.image_paths();
        for obj in &self.objects {
            for effect in &obj.effects {
                for pass in &effect.passes {
                    for tex in &pass.textures {
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
    let parts: Vec<f64> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    [
        parts.first().copied().unwrap_or(0.0),
        parts.get(1).copied().unwrap_or(0.0),
        parts.get(2).copied().unwrap_or(0.0),
    ]
}

pub fn parse_value_vec3(v: &serde_json::Value) -> Option<[f64; 3]> {
    match v {
        serde_json::Value::String(s) => {
            let r = parse_vec3(Some(s));
            Some(r)
        }
        serde_json::Value::Array(arr) if arr.len() >= 3 => {
            Some([
                arr[0].as_f64().unwrap_or(0.0),
                arr[1].as_f64().unwrap_or(0.0),
                arr[2].as_f64().unwrap_or(0.0),
            ])
        }
        serde_json::Value::Object(m) => {
            // SceneScript animated property: {"value": "x y z", "script": "..."}
            // Use the static default; runtime scripting is not yet supported.
            m.get("value").and_then(|inner| parse_value_vec3(inner))
        }
        _ => None,
    }
}
