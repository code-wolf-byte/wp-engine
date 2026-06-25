use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level scene.json structure for Wallpaper Engine scene wallpapers.
#[derive(Debug, Deserialize)]
pub struct Scene {
    pub camera: Option<Camera>,
    pub general: Option<General>,
    #[serde(default)]
    pub objects: Vec<SceneObject>,
}

#[derive(Debug, Deserialize)]
pub struct Camera {
    pub center: Option<Vec<f64>>,
    pub eye: Option<Vec<f64>>,
    #[serde(rename = "parallaxamount")]
    pub parallax_amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct General {
    #[serde(rename = "supportsaudioprocessing")]
    pub supports_audio_processing: Option<bool>,
    pub speed: Option<f64>,
}

/// A single object/layer in the scene.
#[derive(Debug, Deserialize)]
pub struct SceneObject {
    pub name: Option<String>,
    pub visible: Option<bool>,
    pub origin: Option<String>,
    pub angles: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "parallaxDepth")]
    pub parallax_depth: Option<serde_json::Value>,
    pub image: Option<String>,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

impl SceneObject {
    pub fn parsed_origin(&self) -> [f64; 3] {
        parse_vec3(self.origin.as_deref())
    }

    pub fn parsed_size(&self) -> [f64; 3] {
        parse_vec3(self.size.as_deref())
    }

    pub fn is_visible(&self) -> bool {
        self.visible.unwrap_or(true)
    }
}

#[derive(Debug, Deserialize)]
pub struct Effect {
    pub name: Option<String>,
    #[serde(default)]
    pub passes: Vec<Pass>,
}

#[derive(Debug, Deserialize)]
pub struct Pass {
    pub material: Option<String>,
    #[serde(default)]
    pub textures: Vec<TextureRef>,
}

#[derive(Debug, Deserialize)]
pub struct TextureRef {
    pub file: Option<String>,
}

impl Scene {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("failed to parse scene.json")
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
