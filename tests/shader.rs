use std::borrow::Cow;
use std::collections::BTreeMap;

use wp_engine::engine::shader::{
    validate_builtin_textured_quad, ShaderLanguage, ShaderStage, ShaderTranslationRequest,
    ShaderTranslator,
};

#[test]
fn validates_builtin_textured_quad_shader() {
    validate_builtin_textured_quad().expect("builtin textured shader should validate");
}

#[test]
fn translates_basic_glsl_fragment_to_wgsl() {
    let translator = ShaderTranslator;
    let translation = translator.translate(ShaderTranslationRequest {
        label: Cow::Borrowed("test.frag"),
        language: ShaderLanguage::Glsl,
        stage: ShaderStage::Fragment,
        source: Cow::Borrowed(
            r#"
            #version 450 core
            layout(location = 0) out vec4 out_color;
            void main() {
                out_color = vec4(1.0, 0.0, 0.0, 1.0);
            }
            "#,
        ),
        defines: BTreeMap::new(),
    });

    assert!(
        translation.succeeded(),
        "translation failed: {:?}",
        translation.diagnostics
    );
    assert!(
        translation.wgsl.unwrap().contains("@fragment"),
        "WGSL output should contain a fragment entry point"
    );
}
