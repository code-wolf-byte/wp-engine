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

pub fn load_glsl_shader(assets_dir: &Path, shader_name: &str) -> Result<(String, String)> {
    load_glsl_shader_for_effect(assets_dir, shader_name, None)
}

/// Load a GLSL shader, checking the effect bundle dir first if effect_name is given.
/// WE bundles effect shaders inside assets/effects/{effect_name}/shaders/.
pub fn load_glsl_shader_for_effect(
    assets_dir: &Path,
    shader_name: &str,
    effect_name: Option<&str>,
) -> Result<(String, String)> {
    let main_shader_dir = assets_dir.join("shaders");

    if let Some(eff) = effect_name {
        let eff_shader_dir = assets_dir.join("effects").join(eff).join("shaders");
        let frag_path = eff_shader_dir.join(format!("{shader_name}.frag"));
        let vert_path = eff_shader_dir.join(format!("{shader_name}.vert"));
        if frag_path.exists() && vert_path.exists() {
            let frag_source = std::fs::read_to_string(&frag_path)
                .with_context(|| format!("reading {}", frag_path.display()))?;
            let vert_source = std::fs::read_to_string(&vert_path)
                .with_context(|| format!("reading {}", vert_path.display()))?;
            let search_dirs: Vec<&Path> = vec![&eff_shader_dir, &main_shader_dir];
            let frag_resolved = resolve_includes_multi(&search_dirs, &frag_source)?;
            let vert_resolved = resolve_includes_multi(&search_dirs, &vert_source)?;
            return Ok((frag_resolved, vert_resolved));
        }
    }

    let frag_path = main_shader_dir.join(format!("{shader_name}.frag"));
    let vert_path = main_shader_dir.join(format!("{shader_name}.vert"));

    let frag_source = std::fs::read_to_string(&frag_path)
        .with_context(|| format!("reading {}", frag_path.display()))?;
    let vert_source = std::fs::read_to_string(&vert_path)
        .with_context(|| format!("reading {}", vert_path.display()))?;

    let frag_resolved = resolve_includes(&main_shader_dir, &frag_source)?;
    let vert_resolved = resolve_includes(&main_shader_dir, &vert_source)?;

    Ok((frag_resolved, vert_resolved))
}

fn resolve_includes(shader_dir: &Path, source: &str) -> Result<String> {
    resolve_includes_multi(&[shader_dir], source)
}

fn resolve_includes_multi(dirs: &[&Path], source: &str) -> Result<String> {
    resolve_includes_depth(dirs, source, 0)
}

fn resolve_includes_depth(dirs: &[&Path], source: &str, depth: u32) -> Result<String> {
    if depth > 8 {
        return Ok(source.to_string());
    }
    let mut result = String::with_capacity(source.len());

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include \"") {
            if let Some(include_file) = rest.strip_suffix('"') {
                let mut found = false;
                for dir in dirs {
                    let include_path = dir.join(include_file);
                    if let Ok(content) = std::fs::read_to_string(&include_path) {
                        let resolved = resolve_includes_depth(dirs, &content, depth + 1)?;
                        result.push_str(&resolved);
                        result.push('\n');
                        found = true;
                        break;
                    }
                }
                if !found {
                    result.push_str("// [include not found: ");
                    result.push_str(include_file);
                    result.push_str("]\n");
                }
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }

    Ok(result)
}
