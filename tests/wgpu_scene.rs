use std::path::{Path, PathBuf};

use anyhow::Result;
use image::{Rgba, RgbaImage};
use wp_engine::engine::effect_diagnostics::collect_effect_diagnostics;
use wp_engine::engine::{FrameContext, SceneGraph, SceneRenderGraph};
use wp_engine::render::wgpu_scene::WgpuSceneRenderer;

#[test]
fn reference_fixture_parses_assets_and_builds_graph() -> Result<()> {
    let root = fixture_root("parse-assets-graph");
    let _ = std::fs::remove_dir_all(&root);
    write_basic_reference_fixture(&root)?;

    let graph = SceneGraph::from_directory(&root)?;
    let stats = graph.stats().summary();
    assert!(stats.contains("1 objects, 1 images"), "{stats}");

    let texture = graph.assets.read_texture_rgba("sample.png")?;
    assert_eq!((texture.width(), texture.height()), (2, 2));

    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    assert_eq!(render_graph.size, [8, 4]);
    assert_eq!(render_graph.objects.len(), 1);
    assert_eq!(render_graph.passes.len(), 1);
    assert_eq!(render_graph.execution_plan().pass_indices, vec![0]);

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn reference_fixture_reports_effect_shader_and_target_gaps() -> Result<()> {
    let root = fixture_root("effect-diagnostics");
    let _ = std::fs::remove_dir_all(&root);
    write_effect_diagnostic_fixture(&root)?;

    let graph = SceneGraph::from_directory(&root)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let report = collect_effect_diagnostics(&graph);
    let lines = report.summary_lines().join("\n");
    let missing = report.missing_feature_lines().join("\n");

    assert!(lines.contains("effect file=effects/tint.json"), "{lines}");
    assert!(lines.contains("passes=1"), "{lines}");
    assert!(lines.contains("fallback=tint"), "{lines}");
    assert!(lines.contains("pass0:constant:strength"), "{lines}");
    assert!(lines.contains("pass0:texture0=sample.png"), "{lines}");
    assert!(lines.contains("pass0:target=missing_fbo"), "{lines}");
    assert!(missing.contains("unsupported shader feature"), "{missing}");
    assert!(render_graph.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("writes missing render target 'missing_fbo'")
    }));

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn reference_fixture_wgpu_smoke_render_when_adapter_is_available() -> Result<()> {
    let root = fixture_root("wgpu-smoke");
    let _ = std::fs::remove_dir_all(&root);
    write_basic_reference_fixture(&root)?;

    let gpu = match wp_engine::platform::open_device() {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("skipping WGPU smoke render: {err:#}");
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }
    };

    let graph = SceneGraph::from_directory(&root)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let context = FrameContext::for_graph(&graph, 1.0, 1.0 / 60.0, 1);
    let mut renderer = WgpuSceneRenderer::new(gpu)?;
    let image = renderer.render_graph(&graph, &render_graph, &context)?;

    assert_eq!((image.width(), image.height()), (8, 4));
    assert!(
        image.pixels().any(|pixel| pixel[3] > 0),
        "smoke render should produce visible pixels"
    );

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

fn fixture_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("wp-engine-reference-{name}-{}", std::process::id()))
}

fn write_basic_reference_fixture(root: &Path) -> Result<()> {
    write_reference_dirs(root)?;
    write_reference_texture(root)?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "general": {"orthogonalprojection": {"width": 8, "height": 4}},
            "objects": [{
                "id": 1,
                "name": "Reference Image",
                "visible": true,
                "origin": "0 0 0",
                "size": "8 4 0",
                "scale": {"value": "1 1 1"},
                "color": {"value": "1 1 1"},
                "alpha": {"value": 1.0},
                "image": "models/image.json"
            }]
        }"#,
    )?;
    write_reference_model_and_material(root)?;
    Ok(())
}

fn write_effect_diagnostic_fixture(root: &Path) -> Result<()> {
    write_reference_dirs(root)?;
    write_reference_texture(root)?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "general": {"orthogonalprojection": {"width": 8, "height": 4}},
            "objects": [{
                "id": 1,
                "name": "Reference Image",
                "visible": true,
                "origin": "0 0 0",
                "size": "8 4 0",
                "image": "models/image.json",
                "effects": [{
                    "id": 9,
                    "file": "effects/tint.json",
                    "name": "Tint",
                    "passes": [{
                        "constantshadervalues": {"alpha": 0.5},
                        "textures": [{"file": "_rt_missing", "name": "_rt_missing"}]
                    }]
                }]
            }]
        }"#,
    )?;
    write_reference_model_and_material(root)?;
    std::fs::write(
        root.join("materials/effect_tint.json"),
        r#"{
            "passes": [{
                "shader": "missing_tint_shader",
                "textures": ["sample.png"],
                "constantshadervalues": {"strength": 0.5}
            }]
        }"#,
    )?;
    std::fs::write(
        root.join("effects/tint.json"),
        r#"{
            "name": "Tint",
            "passes": [{
                "material": "materials/effect_tint.json",
                "target": "missing_fbo",
                "bind": [{"index": 0, "name": "_rt_missing"}]
            }]
        }"#,
    )?;
    Ok(())
}

fn write_reference_dirs(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("materials"))?;
    std::fs::create_dir_all(root.join("effects"))?;
    Ok(())
}

fn write_reference_model_and_material(root: &Path) -> Result<()> {
    std::fs::write(
        root.join("models/image.json"),
        r#"{
            "material": "materials/image.json",
            "width": 8,
            "height": 4
        }"#,
    )?;
    std::fs::write(
        root.join("materials/image.json"),
        r#"{
            "passes": [{
                "shader": "genericimage2",
                "textures": ["sample.png"]
            }]
        }"#,
    )?;
    Ok(())
}

fn write_reference_texture(root: &Path) -> Result<()> {
    let mut image = RgbaImage::new(2, 2);
    image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
    image.put_pixel(0, 1, Rgba([0, 0, 255, 255]));
    image.put_pixel(1, 1, Rgba([255, 255, 255, 255]));
    image.save(root.join("materials/sample.png"))?;
    Ok(())
}
