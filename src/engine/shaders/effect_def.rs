use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct EffectPass {
    pub material: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EffectDef {
    pub version: Option<u32>,
    pub name: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub passes: Vec<EffectPass>,
    pub dependencies: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct MaterialPass {
    pub shader: Option<String>,
    pub blending: Option<String>,
    pub depthtest: Option<String>,
    pub depthwrite: Option<String>,
    pub cullmode: Option<String>,
    #[serde(default)]
    pub textures: Vec<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct MaterialDef {
    #[serde(default)]
    pub passes: Vec<MaterialPass>,
}

pub fn parse_effect_json(data: &[u8]) -> Result<EffectDef> {
    serde_json::from_slice(data).context("parsing effect.json")
}

pub fn parse_material_json(data: &[u8]) -> Result<MaterialDef> {
    serde_json::from_slice(data).context("parsing material JSON")
}

pub fn load_effect_from_dir(assets_dir: &Path, effect_name: &str) -> Result<EffectDef> {
    let path = assets_dir.join("effects").join(effect_name).join("effect.json");
    let data = std::fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse_effect_json(&data)
}

pub fn load_material_from_dir(assets_dir: &Path, material_path: &str) -> Result<MaterialDef> {
    let path = assets_dir.join(material_path);
    let data = std::fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse_material_json(&data)
}

/// Load material from effect bundle: assets/effects/{effect_name}/{material_path}
pub fn load_material_from_effect(assets_dir: &Path, effect_name: &str, material_path: &str) -> Result<MaterialDef> {
    let bundled = assets_dir.join("effects").join(effect_name).join(material_path);
    if bundled.exists() {
        let data = std::fs::read(&bundled)
            .with_context(|| format!("reading {}", bundled.display()))?;
        return parse_material_json(&data);
    }
    load_material_from_dir(assets_dir, material_path)
}
