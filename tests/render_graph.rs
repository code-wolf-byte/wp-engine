use anyhow::Result;

use wp_engine::engine::{SceneGraph, SceneRenderGraph};

#[test]
fn builds_graph_from_minimal_scene_fixture() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "wp-engine-render-graph-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("materials"))?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "general": {"orthogonalprojection": {"width": 64, "height": 32}},
            "objects": [{
                "id": 7,
                "name": "Fixture Image",
                "visible": true,
                "image": "models/image.json"
            }]
        }"#,
    )?;
    std::fs::write(
        root.join("models/image.json"),
        r#"{
            "material": "materials/image.json",
            "fullscreen": true,
            "width": 64,
            "height": 32
        }"#,
    )?;
    std::fs::write(
        root.join("materials/image.json"),
        r#"{
            "passes": [{
                "shader": "genericimage2",
                "textures": ["sample"]
            }]
        }"#,
    )?;

    let graph = SceneGraph::from_directory(&root)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let object = render_graph
        .first_textured_object()
        .expect("fixture should produce a textured object");

    assert_eq!(render_graph.size, [64, 32]);
    assert_eq!(object.name, "Fixture Image");
    assert_eq!(object.base_texture.as_deref(), Some("sample"));
    assert_eq!(render_graph.passes.len(), 1);
    assert_eq!(render_graph.execution_plan().pass_indices, vec![0]);

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn assigns_global_pass_order_across_objects() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "wp-engine-render-graph-order-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("materials"))?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "general": {"orthogonalprojection": {"width": 64, "height": 32}},
            "objects": [
                {"id": 1, "name": "A", "visible": true, "image": "models/a.json"},
                {"id": 2, "name": "B", "visible": true, "image": "models/b.json"}
            ]
        }"#,
    )?;
    std::fs::write(
        root.join("models/a.json"),
        r#"{"material": "materials/a.json", "width": 64, "height": 32}"#,
    )?;
    std::fs::write(
        root.join("models/b.json"),
        r#"{"material": "materials/b.json", "width": 64, "height": 32}"#,
    )?;
    std::fs::write(
        root.join("materials/a.json"),
        r#"{"passes": [{"shader": "genericimage2", "textures": ["a"]}]}"#,
    )?;
    std::fs::write(
        root.join("materials/b.json"),
        r#"{"passes": [{"shader": "genericimage2", "textures": ["b"]}]}"#,
    )?;

    let graph = SceneGraph::from_directory(&root)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);

    assert_eq!(
        render_graph
            .passes
            .iter()
            .map(|pass| pass.id)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(render_graph.execution_plan().pass_indices, vec![0, 1]);

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn reports_effect_passes_that_target_missing_fbos() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "wp-engine-render-graph-fbo-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("materials"))?;
    std::fs::create_dir_all(root.join("effects"))?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "objects": [{
                "id": 7,
                "name": "Effect Target",
                "visible": true,
                "image": "models/image.json",
                "effects": [{"file": "effects/missing-target.json"}]
            }]
        }"#,
    )?;
    std::fs::write(
        root.join("models/image.json"),
        r#"{"material": "materials/image.json", "width": 64, "height": 32}"#,
    )?;
    std::fs::write(
        root.join("materials/image.json"),
        r#"{"passes": [{"shader": "genericimage2", "textures": ["sample"]}]}"#,
    )?;
    std::fs::write(
        root.join("effects/missing-target.json"),
        r#"{"passes": [{"target": "missing_fbo"}]}"#,
    )?;

    let graph = SceneGraph::from_directory(&root)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);

    assert!(render_graph.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("writes missing render target 'missing_fbo'")
    }));

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
