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
    let shader_dir = assets_dir.join("shaders");

    let frag_path = shader_dir.join(format!("{shader_name}.frag"));
    let vert_path = shader_dir.join(format!("{shader_name}.vert"));

    let frag_source = std::fs::read_to_string(&frag_path)
        .with_context(|| format!("reading {}", frag_path.display()))?;
    let vert_source = std::fs::read_to_string(&vert_path)
        .with_context(|| format!("reading {}", vert_path.display()))?;

    let frag_resolved = resolve_includes(&shader_dir, &frag_source)?;
    let vert_resolved = resolve_includes(&shader_dir, &vert_source)?;

    Ok((frag_resolved, vert_resolved))
}

fn resolve_includes(shader_dir: &Path, source: &str) -> Result<String> {
    let mut result = String::with_capacity(source.len());

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include \"") {
            if let Some(include_file) = rest.strip_suffix('"') {
                let include_path = shader_dir.join(include_file);
                match std::fs::read_to_string(&include_path) {
                    Ok(content) => {
                        result.push_str(&content);
                        result.push('\n');
                        continue;
                    }
                    Err(_) => {
                        result.push_str("// [include not found: ");
                        result.push_str(include_file);
                        result.push_str("]\n");
                        continue;
                    }
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }

    Ok(result)
}
