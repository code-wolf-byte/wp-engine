use super::resolver::AssetResolver;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn find_we_assets_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let home = Path::new(&home);

    let candidates = [
        home.join(".steam/steam/steamapps/common/wallpaper_engine/assets"),
        home.join(".local/share/Steam/steamapps/common/wallpaper_engine/assets"),
    ];

    candidates.into_iter().find(|p| p.exists())
}

/// Load a GLSL shader, checking the effect bundle first
/// (`{effect_dir}/shaders/`, e.g. `effects/clouds/shaders/`) then the main
/// `shaders/` directory, via the resolver's wallpaper-first then
/// global-assets search order.
pub fn load_glsl_shader_with_resolver(
    resolver: &AssetResolver,
    shader_name: &str,
    effect_dir: Option<&str>,
) -> Result<(String, String)> {
    if let Some(base) = effect_dir {
        let eff_dir = format!("{base}/shaders/");
        let frag_rel = format!("{eff_dir}{shader_name}.frag");
        let vert_rel = format!("{eff_dir}{shader_name}.vert");
        if let (Some(frag_source), Some(vert_source)) = (
            resolver.read_string(&frag_rel),
            resolver.read_string(&vert_rel),
        ) {
            let dirs = [eff_dir.as_str(), "shaders/"];
            let frag_resolved = resolve_includes(resolver, &dirs, &frag_source, 0)?;
            let vert_resolved = resolve_includes(resolver, &dirs, &vert_source, 0)?;
            return Ok((frag_resolved, vert_resolved));
        }
    }

    let frag_rel = format!("shaders/{shader_name}.frag");
    let vert_rel = format!("shaders/{shader_name}.vert");
    let frag_source = resolver
        .read_string(&frag_rel)
        .with_context(|| format!("reading {frag_rel}"))?;
    let vert_source = resolver
        .read_string(&vert_rel)
        .with_context(|| format!("reading {vert_rel}"))?;

    let dirs = ["shaders/"];
    let frag_resolved = resolve_includes(resolver, &dirs, &frag_source, 0)?;
    let vert_resolved = resolve_includes(resolver, &dirs, &vert_source, 0)?;

    Ok((frag_resolved, vert_resolved))
}

fn resolve_includes(
    resolver: &AssetResolver,
    dirs: &[&str],
    source: &str,
    depth: u32,
) -> Result<String> {
    if depth > 8 {
        return Ok(source.to_string());
    }
    let mut result = String::with_capacity(source.len());

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include \"") {
            if let Some(include_file) = rest.strip_suffix('"') {
                match resolver.read_include(dirs, include_file) {
                    Some(content) => {
                        let resolved = resolve_includes(resolver, dirs, &content, depth + 1)?;
                        result.push_str(&resolved);
                        result.push('\n');
                    }
                    None => {
                        result.push_str("// [include not found: ");
                        result.push_str(include_file);
                        result.push_str("]\n");
                    }
                }
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }

    Ok(result)
}
