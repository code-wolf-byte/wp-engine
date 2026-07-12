use anyhow::{Context, Result};
use wp_engine::engine::effect_diagnostics::collect_effect_diagnostics;
use wp_engine::engine::render_graph::DiagnosticSeverity;
use wp_engine::engine::{FrameContext, SceneGraph, SceneRenderGraph};
use wp_engine::render::wgpu_scene::WgpuSceneRenderer;
use wp_engine::workshop::{scan_wallpapers, Wallpaper, WallpaperType};

#[test]
fn workshop_scene_wallpapers_parse_graph_and_optionally_render() -> Result<()> {
    if std::env::var_os("WP_ENGINE_TEST_WORKSHOP_SCENES").is_none() {
        eprintln!("skipping workshop scene sweep; set WP_ENGINE_TEST_WORKSHOP_SCENES=1 to enable");
        return Ok(());
    }

    let max_scenes = parse_usize_env("WP_ENGINE_TEST_WORKSHOP_LIMIT")?;
    let run_gpu_smoke = std::env::var_os("WP_ENGINE_TEST_WORKSHOP_GPU").is_some();
    let wallpapers = scan_wallpapers();
    let scene_wallpapers: Vec<_> = wallpapers
        .iter()
        .filter(|wallpaper| matches!(wallpaper.wallpaper_type(), &WallpaperType::Scene))
        .collect();

    if scene_wallpapers.is_empty() {
        anyhow::bail!("WP_ENGINE_TEST_WORKSHOP_SCENES=1 but no scene wallpapers were found");
    }

    let selected_count = max_scenes
        .map(|limit| limit.min(scene_wallpapers.len()))
        .unwrap_or(scene_wallpapers.len());
    if selected_count == 0 {
        anyhow::bail!(
            "WP_ENGINE_TEST_WORKSHOP_LIMIT selected zero scene wallpapers from {} candidates",
            scene_wallpapers.len()
        );
    }

    eprintln!(
        "testing {selected_count}/{} scene wallpapers{}",
        scene_wallpapers.len(),
        if run_gpu_smoke {
            " with WGPU smoke rendering"
        } else {
            ""
        }
    );

    let mut renderer = if run_gpu_smoke {
        let gpu = wp_engine::platform::open_device().context("opening GPU for workshop sweep")?;
        Some(WgpuSceneRenderer::new(gpu).context("creating WGPU scene renderer")?)
    } else {
        None
    };

    let mut failures = Vec::new();
    let mut graph_warnings = 0usize;
    let mut missing_features = 0usize;
    let mut gpu_renders = 0usize;

    for (index, wallpaper) in scene_wallpapers.iter().take(selected_count).enumerate() {
        let label = wallpaper_label(wallpaper);
        match validate_scene_wallpaper(wallpaper, renderer.as_mut()) {
            Ok(summary) => {
                graph_warnings += summary.graph_warnings;
                missing_features += summary.missing_features;
                gpu_renders += usize::from(summary.gpu_rendered);
                eprintln!(
                    "[{}/{}] ok: {} ({})",
                    index + 1,
                    selected_count,
                    label,
                    summary.stats
                );
            }
            Err(err) => {
                eprintln!(
                    "[{}/{}] failed: {label}: {err:#}",
                    index + 1,
                    selected_count
                );
                failures.push(format!("{label}: {err:#}"));
            }
        }
    }

    if !failures.is_empty() {
        let mut message = format!(
            "{} of {selected_count} workshop scene wallpapers failed",
            failures.len()
        );
        for failure in failures.iter().take(25) {
            message.push_str("\n- ");
            message.push_str(failure);
        }
        if failures.len() > 25 {
            message.push_str(&format!("\n- ... and {} more", failures.len() - 25));
        }
        anyhow::bail!("{message}");
    }

    eprintln!(
        "workshop scene sweep passed: {selected_count} scenes, {graph_warnings} graph warnings, {missing_features} missing features, {gpu_renders} GPU renders"
    );
    Ok(())
}

struct SceneSweepSummary {
    stats: String,
    graph_warnings: usize,
    missing_features: usize,
    gpu_rendered: bool,
}

fn validate_scene_wallpaper(
    wallpaper: &Wallpaper,
    renderer: Option<&mut WgpuSceneRenderer>,
) -> Result<SceneSweepSummary> {
    let graph = SceneGraph::from_directory(&wallpaper.path)
        .with_context(|| format!("parsing scene graph from {}", wallpaper.path.display()))?;
    let stats = graph.stats().summary();
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let graph_errors = render_graph
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    if !graph_errors.is_empty() {
        anyhow::bail!(
            "render graph reported {} errors: {}",
            graph_errors.len(),
            graph_errors
                .iter()
                .take(5)
                .copied()
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    let graph_warnings = render_graph
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
        .count();
    let missing_features = collect_effect_diagnostics(&graph)
        .missing_feature_lines()
        .len();
    let mut gpu_rendered = false;

    if let Some(renderer) = renderer {
        let context = FrameContext::for_graph(&graph, 0.0, 1.0 / 60.0, 0);
        let image = renderer
            .render_graph(&graph, &render_graph, &context)
            .with_context(|| format!("WGPU smoke rendering {}", wallpaper.path.display()))?;
        if image.width() != render_graph.size[0] || image.height() != render_graph.size[1] {
            anyhow::bail!(
                "WGPU output size {}x{} did not match render graph size {}x{}",
                image.width(),
                image.height(),
                render_graph.size[0],
                render_graph.size[1]
            );
        }
        gpu_rendered = true;
    }

    Ok(SceneSweepSummary {
        stats,
        graph_warnings,
        missing_features,
        gpu_rendered,
    })
}

fn parse_usize_env(name: &str) -> Result<Option<usize>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => value
            .parse::<usize>()
            .map(Some)
            .with_context(|| format!("parsing {name}={value:?}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {name}")),
    }
}

fn wallpaper_label(wallpaper: &Wallpaper) -> String {
    format!("{} [{}]", wallpaper.title(), wallpaper.workshop_id)
}
