use anyhow::Result;

use wp_engine::engine::frame_context::{FrameContext, UniformValue};
use wp_engine::engine::SceneGraph;

#[test]
fn frame_context_collects_dynamic_uniforms_and_object_rotation() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "wp-engine-frame-context-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("materials"))?;
    std::fs::create_dir_all(root.join("effects"))?;
    std::fs::write(
        root.join("scene.json"),
        r#"{
            "general": {"orthogonalprojection": {"width": 64, "height": 32}},
            "objects": [{
                "id": 1,
                "name": "Dynamic Image",
                "visible": true,
                "origin": "4 6 0",
                "angles": "0 0 90",
                "size": "8 4 0",
                "scale": "2 3 1",
                "alpha": 0.25,
                "colorBlendMode": 2,
                "image": "models/image.json",
                "effects": [{
                    "id": 9,
                    "file": "effects/example.json",
                    "passes": [{
                        "combos": {"mode": 3},
                        "constantshadervalues": {"strength": 0.5},
                        "textures": [{"file": "sample.png", "name": "sample.png"}]
                    }]
                }]
            }]
        }"#,
    )?;
    std::fs::write(
        root.join("models/image.json"),
        r#"{"material": "materials/image.json", "width": 8, "height": 4}"#,
    )?;
    std::fs::write(
        root.join("materials/image.json"),
        r#"{"passes": [{"shader": "genericimage2", "textures": ["sample.png"]}]}"#,
    )?;
    std::fs::write(
        root.join("effects/example.json"),
        r#"{"passes": [{"material": "materials/image.json"}]}"#,
    )?;
    std::fs::write(root.join("materials/sample.png"), [0u8; 4])?;

    let graph = SceneGraph::from_directory(&root)?;
    let context = FrameContext::for_graph(&graph, 1.5, 0.25, 7);

    assert_eq!(context.frame_index, 7);
    assert_eq!(context.resolution, [64, 32]);
    assert!((context.time - 1.5).abs() < f32::EPSILON);
    assert!((context.delta_time - 0.25).abs() < f32::EPSILON);
    assert_ne!(context.camera.view_projection[0][0], 1.0);

    let object = context.object_state(0).expect("object state should exist");
    assert!((object.rotation_z - 90.0).abs() < f32::EPSILON);
    assert!((object.alpha - 0.25).abs() < f32::EPSILON);
    assert_eq!(object.blend_mode, 2);
    assert!(object.transform[0][0].abs() < 1e-6);
    assert!((object.transform[0][1] - 2.0).abs() < 1e-6);
    assert!((object.transform[1][0] + 3.0).abs() < 1e-6);
    assert!(context
        .effect_uniforms
        .get("frame.time")
        .is_some_and(|value| *value == UniformValue::Float(1.5)));
    assert!(context
        .effect_uniforms
        .get("object[0].effect[0].pass[0].constant.strength")
        .is_some_and(|value| *value == UniformValue::Float(0.5)));
    assert!(context
        .effect_uniforms
        .get("object[0].effect[0].pass[0].combo.mode")
        .is_some_and(|value| *value == UniformValue::Int(3)));

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
