use anyhow::{Context, Result};
use std::collections::HashMap;
use crate::engine::model::ShaderModel;

const WE_COMMON_H: &str = "\
#define M_PI 3.14159265359\n\
#define M_PI_HALF 1.57079632679\n\
#define M_PI_2 6.28318530718\n\
#define SQRT_2 1.41421356237\n\
#define SQRT_3 1.73205080756\n\
float greyscale(vec3 c){return dot(c,vec3(0.299,0.587,0.114));}\n\
vec3 hsv2rgb(vec3 c){vec4 K=vec4(1.0,2.0/3.0,1.0/3.0,3.0);vec3 p=abs(fract(c.xxx+K.xyz)*6.0-K.www);return c.z*mix(K.xxx,clamp(p-K.xxx,0.0,1.0),c.y);}\n\
vec3 rgb2hsv(vec3 RGB){vec4 P=(RGB.g<RGB.b)?vec4(RGB.bg,-1.0,2.0/3.0):vec4(RGB.gb,0.0,-1.0/3.0);vec4 Q=(RGB.r<P.x)?vec4(P.xyw,RGB.r):vec4(RGB.r,P.yzx);float C=Q.x-min(Q.w,Q.y);float H=abs((Q.w-Q.y)/(6.0*C+1e-10)+Q.z);vec3 HCV=vec3(H,C,Q.x);float S=HCV.y/(HCV.z+1e-10);return vec3(HCV.x,S,HCV.z);}\n\
vec2 rotateVec2(vec2 v,float r){vec2 cs=vec2(cos(r),sin(r));return vec2(v.x*cs.x-v.y*cs.y,v.x*cs.y+v.y*cs.x);}\n\
";

const WE_COMMON_BLENDING_H: &str = "\
vec3 _we_RGBToHSL(vec3 color){vec3 hsl;float fmin=min(min(color.r,color.g),color.b);float fmax=max(max(color.r,color.g),color.b);float delta=fmax-fmin;hsl.z=(fmax+fmin)/2.0;if(delta==0.0){hsl.x=0.0;hsl.y=0.0;}else{if(hsl.z<0.5)hsl.y=delta/(fmax+fmin);else hsl.y=delta/(2.0-fmax-fmin);float dR=(((fmax-color.r)/6.0)+(delta/2.0))/delta;float dG=(((fmax-color.g)/6.0)+(delta/2.0))/delta;float dB=(((fmax-color.b)/6.0)+(delta/2.0))/delta;if(color.r==fmax)hsl.x=dB-dG;else if(color.g==fmax)hsl.x=(1.0/3.0)+dR-dB;else hsl.x=(2.0/3.0)+dG-dR;if(hsl.x<0.0)hsl.x+=1.0;else if(hsl.x>1.0)hsl.x-=1.0;}return hsl;}\n\
float _we_HueToRGB(float f1,float f2,float hue){if(hue<0.0)hue+=1.0;else if(hue>1.0)hue-=1.0;if((6.0*hue)<1.0)return f1+(f2-f1)*6.0*hue;if((2.0*hue)<1.0)return f2;if((3.0*hue)<2.0)return f1+(f2-f1)*((2.0/3.0)-hue)*6.0;return f1;}\n\
vec3 _we_HSLToRGB(vec3 hsl){if(hsl.y==0.0)return vec3(hsl.z);float f2=hsl.z<0.5?hsl.z*(1.0+hsl.y):(hsl.z+hsl.y)-(hsl.y*hsl.z);float f1=2.0*hsl.z-f2;return vec3(_we_HueToRGB(f1,f2,hsl.x+(1.0/3.0)),_we_HueToRGB(f1,f2,hsl.x),_we_HueToRGB(f1,f2,hsl.x-(1.0/3.0)));}\n\
vec4 Desaturate(vec3 color,float Desaturation){vec3 grayXfer=vec3(0.3,0.59,0.11);vec3 gray=vec3(dot(grayXfer,color));return vec4(mix(color,gray,Desaturation),1.0);}\n\
#define BlendLinearDodgef(base,blend) (base+blend)\n\
#define BlendLinearBurnf(base,blend) max(base+blend-1.0,0.0)\n\
#define BlendLightenf(base,blend) max(blend,base)\n\
#define BlendDarkenf(base,blend) min(blend,base)\n\
#define BlendScreenf(base,blend) (1.0-((1.0-base)*(1.0-blend)))\n\
#define BlendOverlayf(base,blend) (base<0.5?(2.0*base*blend):(1.0-2.0*(1.0-base)*(1.0-blend)))\n\
#define BlendSoftLightf(base,blend) ((blend<0.5)?(2.0*base*blend+base*base*(1.0-2.0*blend)):(sqrt(base)*(2.0*blend-1.0)+2.0*base*(1.0-blend)))\n\
#define BlendColorDodgef(base,blend) ((blend==1.0)?blend:min(base/(1.0-blend),1.0))\n\
#define BlendColorBurnf(base,blend) ((blend==0.0)?blend:max((1.0-((1.0-base)/blend)),0.0))\n\
#define BlendVividLightf(base,blend) ((blend<0.5)?BlendColorBurnf(base,(2.0*blend)):BlendColorDodgef(base,(2.0*(blend-0.5))))\n\
#define BlendPinLightf(base,blend) ((blend<0.5)?BlendDarkenf(base,(2.0*blend)):BlendLightenf(base,(2.0*(blend-0.5))))\n\
#define BlendHardMixf(base,blend) ((BlendVividLightf(base,blend)<0.5)?0.0:1.0)\n\
#define BlendReflectf(base,blend) ((blend==1.0)?blend:min(base*base/(1.0-blend),1.0))\n\
#define BlendNormal(base,blend) (blend)\n\
#define BlendLighten BlendLightenf\n\
#define BlendDarken BlendDarkenf\n\
#define BlendMultiply(base,blend) (base*blend)\n\
#define BlendAverage(base,blend) ((base+blend)/2.0)\n\
#define BlendAdd(base,blend) min(base+blend,vec3(1.0))\n\
#define BlendSubstract(base,blend) max(base+blend-vec3(1.0),vec3(0.0))\n\
#define BlendDifference(base,blend) abs(base-blend)\n\
#define BlendNegation(base,blend) (vec3(1.0)-abs(vec3(1.0)-base-blend))\n\
#define BlendExclusion(base,blend) (base+blend-2.0*base*blend)\n\
#define BlendScreen(base,blend) vec3(BlendScreenf(base.r,blend.r),BlendScreenf(base.g,blend.g),BlendScreenf(base.b,blend.b))\n\
#define BlendOverlay(base,blend) vec3(BlendOverlayf(base.r,blend.r),BlendOverlayf(base.g,blend.g),BlendOverlayf(base.b,blend.b))\n\
#define BlendSoftLight(base,blend) vec3(BlendSoftLightf(base.r,blend.r),BlendSoftLightf(base.g,blend.g),BlendSoftLightf(base.b,blend.b))\n\
#define BlendHardLight(base,blend) BlendOverlay(blend,base)\n\
#define BlendColorDodge(base,blend) vec3(BlendColorDodgef(base.r,blend.r),BlendColorDodgef(base.g,blend.g),BlendColorDodgef(base.b,blend.b))\n\
#define BlendColorBurn(base,blend) vec3(BlendColorBurnf(base.r,blend.r),BlendColorBurnf(base.g,blend.g),BlendColorBurnf(base.b,blend.b))\n\
#define BlendLinearLight(base,blend) vec3(BlendLinearLightf(base.r,blend.r),BlendLinearLightf(base.g,blend.g),BlendLinearLightf(base.b,blend.b))\n\
#define BlendVividLight(base,blend) vec3(BlendVividLightf(base.r,blend.r),BlendVividLightf(base.g,blend.g),BlendVividLightf(base.b,blend.b))\n\
#define BlendPinLight(base,blend) vec3(BlendPinLightf(base.r,blend.r),BlendPinLightf(base.g,blend.g),BlendPinLightf(base.b,blend.b))\n\
#define BlendHardMix(base,blend) vec3(BlendHardMixf(base.r,blend.r),BlendHardMixf(base.g,blend.g),BlendHardMixf(base.b,blend.b))\n\
#define BlendReflect(base,blend) vec3(BlendReflectf(base.r,blend.r),BlendReflectf(base.g,blend.g),BlendReflectf(base.b,blend.b))\n\
#define BlendGlow(base,blend) BlendReflect(blend,base)\n\
#define BlendPhoenix(base,blend) (min(base,blend)-max(base,blend)+vec3(1.0))\n\
#define BlendLinearDodge(base,blend) min(base+blend,vec3(1.0))\n\
#define BlendLinearBurn(base,blend) max(base+blend-vec3(1.0),vec3(0.0))\n\
#define BlendTint(base,blend) (vec3(max(base.x,max(base.y,base.z)))*blend)\n\
#define BlendOpacity(base,opacity,BlendFunc,blend) mix(base,BlendFunc(base,blend),opacity)\n\
vec3 BlendHue(vec3 base,vec3 blend){vec3 b=_we_RGBToHSL(base);return _we_HSLToRGB(vec3(_we_RGBToHSL(blend).r,b.g,b.b));}\n\
vec3 BlendSaturation(vec3 base,vec3 blend){vec3 b=_we_RGBToHSL(base);return _we_HSLToRGB(vec3(b.r,_we_RGBToHSL(blend).g,b.b));}\n\
vec3 BlendColor(vec3 base,vec3 blend){vec3 b=_we_RGBToHSL(blend);return _we_HSLToRGB(vec3(b.r,b.g,_we_RGBToHSL(base).b));}\n\
vec3 BlendLuminosity(vec3 base,vec3 blend){vec3 b=_we_RGBToHSL(base);return _we_HSLToRGB(vec3(b.r,b.g,_we_RGBToHSL(blend).b));}\n\
vec3 ApplyBlending(const int blendMode,in vec3 A,in vec3 B,in float opacity){\n\
#if BLENDMODE==1\nreturn mix(A,BlendDarken(A,B),opacity);\n#endif\n\
#if BLENDMODE==2\nreturn mix(A,BlendMultiply(A,B),opacity);\n#endif\n\
#if BLENDMODE==3\nreturn mix(A,BlendColorBurn(A,B),opacity);\n#endif\n\
#if BLENDMODE==4\nreturn mix(A,BlendSubstract(A,B),opacity);\n#endif\n\
#if BLENDMODE==6\nreturn mix(A,BlendLighten(A,B),opacity);\n#endif\n\
#if BLENDMODE==7\nreturn mix(A,BlendScreen(A,B),opacity);\n#endif\n\
#if BLENDMODE==8\nreturn mix(A,BlendColorDodge(A,B),opacity);\n#endif\n\
#if BLENDMODE==9\nreturn mix(A,BlendAdd(A,B),opacity);\n#endif\n\
#if BLENDMODE==11\nreturn mix(A,BlendOverlay(A,B),opacity);\n#endif\n\
#if BLENDMODE==12\nreturn mix(A,BlendSoftLight(A,B),opacity);\n#endif\n\
#if BLENDMODE==18\nreturn mix(A,BlendDifference(A,B),opacity);\n#endif\n\
#if BLENDMODE==26\nreturn mix(A,BlendHue(A,B),opacity);\n#endif\n\
#if BLENDMODE==27\nreturn mix(A,BlendSaturation(A,B),opacity);\n#endif\n\
#if BLENDMODE==28\nreturn mix(A,BlendColor(A,B),opacity);\n#endif\n\
#if BLENDMODE==29\nreturn mix(A,BlendLuminosity(A,B),opacity);\n#endif\n\
return mix(A,B,opacity);}\n\
";

/// A single scalar-or-vector uniform to be packed into the effect UBO.
/// `glsl_type` is the GLSL type token ("float", "vec2", "vec3", "vec4", "mat4", …).
#[derive(Debug, Clone)]
pub struct UniformEntry {
    pub key: String,
    pub glsl_type: String,
    pub default: f32,
}

#[derive(Debug)]
pub struct TranslatedShader {
    pub wgsl: String,
    pub vert_wgsl: Option<String>,
    pub uniform_keys: Vec<UniformEntry>,
    pub texture_count: usize,
}

pub fn translate(model: &ShaderModel) -> Result<TranslatedShader> {
    // Shaders that use array varyings (e.g. `varying vec2 v[7]`) cannot be translated
    // because naga's SPIR-V reader rejects array-typed entry-point I/O.
    for line in model.frag_glsl.lines() {
        let t = line.trim();
        if (t.starts_with("varying ") || t.starts_with("attribute ")) {
            let tok: Vec<&str> = t.split_whitespace().collect();
            if tok.len() >= 3 {
                let name_raw = t.split("//").next().unwrap_or(t)
                    .split_whitespace().nth(2).unwrap_or("").trim_end_matches(';');
                if name_raw.contains('[') {
                    anyhow::bail!("array varying '{}' not supported by naga — skipping shader '{}'",
                        name_raw, model.name);
                }
            }
        }
    }

    let glsl = preprocess_frag(model);
    let spv = glsl_to_spirv(&glsl, naga::ShaderStage::Fragment)
        .context("GLSL→SPIR-V compilation failed")?;
    let wgsl = spirv_to_wgsl(&spv).context("SPIR-V→WGSL conversion failed")?;

    // Build a map from glsl_name → (material_key, default) for annotated uniforms.
    let annotated: std::collections::HashMap<&str, (&str, f32)> = model.value_uniforms.iter()
        .map(|u| (u.glsl_name.as_str(), (u.material_key.as_str(), u.default.as_float())))
        .collect();

    // The UBO contains ALL non-sampler uniforms in source order (same as preprocess_frag).
    // We store the GLSL type so the renderer can compute proper WGSL struct layout/padding.
    let mut uniform_keys: Vec<UniformEntry> = Vec::new();
    for line in model.frag_glsl.lines() {
        let decl = line.trim().split("//").next().unwrap_or("").trim();
        let tok: Vec<&str> = decl.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "uniform" {
            let ty = tok[1];
            if !ty.starts_with("sampler") && !ty.is_empty() {
                let name = tok[2].trim_end_matches(';');
                let (key, default) = annotated.get(name)
                    .map(|(k, d)| (k.to_string(), *d))
                    .unwrap_or_else(|| (name.to_string(), 0.0));
                uniform_keys.push(UniformEntry { key, glsl_type: ty.to_string(), default });
            }
        }
    }

    // Generate a synthetic vertex shader whose outputs match the fragment shader's declared
    // varyings.  vs_we_effect always outputs (vec4 v_TexCoord, vec2 v_Scroll) which causes
    // a pipeline validation panic when the frag expects different types (e.g. clouds expects
    // vec4 v_TexCoordClouds at location 1).  Our synthetic VS has no uniforms so it doesn't
    // conflict with any bind group layout.
    let inputs = frag_inputs(&model.frag_glsl);
    let vert_wgsl = if needs_custom_vs(&inputs) {
        let vert_glsl = synthetic_vs_glsl(&inputs);
        glsl_to_spirv(&vert_glsl, naga::ShaderStage::Vertex)
            .and_then(|spv| spirv_to_wgsl(&spv))
            .ok()
    } else {
        None
    };

    Ok(TranslatedShader {
        wgsl,
        vert_wgsl,
        uniform_keys,
        texture_count: model.texture_slots.len(),
    })
}

pub fn translate_vertex(vert_glsl: &str) -> Result<String> {
    let glsl = preprocess_vert(vert_glsl);
    let spv = glsl_to_spirv(&glsl, naga::ShaderStage::Vertex)
        .context("vertex GLSL→SPIR-V compilation failed")?;
    spirv_to_wgsl(&spv).context("vertex SPIR-V→WGSL conversion failed")
}

pub fn translate_with_vertex(model: &ShaderModel, vert_glsl: &str) -> Result<TranslatedShader> {
    let mut result = translate(model)?;
    match translate_vertex(vert_glsl) {
        Ok(wgsl) => result.vert_wgsl = Some(wgsl),
        Err(e) => eprintln!("warn: vertex shader translation failed for '{}': {e}", model.name),
    }
    Ok(result)
}

// ── GLSL → SPIR-V via shaderc ────────────────────────────────────────────────
//
// shaderc wraps glslangValidator as a linkable library — no external binary
// required, works identically on Linux (Vulkan) and macOS (Metal via wgpu).

fn glsl_to_spirv(glsl: &str, stage: naga::ShaderStage) -> Result<Vec<u8>> {
    let (kind, filename) = match stage {
        naga::ShaderStage::Fragment => (shaderc::ShaderKind::Fragment, "shader.frag"),
        naga::ShaderStage::Vertex   => (shaderc::ShaderKind::Vertex,   "shader.vert"),
        _                           => (shaderc::ShaderKind::Compute,  "shader.comp"),
    };

    let compiler = shaderc::Compiler::new()
        .context("failed to create shaderc compiler")?;
    let mut options = shaderc::CompileOptions::new()
        .context("failed to create shaderc options")?;
    // Target Vulkan SPIR-V — required for naga's spv-in reader.
    // Our preprocessor already emits Vulkan GLSL 4.50 with explicit set/binding/location qualifiers.
    options.set_target_env(shaderc::TargetEnv::Vulkan, shaderc::EnvVersion::Vulkan1_1 as u32);
    options.set_source_language(shaderc::SourceLanguage::GLSL);
    options.set_optimization_level(shaderc::OptimizationLevel::Performance);

    let artifact = compiler
        .compile_into_spirv(glsl, kind, filename, "main", Some(&options))
        .context("shaderc GLSL→SPIR-V compilation failed")?;

    Ok(artifact.as_binary_u8().to_vec())
}

// ── SPIR-V → WGSL via naga ───────────────────────────────────────────────────

fn spirv_to_wgsl(spv: &[u8]) -> Result<String> {
    let module = naga::front::spv::parse_u8_slice(spv, &naga::front::spv::Options {
        adjust_coordinate_space: true,
        ..Default::default()
    }).context("naga SPIR-V parse failed")?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    ).validate(&module).context("naga validation failed")?;

    naga::back::wgsl::write_string(
        &module,
        &info,
        naga::back::wgsl::WriterFlags::empty(),
    ).context("naga WGSL output failed")
}

// ── Preprocessors ─────────────────────────────────────────────────────────────
//
// Convert WE-style GLSL (OpenGL 2.x) to Vulkan-compatible GLSL 4.50 with
// separate texture2D + sampler bindings (naga's SPIR-V reader doesn't support
// combined sampler2D loads via OpLoad; it requires OpSampledImage from separate parts).
//
// Binding layout matches our GPU renderer's bind groups:
//   Group 0, binding 0    : texture 0 (source / framebuffer)
//   Group 0, binding 1    : shared sampler
//   Group 0, binding 2    : (buffer — unused by dynamic shader, reserved)
//   Group 0, binding 3    : texture 1
//   Group 0, binding 4    : texture 2
//   Group 0, binding 5    : texture 3
//   Group 1, binding 0    : effect UBO (scalar uniforms)

fn preprocess_frag(model: &ShaderModel) -> String {
    let src = &model.frag_glsl;

    // First pass: collect declarations in order
    let mut sampler_names: Vec<String> = Vec::new();  // g_Texture0, g_Texture1, ...
    let mut scalars:  Vec<(String, String)> = Vec::new(); // (type, name) → UBO
    let mut inputs:   Vec<(String, String)> = Vec::new(); // varyings

    for line in src.lines() {
        let t = line.trim();
        let decl = t.split("//").next().unwrap_or(t).trim();
        let tok: Vec<&str> = decl.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "uniform" {
            let ty   = tok[1];
            let name = tok[2].trim_end_matches(';');
            if ty.starts_with("sampler") {
                sampler_names.push(name.to_string());
            } else if !ty.is_empty() {
                scalars.push((ty.to_string(), name.to_string()));
            }
        } else if t.starts_with("varying ") || t.starts_with("attribute ") {
            if tok.len() >= 3 {
                let ty   = tok[1];
                let name_raw = decl.split_whitespace().nth(2).unwrap_or("").trim_end_matches(';');
                // Skip array varyings (e.g. `varying vec2 v_TexCoord[7]`) — naga cannot
                // represent arrays as entry-point I/O; the shader would fail at SPIR-V parse.
                if name_raw.contains('[') { continue; }
                inputs.push((ty.to_string(), name_raw.to_string()));
            }
        }
    }

    // Texture binding indices: 1st texture → 0, rest → 3, 4, 5, ...
    let tex_binding = |i: usize| -> u32 {
        match i { 0 => 0, n => 2 + n as u32 }  // 0→0, 1→3, 2→4, 3→5
    };

    let mut out = String::new();
    out.push_str("#version 450\n");
    out.push_str("#define frac(x) fract(x)\n");
    out.push_str("#define saturate(x) clamp((x), 0.0, 1.0)\n");
    out.push_str("#define CAST2(x) vec2(x)\n");
    out.push_str("#define CAST3(x) vec3(x)\n");
    out.push_str("#define CAST4(x) vec4(x)\n");
    // Use separate texture2D + sampler for all texture sample macros so naga sees
    // OpSampledImage (from constructor) rather than OpLoad of a combined sampler2D.
    out.push_str("#define texSample2D(s, uv) texture(sampler2D(s, _wp_sampler), uv)\n");
    out.push_str("#define texSample2DLod(s, uv, lod) textureLod(sampler2D(s, _wp_sampler), uv, lod)\n");
    out.push_str("#define textureSample2D(s, uv) texture(sampler2D(s, _wp_sampler), uv)\n");
    out.push_str("#define texture2D(s, uv) texture(sampler2D(s, _wp_sampler), uv)\n");
    out.push_str("#define texture2DLod(s, uv, lod) textureLod(sampler2D(s, _wp_sampler), uv, lod)\n");
    out.push_str("#define textureCube(s, uv) texture(samplerCube(s, _wp_sampler), uv)\n");
    out.push_str("#define lerp(a, b, t) mix(a, b, t)\n");
    out.push_str("#define mul(a, b) ((b) * (a))\n");
    out.push_str(&combo_defines(&model.effective_combos()));
    // Inline WE standard includes so shaders using #include "common.h" / "common_blending.h"
    // compile without an include resolver (those directives are stripped in the body pass).
    out.push_str(WE_COMMON_H);
    out.push_str(WE_COMMON_BLENDING_H);

    // Separate texture2D declarations with matching bindings
    for (i, name) in sampler_names.iter().enumerate() {
        let b = tex_binding(i);
        out.push_str(&format!("layout(set=0, binding={b}) uniform texture2D {name};\n"));
    }
    // Shared sampler at binding 1
    if !sampler_names.is_empty() {
        out.push_str("layout(set=0, binding=1) uniform sampler _wp_sampler;\n");
    }
    // UBO for scalars at group 1 binding 0 (matches effect_bgl)
    if !scalars.is_empty() {
        out.push_str("layout(set=1, binding=0) uniform WEUniforms {\n");
        for (ty, name) in &scalars {
            out.push_str(&format!("    {ty} {name};\n"));
        }
        out.push_str("};\n");
    }
    // Input varyings with explicit locations
    for (i, (ty, name)) in inputs.iter().enumerate() {
        out.push_str(&format!("layout(location={i}) in {ty} {name};\n"));
    }
    out.push_str("layout(location=0) out vec4 fragColor;\n");

    // Emit shader body (skip declarations already emitted above)
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("// [COMBO]") || t.starts_with("#version") || t.starts_with("precision ") {
            continue;
        }
        let decl = t.split("//").next().unwrap_or(t).trim();
        let first = decl.split_whitespace().next().unwrap_or("");
        if first == "varying" || first == "attribute" || first == "uniform" { continue; }

        // Skip #include directives — included content is inlined in the header.
        if t.starts_with("#include ") { continue; }

        let renamed = rename_reserved_word(line, "sample", "_wp_s");
        let l = renamed
            .replace("gl_FragColor",   "fragColor")
            .replace("gl_FragData[0]", "fragColor");
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn preprocess_vert(glsl: &str) -> String {
    // Collect outputs (varying) and inputs (attribute) for explicit locations
    let mut attr_count = 0u32;
    let mut vary_count = 0u32;

    let mut out = String::new();
    out.push_str("#version 450\n");
    out.push_str("#define frac(x) fract(x)\n");
    out.push_str("#define saturate(x) clamp((x), 0.0, 1.0)\n");
    out.push_str("#define CAST2(x) vec2(x)\n");
    out.push_str("#define CAST3(x) vec3(x)\n");
    out.push_str("#define CAST4(x) vec4(x)\n");
    out.push_str("#define texture2D(s, uv) texture(s, uv)\n");
    out.push_str("#define lerp(a, b, t) mix(a, b, t)\n");
    out.push_str("#define mul(a, b) ((b) * (a))\n");

    // Collect scalar uniforms for UBO
    let mut scalars: Vec<(String, String)> = Vec::new();
    for line in glsl.lines() {
        let t = line.trim();
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "uniform" && !tok[1].starts_with("sampler") {
            let ty   = tok[1];
            let decl = t.split("//").next().unwrap_or(t);
            let name = decl.split_whitespace().nth(2).unwrap_or("").trim_end_matches(';').trim();
            scalars.push((ty.to_string(), name.to_string()));
        }
    }
    if !scalars.is_empty() {
        out.push_str("layout(set=0, binding=0) uniform WEUniforms {\n");
        for (ty, name) in &scalars {
            out.push_str(&format!("    {ty} {name};\n"));
        }
        out.push_str("};\n");
    }

    for line in glsl.lines() {
        let t = line.trim_start();
        if t.starts_with("#version") || t.starts_with("precision ") || t.starts_with("// [COMBO]") {
            continue;
        }
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "attribute" {
            let ty   = tok[1];
            let decl = t.split("//").next().unwrap_or(t);
            let name = decl.split_whitespace().nth(2).unwrap_or("").trim_end_matches(';').trim();
            out.push_str(&format!("layout(location={attr_count}) in {ty} {name};\n"));
            attr_count += 1;
            continue;
        }
        if tok.len() >= 3 && tok[0] == "varying" {
            let ty   = tok[1];
            let decl = t.split("//").next().unwrap_or(t);
            let name = decl.split_whitespace().nth(2).unwrap_or("").trim_end_matches(';').trim();
            out.push_str(&format!("layout(location={vary_count}) out {ty} {name};\n"));
            vary_count += 1;
            continue;
        }
        if tok.first() == Some(&"uniform") { continue; } // already in UBO
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ── Synthetic vertex shader helpers ──────────────────────────────────────────

/// Extract (type, name) pairs from fragment `varying` declarations (array varyings excluded).
fn frag_inputs(frag_glsl: &str) -> Vec<(String, String)> {
    let mut inputs = Vec::new();
    for line in frag_glsl.lines() {
        let t = line.trim();
        if !t.starts_with("varying ") && !t.starts_with("attribute ") { continue; }
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.len() < 3 { continue; }
        let name_raw = t.split("//").next().unwrap_or(t)
            .split_whitespace().nth(2).unwrap_or("").trim_end_matches(';');
        if name_raw.contains('[') { continue; }  // array varyings already rejected upstream
        inputs.push((tok[1].to_string(), name_raw.to_string()));
    }
    inputs
}

/// Return true when the fragment shader's varyings differ from the vs_we_effect interface
/// (vec4 v_TexCoord @ loc 0, vec2 v_Scroll @ loc 1).  A mismatch causes a wgpu panic.
fn needs_custom_vs(inputs: &[(String, String)]) -> bool {
    match inputs {
        [] => false,
        [(t0, _)] => t0 != "vec4",
        [(t0, _), (t1, _)] => t0 != "vec4" || t1 != "vec2",
        _ => true,  // more than 2 varyings → vs_we_effect definitely can't match
    }
}

/// Build a Vulkan GLSL 4.50 vertex shader that outputs every declared varying as a
/// simple UV-derived value.  No uniforms — this shader works with any bind group layout.
fn synthetic_vs_glsl(inputs: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str("#version 450\n");
    for (i, (ty, name)) in inputs.iter().enumerate() {
        out.push_str(&format!("layout(location={i}) out {ty} {name};\n"));
    }
    out.push_str("void main() {\n");
    out.push_str("    int vi = gl_VertexIndex;\n");
    out.push_str("    float x = float(vi & 1) * 4.0 - 1.0;\n");
    out.push_str("    float y = float(vi >> 1) * 4.0 - 1.0;\n");
    out.push_str("    gl_Position = vec4(x, y, 0.0, 1.0);\n");
    out.push_str("    float u = (x + 1.0) * 0.5;\n");
    out.push_str("    float v = (1.0 - y) * 0.5;\n");
    for (ty, name) in inputs {
        let val = match ty.as_str() {
            "float" => "u".to_string(),
            "vec2"  => "vec2(u, v)".to_string(),
            "vec3"  => "vec3(u, v, 0.0)".to_string(),
            "vec4"  => "vec4(u, v, u, v)".to_string(),
            _       => format!("{ty}(0.0)"),
        };
        out.push_str(&format!("    {name} = {val};\n"));
    }
    out.push_str("}\n");
    out
}

pub fn combo_defines(combos: &HashMap<String, i32>) -> String {
    combos.iter()
        .map(|(name, value)| format!("#define {} {}\n", name, value))
        .collect()
}

// Replace a reserved keyword with `replacement` only at word boundaries (i.e., not when
// adjacent to alphanumeric chars or underscore).  Used to rename `sample` → `_wp_s`.
fn rename_reserved_word(src: &str, keyword: &str, replacement: &str) -> String {
    let klen = keyword.len();
    let sbytes = src.as_bytes();
    let kbytes = keyword.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < sbytes.len() {
        if sbytes[i..].starts_with(kbytes) {
            let before_ok = i == 0 || !is_word_char(sbytes[i - 1]);
            let after_ok  = i + klen >= sbytes.len() || !is_word_char(sbytes[i + klen]);
            if before_ok && after_ok {
                out.push_str(replacement);
                i += klen;
                continue;
            }
        }
        out.push(sbytes[i] as char);
        i += 1;
    }
    out
}

#[inline]
fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
