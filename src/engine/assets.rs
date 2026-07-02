use anyhow::{Context, Result};
use image::RgbaImage;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

use super::pkg::Package;
use super::tex::TexFile;

/// Wallpaper asset resolver.
///
/// This is the Rust equivalent of linux-wallpaperengine's mounted asset
/// container: loose files in the workshop directory win, then packed files in
/// scene/gifscene PKGs are used as a fallback.
pub struct AssetStore {
    root: PathBuf,
    packages: Vec<Package>,
}

impl AssetStore {
    pub fn from_directory(root: &Path) -> Result<Self> {
        let mut packages = Vec::new();
        for name in ["scene.pkg", "gifscene.pkg"] {
            let path = root.join(name);
            if path.exists() {
                packages.push(Package::from_file(&path)?);
            }
        }

        Ok(Self {
            root: root.to_path_buf(),
            packages,
        })
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let normalized = normalize_asset_path(path);
        let disk_path = self.root.join(&normalized);
        if disk_path.exists() {
            return std::fs::read(&disk_path)
                .with_context(|| format!("reading {}", disk_path.display()));
        }

        for package in &self.packages {
            if let Some(data) = package.get(&normalized) {
                return Ok(data.to_vec());
            }
        }

        anyhow::bail!("asset not found: {path}")
    }

    pub fn read_string(&self, path: &str) -> Result<String> {
        let data = self.read(path)?;
        String::from_utf8(data).with_context(|| format!("{path} is not valid UTF-8"))
    }

    pub fn read_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let text = self.read_string(path)?;
        serde_json::from_str(&text).with_context(|| format!("parsing {path}"))
    }

    pub fn scene_json(&self) -> Result<String> {
        self.read_string("scene.json")
    }

    pub fn read_texture(&self, name: &str) -> Result<TextureAsset> {
        let mut last_error = None;
        for candidate in texture_candidates(name) {
            match self.read(&candidate) {
                Ok(bytes) => {
                    return Ok(TextureAsset {
                        path: candidate,
                        bytes,
                    });
                }
                Err(err) => last_error = Some(err),
            }
        }

        if let Some(err) = last_error {
            Err(err).with_context(|| format!("resolving texture {name}"))
        } else {
            anyhow::bail!("texture not found: {name}")
        }
    }

    pub fn read_texture_rgba(&self, name: &str) -> Result<RgbaImage> {
        if name.starts_with("_rt_") || name.starts_with("_alias_") || name.starts_with('$') {
            anyhow::bail!("runtime render target texture is not backed by an asset: {name}");
        }

        let asset = self.read_texture(name)?;
        match TexFile::parse(&asset.bytes) {
            Ok(tex) => tex
                .to_rgba()
                .with_context(|| format!("decoding {}", asset.path)),
            Err(tex_err) => image::load_from_memory(&asset.bytes)
                .map(|image| image.into_rgba8())
                .with_context(|| {
                    format!(
                        "loading {} as image after TEX parse failed: {tex_err}",
                        asset.path
                    )
                }),
        }
    }
}

pub struct TextureAsset {
    pub path: String,
    pub bytes: Vec<u8>,
}

fn normalize_asset_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

fn texture_candidates(name: &str) -> Vec<String> {
    let normalized = normalize_asset_path(name);
    let lower = normalized.to_lowercase();
    let has_ext = Path::new(&normalized).extension().is_some();
    let in_materials = lower.starts_with("materials/");

    let mut candidates = Vec::new();

    if has_ext {
        candidates.push(normalized.clone());
        if !in_materials {
            candidates.push(format!("materials/{normalized}"));
        }
    } else {
        if !in_materials {
            candidates.push(format!("materials/{normalized}.tex"));
        }
        candidates.push(format!("{normalized}.tex"));
        candidates.push(normalized.clone());
    }

    candidates.dedup();
    candidates
}
