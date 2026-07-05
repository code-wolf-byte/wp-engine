use crate::engine::model::ShaderModel;
use anyhow::{Context, Result};
use std::collections::HashMap;

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
    pub default: [f32; 4],
}

#[derive(Debug)]
pub struct TranslatedShader {
    pub wgsl: String,
    pub vert_wgsl: Option<String>,
    pub uniform_keys: Vec<UniformEntry>,
    pub texture_count: usize,
    /// Vertex attributes (glsl type, name) in declaration order when the real
    /// WE vertex shader was translated; empty for the synthetic-VS path.
    pub attributes: Vec<(String, String)>,
}

/// Collect non-sampler `uniform` declarations (type, name) in source order.
fn collect_scalar_uniforms(src: &str) -> Vec<(String, String)> {
    let mut scalars = Vec::new();
    for line in src.lines() {
        let decl = line.trim().split("//").next().unwrap_or("").trim();
        let tok: Vec<&str> = decl.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "uniform" {
            let ty = tok[1];
            if !ty.starts_with("sampler") && !ty.is_empty() {
                let name = tok[2].trim_end_matches(';');
                // Array uniforms (audio spectra etc.) can't be packed by the
                // renderer's flat UBO writer; skip them (they read as zero).
                if !name.contains('[') {
                    scalars.push((ty.to_string(), name.to_string()));
                }
            }
        }
    }
    scalars
}

/// True if every `name[...]` access in `src` has a plain non-negative-integer
/// literal inside the brackets (no variables/expressions).
fn all_indices_are_literal(src: &str, name: &str) -> bool {
    let pattern = format!("{name}[");
    let mut search_from = 0;
    while let Some(rel) = src[search_from..].find(pattern.as_str()) {
        let start = search_from + rel + pattern.len();
        let Some(end_rel) = src[start..].find(']') else {
            return false; // unterminated — be conservative
        };
        let content = src[start..start + end_rel].trim();
        if content.parse::<usize>().is_err() {
            return false;
        }
        search_from = start + end_rel + 1;
    }
    true
}

/// Finds the balanced-bracket span starting at `open_pos` (which must point
/// at an `open` byte), skipping bracket-like bytes inside `//` line comments.
/// Returns `(span including both brackets, index just past the closer)`.
fn extract_balanced(s: &str, open_pos: usize, open: u8, close: u8) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(open_pos).copied() != Some(open) {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open_pos;
    let mut in_line_comment = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some((&s[open_pos..=i], i + 1));
            }
        }
        i += 1;
    }
    None
}

/// Parses a `for` header's three `;`-separated clauses into
/// `(loop_var_name, iteration_count)`, only recognizing the exact shape real
/// WE shaders use: `int NAME = 0; NAME < N; NAME++` (or `++NAME`, or `<=`).
/// Returns `None` for anything else — callers must leave the input untouched
/// rather than guess.
fn parse_simple_for_header(header_inner: &str) -> Option<(String, usize)> {
    let clauses: Vec<&str> = header_inner.split(';').collect();
    if clauses.len() != 3 {
        return None;
    }
    let init = clauses[0].trim();
    let init = init
        .strip_prefix("int ")
        .or_else(|| init.strip_prefix("uint "))?;
    let (name, start_val) = init.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    if start_val.trim().parse::<i64>() != Ok(0) {
        return None; // only handle the universal `= 0` starting point
    }

    let cond = clauses[1].trim();
    let (cmp_name, op, end_str) = if let Some((n, v)) = cond.split_once("<=") {
        (n.trim(), "<=", v.trim())
    } else if let Some((n, v)) = cond.split_once('<') {
        (n.trim(), "<", v.trim())
    } else {
        return None;
    };
    if cmp_name != name {
        return None;
    }
    let end_val: i64 = end_str.parse().ok()?;
    let count = match op {
        "<" => end_val,
        "<=" => end_val + 1,
        _ => return None,
    };
    if count <= 0 || count > 64 {
        return None; // sanity bound — real shaders use small fixed kernels
    }

    let incr = clauses[2].trim();
    if incr != format!("{name}++") && incr != format!("++{name}") {
        return None;
    }

    Some((name.to_string(), count as usize))
}

/// Unrolls simple, fixed-bound `for (int i = 0; i < N; i++) { BODY }` loops
/// into N literal copies of `BODY` with `i` replaced by each literal index.
///
/// Needed alongside `unroll_array_varyings`: naga's GLSL frontend rejects
/// array-typed varyings outright, and that pass only unrolls *literally*
/// indexed arrays — but some real shaders (e.g. `blur_downsample4.frag`) read
/// a fixed-size array varying through exactly this kind of loop
/// (`for (i...) texSample2D(tex, v_TexCoord[i])`). Unrolling the loop first
/// turns `v_TexCoord[i]` into `v_TexCoord[0]`, `v_TexCoord[1]`, ... so the
/// array-varying pass can then unroll the declaration too, and the shader
/// translates instead of being skipped.
///
/// Only ever touches text that matches this exact shape; anything else (a
/// different loop form, a non-zero start, a non-literal bound, nested loops
/// it can't balance) is left completely untouched — no partial rewrites.
pub fn unroll_simple_for_loops(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    loop {
        let Some(for_rel) = rest.find("for") else {
            out.push_str(rest);
            break;
        };
        let before_ok = for_rel == 0 || !is_word_char(rest.as_bytes()[for_rel - 1]);
        let after_rel = for_rel + 3;
        let after_ok = rest
            .as_bytes()
            .get(after_rel)
            .is_none_or(|&b| !is_word_char(b));
        if !before_ok || !after_ok {
            out.push_str(&rest[..after_rel]);
            rest = &rest[after_rel..];
            continue;
        }

        let paren_start =
            after_rel + rest[after_rel..].len() - rest[after_rel..].trim_start().len();
        if rest.as_bytes().get(paren_start).copied() != Some(b'(') {
            out.push_str(&rest[..after_rel]);
            rest = &rest[after_rel..];
            continue;
        }
        let Some((header, header_end)) = extract_balanced(rest, paren_start, b'(', b')') else {
            out.push_str(&rest[..after_rel]);
            rest = &rest[after_rel..];
            continue;
        };
        let Some((name, count)) = parse_simple_for_header(&header[1..header.len() - 1]) else {
            out.push_str(&rest[..header_end]);
            rest = &rest[header_end..];
            continue;
        };

        let after_header = &rest[header_end..];
        let body_open_rel = after_header.len() - after_header.trim_start().len();
        let body_open_abs = header_end + body_open_rel;
        if rest.as_bytes().get(body_open_abs).copied() != Some(b'{') {
            out.push_str(&rest[..header_end]);
            rest = &rest[header_end..];
            continue;
        }
        let Some((body, body_end)) = extract_balanced(rest, body_open_abs, b'{', b'}') else {
            out.push_str(&rest[..header_end]);
            rest = &rest[header_end..];
            continue;
        };
        let body_inner = &body[1..body.len() - 1];

        out.push_str(&rest[..for_rel]);
        for i in 0..count {
            // Each iteration gets its own `{}` scope, matching the original
            // loop body's scoping — otherwise a variable declared inside the
            // loop (e.g. `vec4 sample = ...`) becomes a flat redeclaration
            // once concatenated N times into the same enclosing scope.
            out.push_str("{\n");
            out.push_str(&rename_reserved_word(body_inner, &name, &i.to_string()));
            out.push_str("\n}\n");
        }
        rest = &rest[body_end..];
    }
    out
}

/// Splits a function-call argument list on *top-level* commas (not inside
/// nested parens), returning each argument's `(text, start_offset)` within
/// `args`.
fn split_top_level_args(args: &str) -> Vec<(&str, usize)> {
    let bytes = args.as_bytes();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                out.push((&args[start..i], start));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push((&args[start..], start));
    out
}

/// True if `s` (already trimmed) is a bare numeric literal: an optional `-`,
/// digits, and an optional `.` plus more digits — no identifiers, swizzles,
/// or function calls.
fn is_bare_number(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let s = s.strip_prefix('-').unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    for c in s.chars() {
        match c {
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    seen_digit
}

fn ensure_float_literal(trimmed: &str) -> String {
    if trimmed.contains('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.0")
    }
}

/// Legacy NVIDIA GLSL compilers tolerated `max`/`min` calls with a bare
/// numeric literal as the *first* argument and a vector expression second
/// (e.g. nitro.frag's `max(0, albedo.rgb)`), including when the literal was
/// an integer where a float is needed. Strict Vulkan GLSL (naga/shaderc)
/// requires both: the literal to be a genuine float, *and* the vector
/// argument first — GLSL only defines `genType max(genType x, float y)`, not
/// the reverse. `max`/`min` are commutative, so swapping is always safe (not
/// just when the second argument turns out to be a vector). `clamp`'s
/// numeric arguments are only ever coerced to float, never reordered, since
/// `clamp(x, minVal, maxVal)` isn't commutative.
pub fn coerce_int_literal_builtin_args(src: &str) -> String {
    const COMMUTATIVE_FUNCS: &[&str] = &["max(", "min("];
    const OTHER_FUNCS: &[&str] = &["clamp("];
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    'outer: loop {
        let mut best: Option<(usize, usize, bool)> = None; // (rel_pos, fn_len, commutative)
        for f in COMMUTATIVE_FUNCS {
            if let Some(rel) = rest.find(f) {
                if best.is_none_or(|(bp, ..)| rel < bp) {
                    best = Some((rel, f.len(), true));
                }
            }
        }
        for f in OTHER_FUNCS {
            if let Some(rel) = rest.find(f) {
                if best.is_none_or(|(bp, ..)| rel < bp) {
                    best = Some((rel, f.len(), false));
                }
            }
        }
        let Some((rel, flen, commutative)) = best else {
            out.push_str(rest);
            break 'outer;
        };
        let open_pos = rel + flen - 1; // index of the '('
        out.push_str(&rest[..open_pos]);
        let Some((call, call_end)) = extract_balanced(rest, open_pos, b'(', b')') else {
            out.push_str(&rest[open_pos..open_pos + 1]);
            rest = &rest[open_pos + 1..];
            continue;
        };
        let inner = &call[1..call.len() - 1];
        let args = split_top_level_args(inner);
        let trimmed: Vec<&str> = args.iter().map(|(a, _)| a.trim()).collect();

        let mut fixed_args: Vec<String> = trimmed.iter().map(|t| t.to_string()).collect();
        for t in fixed_args.iter_mut() {
            if is_bare_number(t) {
                *t = ensure_float_literal(t);
            }
        }
        // Only max/min, only the exact 2-arg "literal first, non-literal
        // second" shape — swap so the literal ends up in the (required)
        // second position.
        if commutative
            && fixed_args.len() == 2
            && is_bare_number(trimmed[0])
            && !is_bare_number(trimmed[1])
        {
            fixed_args.swap(0, 1);
        }

        out.push('(');
        out.push_str(&fixed_args.join(", "));
        out.push(')');
        rest = &rest[call_end..];
    }
    out
}

pub fn unroll_array_varyings(src: &str) -> String {
    // name -> (qualifier, type, count)
    let mut arrays: Vec<(String, String, String, usize)> = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        let (qualifier, rest) = if let Some(r) = t.strip_prefix("varying ") {
            ("varying", r)
        } else if let Some(r) = t.strip_prefix("attribute ") {
            ("attribute", r)
        } else {
            continue;
        };
        let rest = rest.split("//").next().unwrap_or(rest).trim();
        let rest = rest.trim_end_matches(';').trim();
        let mut parts = rest.splitn(2, char::is_whitespace);
        let Some(ty) = parts.next() else { continue };
        let Some(name_and_dim) = parts.next().map(str::trim) else {
            continue;
        };
        let Some(bracket) = name_and_dim.find('[') else {
            continue;
        };
        let name = &name_and_dim[..bracket];
        let Some(close) = name_and_dim[bracket + 1..].find(']') else {
            continue;
        };
        let dim_str = &name_and_dim[bracket + 1..bracket + 1 + close];
        let Ok(n) = dim_str.parse::<usize>() else {
            continue;
        };
        if n == 0 {
            continue;
        }
        arrays.push((name.to_string(), qualifier.to_string(), ty.to_string(), n));
    }

    if arrays.is_empty() {
        return src.to_string();
    }

    // WE shaders often declare the same array varying multiple times at
    // different sizes under mutually-exclusive `#if COMBO==N` branches (e.g.
    // godrays_gaussian.frag's `v_TexCoord[13|7|3]` per KERNEL quality level).
    // Since this pass runs before `#if` resolution, all branches are still
    // literally present; collapse same-named declarations to one, sized to
    // the largest variant seen (extra unused varyings are harmless).
    let mut merged: Vec<(String, String, String, usize)> = Vec::new();
    for (name, qualifier, ty, n) in arrays {
        if let Some(existing) = merged.iter_mut().find(|(en, ..)| *en == name) {
            existing.3 = existing.3.max(n);
        } else {
            merged.push((name, qualifier, ty, n));
        }
    }
    // Only unroll names indexed *exclusively* by literal integers everywhere
    // in the shader. Some real shaders (e.g. blur_downsample4.frag) loop over
    // the array with a variable index (`for (i...) tex[i]`); rewriting just
    // the literal-looking declaration while leaving `tex[i]` untouched would
    // unroll the declaration out from under a body reference that's still
    // there, producing "undeclared identifier" instead of a clean bail-out.
    // Leaving such names alone lets the existing "array varying not
    // supported" check below reject the shader honestly instead.
    let arrays: Vec<_> = merged
        .into_iter()
        .filter(|(name, ..)| all_indices_are_literal(src, name))
        .collect();

    // Re-extract this line's declared array name (if any) the same way as the
    // collection pass, so the declaration line is matched precisely instead
    // of by fragile substring checks against the qualifier/type prefix.
    let declared_name = |t: &str| -> Option<String> {
        let rest = t
            .strip_prefix("varying ")
            .or_else(|| t.strip_prefix("attribute "))?;
        let rest = rest.split("//").next().unwrap_or(rest).trim();
        let rest = rest.trim_end_matches(';').trim();
        let mut parts = rest.splitn(2, char::is_whitespace);
        parts.next()?;
        let name_and_dim = parts.next()?.trim();
        let bracket = name_and_dim.find('[')?;
        Some(name_and_dim[..bracket].to_string())
    };

    let mut emitted: Vec<String> = Vec::new();
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim();
        if let Some(decl_name) = declared_name(t) {
            if emitted.contains(&decl_name) {
                // Same name already declared (a smaller #if/#else variant) —
                // drop this line rather than redeclare it.
                continue;
            }
            if let Some((name, qualifier, ty, n)) = arrays.iter().find(|(n, ..)| *n == decl_name) {
                for i in 0..*n {
                    out.push_str(&format!("{qualifier} {ty} {name}_{i};\n"));
                }
                emitted.push(decl_name);
                continue;
            }
        }
        let mut rewritten = line.to_string();
        for (name, _, _, n) in &arrays {
            for i in 0..*n {
                rewritten = rewritten.replace(&format!("{name}[{i}]"), &format!("{name}_{i}"));
            }
        }
        out.push_str(&rewritten);
        out.push('\n');
    }
    out
}

pub fn translate(model: &ShaderModel) -> Result<TranslatedShader> {
    translate_full(model, None)
}

/// Translate the fragment shader and, when `vert_glsl` is given, the real WE
/// vertex shader. Both stages share one UBO layout (union of their scalar
/// uniforms, fragment's first) at group(1) binding(0), and varying locations
/// are matched by name so the naga-generated interfaces line up.
pub fn translate_full(model: &ShaderModel, vert_glsl: Option<&str>) -> Result<TranslatedShader> {
    // Shaders that use array varyings (e.g. `varying vec2 v[7]`) cannot be translated
    // because naga's SPIR-V reader rejects array-typed entry-point I/O.
    for line in model.frag_glsl.lines() {
        let t = line.trim();
        if t.starts_with("varying ") || t.starts_with("attribute ") {
            let tok: Vec<&str> = t.split_whitespace().collect();
            if tok.len() >= 3 {
                let name_raw = t
                    .split("//")
                    .next()
                    .unwrap_or(t)
                    .split_whitespace()
                    .nth(2)
                    .unwrap_or("")
                    .trim_end_matches(';');
                if name_raw.contains('[') {
                    anyhow::bail!(
                        "array varying '{}' not supported by naga — skipping shader '{}'",
                        name_raw,
                        model.name
                    );
                }
            }
        }
    }

    // Union scalar-uniform list: fragment's declarations first (their order
    // defines the front of the UBO), then vertex-only ones appended. Both
    // stages emit the SAME block so std140 offsets agree.
    let frag_scalars = collect_scalar_uniforms(&model.frag_glsl);
    let vert_only_scalars: Vec<(String, String)> = vert_glsl
        .map(|v| {
            collect_scalar_uniforms(v)
                .into_iter()
                .filter(|(_, name)| !frag_scalars.iter().any(|(_, f)| f == name))
                .collect()
        })
        .unwrap_or_default();

    let glsl = preprocess_frag(model, &vert_only_scalars);
    let spv = glsl_to_spirv(&glsl, naga::ShaderStage::Fragment)
        .context("GLSL→SPIR-V compilation failed")?;
    let wgsl = spirv_to_wgsl(&spv).context("SPIR-V→WGSL conversion failed")?;

    // glsl_name → (material_key, typed default) from `// {"material": ...}`
    // annotations, collected from BOTH stages (fragment wins on conflicts).
    let mut annotated: std::collections::HashMap<String, (String, [f32; 4])> =
        std::collections::HashMap::new();
    if let Some(v) = vert_glsl {
        for meta in crate::engine::shaders::uniform_meta::parse_uniform_metadata(v) {
            annotated.insert(
                meta.uniform_name.clone(),
                (
                    meta.material_key.unwrap_or(meta.uniform_name),
                    meta.default_value
                        .as_ref()
                        .map(json_default_vec4)
                        .unwrap_or([0.0; 4]),
                ),
            );
        }
    }
    for u in &model.value_uniforms {
        annotated.insert(
            u.glsl_name.clone(),
            (u.material_key.clone(), u.default.as_vec4()),
        );
    }

    // The UBO contains all union scalars in emission order. We store the GLSL
    // type so the renderer can compute proper std140 layout/padding.
    let uniform_keys: Vec<UniformEntry> = frag_scalars
        .iter()
        .chain(vert_only_scalars.iter())
        .map(|(ty, name)| {
            let (key, default) = annotated
                .get(name)
                .cloned()
                .unwrap_or_else(|| (name.clone(), [0.0; 4]));
            UniformEntry {
                key,
                glsl_type: ty.clone(),
                default,
            }
        })
        .collect();

    let inputs = frag_inputs(&model.frag_glsl);

    // Prefer the real WE vertex shader: it computes scrolled/scaled UVs, wave
    // phases, blur offsets etc. Fall back to a synthetic passthrough VS when
    // translation fails (the caller falls back again if the pipeline fails).
    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut vert_wgsl: Option<String> = None;
    if let Some(v) = vert_glsl {
        let union_scalars: Vec<(String, String)> = frag_scalars
            .iter()
            .chain(vert_only_scalars.iter())
            .cloned()
            .collect();
        match preprocess_vert_matched(v, &union_scalars, &inputs, &model.effective_combos()) {
            Ok((vsrc, attrs)) => {
                match glsl_to_spirv(&vsrc, naga::ShaderStage::Vertex)
                    .and_then(|spv| spirv_to_wgsl(&spv))
                {
                    Ok(w) => {
                        vert_wgsl = Some(w);
                        attributes = attrs;
                    }
                    Err(e) => eprintln!(
                        "[shader] '{}': real VS translation failed ({e}); using synthetic VS",
                        model.name
                    ),
                }
            }
            Err(e) => eprintln!(
                "[shader] '{}': real VS preprocess failed ({e}); using synthetic VS",
                model.name
            ),
        }
    }

    // Synthetic fallback: outputs match the fragment shader's declared varyings.
    // vs_we_effect always outputs (vec4 v_TexCoord, vec2 v_Scroll) which causes
    // a pipeline validation panic when the frag expects different types.
    if vert_wgsl.is_none() && needs_custom_vs(&inputs) {
        let vert_glsl = synthetic_vs_glsl(&inputs);
        vert_wgsl = glsl_to_spirv(&vert_glsl, naga::ShaderStage::Vertex)
            .and_then(|spv| spirv_to_wgsl(&spv))
            .ok();
    }

    Ok(TranslatedShader {
        wgsl,
        vert_wgsl,
        uniform_keys,
        texture_count: model.texture_slots.len(),
        attributes,
    })
}

/// Convert an annotation `default` JSON value into a vec4-shaped default.
fn json_default_vec4(val: &serde_json::Value) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    match val {
        serde_json::Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0) as f32;
            out = [f; 4];
        }
        serde_json::Value::String(s) => {
            for (i, p) in s.split_whitespace().take(4).enumerate() {
                out[i] = p.parse().unwrap_or(0.0);
            }
        }
        serde_json::Value::Array(a) => {
            for (i, p) in a.iter().take(4).enumerate() {
                out[i] = p.as_f64().unwrap_or(0.0) as f32;
            }
        }
        _ => {}
    }
    out
}

// ── GLSL → SPIR-V via shaderc ────────────────────────────────────────────────
//
// shaderc wraps glslangValidator as a linkable library — no external binary
// required, works identically on Linux (Vulkan) and macOS (Metal via wgpu).

fn glsl_to_spirv(glsl: &str, stage: naga::ShaderStage) -> Result<Vec<u8>> {
    let (kind, filename) = match stage {
        naga::ShaderStage::Fragment => (shaderc::ShaderKind::Fragment, "shader.frag"),
        naga::ShaderStage::Vertex => (shaderc::ShaderKind::Vertex, "shader.vert"),
        _ => (shaderc::ShaderKind::Compute, "shader.comp"),
    };

    let compiler = shaderc::Compiler::new().context("failed to create shaderc compiler")?;
    let mut options = shaderc::CompileOptions::new().context("failed to create shaderc options")?;
    // Target Vulkan SPIR-V — required for naga's spv-in reader.
    // Our preprocessor already emits Vulkan GLSL 4.50 with explicit set/binding/location qualifiers.
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_1 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);
    options.set_optimization_level(shaderc::OptimizationLevel::Performance);

    let artifact = compiler
        .compile_into_spirv(glsl, kind, filename, "main", Some(&options))
        .context("shaderc GLSL→SPIR-V compilation failed")?;

    Ok(artifact.as_binary_u8().to_vec())
}

// ── SPIR-V → WGSL via naga ───────────────────────────────────────────────────

fn spirv_to_wgsl(spv: &[u8]) -> Result<String> {
    let module = naga::front::spv::parse_u8_slice(
        spv,
        &naga::front::spv::Options {
            adjust_coordinate_space: true,
            ..Default::default()
        },
    )
    .context("naga SPIR-V parse failed")?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .context("naga validation failed")?;

    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .context("naga WGSL output failed")
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

fn vec_width(ty: &str) -> Option<u8> {
    match ty {
        "vec2" => Some(2),
        "vec3" => Some(3),
        "vec4" => Some(4),
        _ => None,
    }
}

fn swizzle_prefix(n: u8) -> &'static str {
    match n {
        1 => ".x",
        2 => ".xy",
        3 => ".xyz",
        4 => ".xyzw",
        _ => "",
    }
}

const TEX_SAMPLE_CALLS: &[&str] = &[
    "texSample2DLod(",
    "texSample2D(",
    "texture2DLod(",
    "textureSample2D(",
    "texture2D(",
    "textureCube(",
    "texture(",
];

/// Legacy NVIDIA GLSL compilers silently truncated the wider operand when
/// vector widths mismatched in an assignment or binary op (e.g. assigning a
/// vec4 texture sample to a `vec3` without a `.rgb` swizzle, or subtracting a
/// `vec2` from a `vec3` varying). Workshop shaders authored and only ever
/// tested on NVIDIA rely on this; naga/shaderc's stricter frontend rejects it
/// outright. `var_width` is updated in place as declarations are seen so the
/// caller can process a shader body line by line.
fn coerce_vector_widths(line: &str, var_width: &mut HashMap<String, u8>) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);

    // `vecN name = <expr>;` — track the declared width, and if <expr> is an
    // unswizzled texture sample (always vec4) narrower than N, truncate it.
    let mut declared: Option<(u8, &str)> = None;
    for w in [2u8, 3u8] {
        let prefix = format!("vec{w} ");
        if let Some(rest) = trimmed.strip_prefix(prefix.as_str()) {
            if let Some(eq_pos) = rest.find('=') {
                let name = rest[..eq_pos].trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    declared = Some((w, name));
                }
            }
        }
    }
    if let Some((w, name)) = declared {
        var_width.insert(name.to_string(), w);
        if let Some(body) = trimmed.strip_suffix(';') {
            if let Some(eq_pos) = body.find('=') {
                let rhs = body[eq_pos + 1..].trim();
                if TEX_SAMPLE_CALLS.iter().any(|c| rhs.starts_with(c))
                    && rhs.ends_with(')')
                    && w < 4
                {
                    let lhs = &body[..=eq_pos];
                    return format!("{indent}{lhs} {rhs}{};", swizzle_prefix(w));
                }
            }
        }
    }

    // Plain reassignment `name = <texture sample>;` to an already-known narrower var.
    if declared.is_none() {
        if let Some(body) = trimmed.strip_suffix(';') {
            if let Some(eq_pos) = body.find('=') {
                let (lhs, rhs) = (body[..eq_pos].trim(), body[eq_pos + 1..].trim());
                if lhs.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    if let Some(&w) = var_width.get(lhs) {
                        if w < 4
                            && TEX_SAMPLE_CALLS.iter().any(|c| rhs.starts_with(c))
                            && rhs.ends_with(')')
                        {
                            return format!("{indent}{lhs} = {rhs}{};", swizzle_prefix(w));
                        }
                    }
                }
            }
        }
    }

    // Binary arithmetic against a `CASTn(...)` broadcast: `wide_var - CASTn(x)` where
    // wide_var's known width exceeds n. Truncate wide_var's trailing components.
    let mut out = trimmed.to_string();
    for cast_w in [2u8, 3u8] {
        let marker = format!("CAST{cast_w}(");
        let mut search_from = 0;
        while let Some(rel) = out[search_from..].find(marker.as_str()) {
            let cast_pos = search_from + rel;
            // Walk left over whitespace, then the binary operator, then whitespace,
            // to find the end of the preceding operand.
            let mut i = cast_pos;
            let bytes = out.as_bytes();
            while i > 0 && bytes[i - 1] == b' ' {
                i -= 1;
            }
            if i == 0 || !matches!(bytes[i - 1], b'-' | b'+' | b'*' | b'/') {
                search_from = cast_pos + marker.len();
                continue;
            }
            i -= 1; // consume operator
            while i > 0 && bytes[i - 1] == b' ' {
                i -= 1;
            }
            let operand_end = i;
            let mut start = i;
            while start > 0
                && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
            {
                start -= 1;
            }
            let operand = &out[start..operand_end];
            if let Some(&w) = var_width.get(operand) {
                if w > cast_w && start > 0 && bytes[start - 1] != b'.' {
                    let swz = swizzle_prefix(cast_w);
                    out = format!("{}{}{}", &out[..operand_end], swz, &out[operand_end..]);
                    search_from = operand_end + swz.len() + marker.len();
                    continue;
                }
            }
            search_from = cast_pos + marker.len();
        }
    }
    format!("{indent}{out}")
}

fn preprocess_frag(model: &ShaderModel, extra_scalars: &[(String, String)]) -> String {
    let src = &model.frag_glsl;

    // First pass: collect declarations in order
    let mut sampler_names: Vec<String> = Vec::new(); // g_Texture0, g_Texture1, ...
    let mut scalars: Vec<(String, String)> = Vec::new(); // (type, name) → UBO
    let mut inputs: Vec<(String, String)> = Vec::new(); // varyings

    for line in src.lines() {
        let t = line.trim();
        let decl = t.split("//").next().unwrap_or(t).trim();
        let tok: Vec<&str> = decl.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "uniform" {
            let ty = tok[1];
            let name = tok[2].trim_end_matches(';');
            if ty.starts_with("sampler") {
                sampler_names.push(name.to_string());
            } else if !ty.is_empty() && !name.contains('[') {
                scalars.push((ty.to_string(), name.to_string()));
            }
        } else if t.starts_with("varying ") || t.starts_with("attribute ") {
            if tok.len() >= 3 {
                let ty = tok[1];
                let name_raw = decl
                    .split_whitespace()
                    .nth(2)
                    .unwrap_or("")
                    .trim_end_matches(';');
                // Skip array varyings (e.g. `varying vec2 v_TexCoord[7]`) — naga cannot
                // represent arrays as entry-point I/O; the shader would fail at SPIR-V parse.
                if name_raw.contains('[') {
                    continue;
                }
                inputs.push((ty.to_string(), name_raw.to_string()));
            }
        }
    }

    // Texture binding indices: 1st texture → 0, rest → 3, 4, 5, ...
    let tex_binding = |i: usize| -> u32 {
        match i {
            0 => 0,
            n => 2 + n as u32,
        } // 0→0, 1→3, 2→4, 3→5
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
    out.push_str(
        "#define texSample2DLod(s, uv, lod) textureLod(sampler2D(s, _wp_sampler), uv, lod)\n",
    );
    out.push_str("#define textureSample2D(s, uv) texture(sampler2D(s, _wp_sampler), uv)\n");
    out.push_str("#define texture2D(s, uv) texture(sampler2D(s, _wp_sampler), uv)\n");
    out.push_str(
        "#define texture2DLod(s, uv, lod) textureLod(sampler2D(s, _wp_sampler), uv, lod)\n",
    );
    out.push_str("#define textureCube(s, uv) texture(samplerCube(s, _wp_sampler), uv)\n");
    out.push_str("#define lerp(a, b, t) mix(a, b, t)\n");
    out.push_str("#define mul(a, b) ((b) * (a))\n");
    out.push_str(&combo_defines(&model.effective_combos()));
    // HLSL type aliases used pervasively in WE shaders.
    // These must come BEFORE any inlined shader body (common.h etc. are inlined by the loader).
    // We do NOT emit our WE_COMMON_H / WE_COMMON_BLENDING_H function bodies here because
    // the loader already inlines the real WE common.h content into the source, and emitting
    // our copies would cause "redefinition" errors in shaderc.
    out.push_str("#define float2 vec2\n");
    out.push_str("#define float3 vec3\n");
    out.push_str("#define float4 vec4\n");
    out.push_str("#define int2 ivec2\n");
    out.push_str("#define int3 ivec3\n");
    out.push_str("#define int4 ivec4\n");
    out.push_str("#define CAST3X3(x) mat3(x)\n");
    out.push_str("#define atan2(y,x) atan(y,x)\n");
    out.push_str("#define fmod(x,y) ((x)-(y)*trunc((x)/(y)))\n");
    out.push_str("#define ddx dFdx\n");
    out.push_str("#define ddy(x) dFdy(-(x))\n");
    out.push_str("#define log10(x) (log2(x)*0.301029995663981)\n");
    // Constants sometimes needed by shaders that don't include common.h
    out.push_str("#ifndef M_PI\n#define M_PI 3.14159265359\n#endif\n");
    out.push_str("#ifndef M_PI_2\n#define M_PI_2 6.28318530718\n#endif\n");

    // Separate texture2D declarations with matching bindings
    for (i, name) in sampler_names.iter().enumerate() {
        let b = tex_binding(i);
        out.push_str(&format!(
            "layout(set=0, binding={b}) uniform texture2D {name};\n"
        ));
    }
    // Shared sampler at binding 1
    if !sampler_names.is_empty() {
        out.push_str("layout(set=0, binding=1) uniform sampler _wp_sampler;\n");
    }
    // UBO for scalars at group 1 binding 0 (matches effect_bgl). The block is
    // the union of fragment + vertex scalars (fragment first) so both stages
    // agree on std140 offsets.
    if !scalars.is_empty() || !extra_scalars.is_empty() {
        out.push_str("layout(set=1, binding=0) uniform WEUniforms {\n");
        for (ty, name) in scalars.iter().chain(extra_scalars.iter()) {
            out.push_str(&format!("    {ty} {name};\n"));
        }
        out.push_str("};\n");
    }
    // Input varyings with explicit locations
    for (i, (ty, name)) in inputs.iter().enumerate() {
        out.push_str(&format!("layout(location={i}) in {ty} {name};\n"));
    }
    out.push_str("layout(location=0) out vec4 fragColor;\n");

    // Seed known vector widths (varyings + uniforms) for the truncation-coercion
    // pass below; it grows this map with local declarations as it walks the body.
    let mut var_width: HashMap<String, u8> = HashMap::new();
    for (ty, name) in inputs.iter().chain(scalars.iter()) {
        if let Some(w) = vec_width(ty) {
            var_width.insert(name.clone(), w);
        }
    }

    // Emit shader body (skip declarations already emitted above)
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("// [COMBO]") || t.starts_with("#version") || t.starts_with("precision ") {
            continue;
        }
        let decl = t.split("//").next().unwrap_or(t).trim();
        let first = decl.split_whitespace().next().unwrap_or("");
        if first == "varying" || first == "attribute" || first == "uniform" {
            continue;
        }

        // Skip #include directives — included content is inlined in the header.
        if t.starts_with("#include ") {
            continue;
        }

        // Replace #require LightingV1 with a no-op stub (same as linux-wallpaperengine).
        if t.starts_with("#require LightingV1") {
            out.push_str(
                "vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,\n",
            );
            out.push_str(
                "    vec3 specularTint, vec3 baseReflectance, float roughness, float metallic)\n",
            );
            out.push_str("{ return vec3(0.0); }\n");
            continue;
        }

        let coerced = coerce_vector_widths(line, &mut var_width);
        let renamed = rename_reserved_word(&coerced, "sample", "_wp_s");
        let l = renamed
            .replace("gl_FragColor", "fragColor")
            .replace("gl_FragData[0]", "fragColor");
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Preprocess a real WE vertex shader into Vulkan GLSL 4.50.
///
/// - Emits the SAME `WEUniforms` block as the fragment stage (union scalar
///   list, set=1 binding=0) so std140 offsets agree across stages.
/// - Assigns each varying the location of the same-named fragment input;
///   fragment inputs the VS never writes are declared and zero-initialized at
///   the top of `main()` (wgpu requires every FS input to have a VS output).
/// - Returns the processed source plus the attribute list (type, name) in
///   declaration order for vertex-buffer layout construction.
fn preprocess_vert_matched(
    glsl: &str,
    union_scalars: &[(String, String)],
    frag_inputs: &[(String, String)],
    combos: &HashMap<String, i32>,
) -> Result<(String, Vec<(String, String)>)> {
    // Vertex shaders that sample textures need image bindings we don't
    // provide in the vertex stage; let the synthetic fallback handle those.
    if glsl.contains("texSample2D") || glsl.contains("texture2D") {
        anyhow::bail!("vertex shader samples textures");
    }

    let mut out = String::new();
    out.push_str("#version 450\n");
    out.push_str("#define frac(x) fract(x)\n");
    out.push_str("#define saturate(x) clamp((x), 0.0, 1.0)\n");
    out.push_str("#define CAST2(x) vec2(x)\n");
    out.push_str("#define CAST3(x) vec3(x)\n");
    out.push_str("#define CAST4(x) vec4(x)\n");
    out.push_str("#define lerp(a, b, t) mix(a, b, t)\n");
    out.push_str("#define mul(a, b) ((b) * (a))\n");
    out.push_str(&combo_defines(combos));
    out.push_str("#define float2 vec2\n");
    out.push_str("#define float3 vec3\n");
    out.push_str("#define float4 vec4\n");
    out.push_str("#define int2 ivec2\n");
    out.push_str("#define int3 ivec3\n");
    out.push_str("#define int4 ivec4\n");
    out.push_str("#define CAST3X3(x) mat3(x)\n");
    out.push_str("#define atan2(y,x) atan(y,x)\n");
    out.push_str("#define fmod(x,y) ((x)-(y)*trunc((x)/(y)))\n");
    out.push_str("#define log10(x) (log2(x)*0.301029995663981)\n");
    out.push_str("#ifndef M_PI\n#define M_PI 3.14159265359\n#endif\n");
    out.push_str("#ifndef M_PI_2\n#define M_PI_2 6.28318530718\n#endif\n");

    if !union_scalars.is_empty() {
        out.push_str("layout(set=1, binding=0) uniform WEUniforms {\n");
        for (ty, name) in union_scalars {
            out.push_str(&format!("    {ty} {name};\n"));
        }
        out.push_str("};\n");
    }

    // Fragment-input locations by name; varyings must line up with them.
    let frag_loc: HashMap<&str, usize> = frag_inputs
        .iter()
        .enumerate()
        .map(|(i, (_, name))| (name.as_str(), i))
        .collect();
    let mut next_free_loc = frag_inputs.len();

    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut declared_varyings: Vec<String> = Vec::new();
    let mut body = String::new();

    for line in glsl.lines() {
        let t = line.trim_start();
        if t.starts_with("#version")
            || t.starts_with("precision ")
            || t.starts_with("// [COMBO]")
            || t.starts_with("#include ")
        {
            continue;
        }
        let decl = t.split("//").next().unwrap_or(t).trim();
        let tok: Vec<&str> = decl.split_whitespace().collect();
        if tok.len() >= 3 && tok[0] == "attribute" {
            let ty = tok[1].to_string();
            let name = tok[2].trim_end_matches(';').to_string();
            out.push_str(&format!(
                "layout(location={}) in {ty} {name};\n",
                attributes.len()
            ));
            attributes.push((ty, name));
            continue;
        }
        if tok.len() >= 3 && tok[0] == "varying" {
            let ty = tok[1].to_string();
            let name = tok[2].trim_end_matches(';').to_string();
            if name.contains('[') {
                anyhow::bail!("array varying '{name}' not supported");
            }
            let loc = frag_loc.get(name.as_str()).copied().unwrap_or_else(|| {
                let l = next_free_loc;
                next_free_loc += 1;
                l
            });
            out.push_str(&format!("layout(location={loc}) out {ty} {name};\n"));
            declared_varyings.push(name);
            continue;
        }
        if tok.first() == Some(&"uniform") {
            continue; // already in the shared UBO
        }
        body.push_str(line);
        body.push('\n');
    }

    // Fragment inputs the vertex shader never declares: emit + zero-init so
    // the wgpu interface check passes.
    let mut missing_inits = String::new();
    for (ty, name) in frag_inputs {
        if !declared_varyings.iter().any(|v| v == name) {
            out.push_str(&format!(
                "layout(location={}) out {ty} {name};\n",
                frag_loc[name.as_str()]
            ));
            missing_inits.push_str(&format!("    {name} = {ty}(0.0);\n"));
        }
    }
    if !missing_inits.is_empty() {
        // Insert right after the opening brace of main().
        let main_pos = body
            .find("void main")
            .context("vertex shader has no main()")?;
        let brace = body[main_pos..]
            .find('{')
            .map(|i| main_pos + i + 1)
            .context("vertex shader main() has no body")?;
        body.insert_str(brace, &format!("\n{missing_inits}"));
    }

    out.push_str(&body);
    Ok((out, attributes))
}

// ── Synthetic vertex shader helpers ──────────────────────────────────────────

/// Extract (type, name) pairs from fragment `varying` declarations (array varyings excluded).
fn frag_inputs(frag_glsl: &str) -> Vec<(String, String)> {
    let mut inputs = Vec::new();
    for line in frag_glsl.lines() {
        let t = line.trim();
        if !t.starts_with("varying ") && !t.starts_with("attribute ") {
            continue;
        }
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.len() < 3 {
            continue;
        }
        let name_raw = t
            .split("//")
            .next()
            .unwrap_or(t)
            .split_whitespace()
            .nth(2)
            .unwrap_or("")
            .trim_end_matches(';');
        if name_raw.contains('[') {
            continue;
        } // array varyings already rejected upstream
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
        _ => true, // more than 2 varyings → vs_we_effect definitely can't match
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
    // naga's SPIR-V frontend runs with adjust_coordinate_space=true, which
    // negates gl_Position.y. UVs must be derived from the POST-flip screen
    // position, so v maps y=-1 (screen top after flip) → 0.
    out.push_str("    float v = (y + 1.0) * 0.5;\n");
    for (ty, name) in inputs {
        let val = match ty.as_str() {
            "float" => "u".to_string(),
            "vec2" => "vec2(u, v)".to_string(),
            "vec3" => "vec3(u, v, 0.0)".to_string(),
            "vec4" => "vec4(u, v, u, v)".to_string(),
            _ => format!("{ty}(0.0)"),
        };
        out.push_str(&format!("    {name} = {val};\n"));
    }
    out.push_str("}\n");
    out
}

pub fn combo_defines(combos: &HashMap<String, i32>) -> String {
    combos
        .iter()
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
            let after_ok = i + klen >= sbytes.len() || !is_word_char(sbytes[i + klen]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrolls_literal_indexed_array_varying() {
        let src = "varying vec2 v_TexCoord[4];\nvarying vec2 v_TexCoordBase;\n\
                   void main() {\n\
                   \tv_TexCoord[0] = a;\n\tv_TexCoord[1] = b;\n\
                   \tv_TexCoord[2] = c;\n\tv_TexCoord[3] = d;\n}\n";
        let out = unroll_array_varyings(src);
        assert!(out.contains("varying vec2 v_TexCoord_0;"));
        assert!(out.contains("varying vec2 v_TexCoord_1;"));
        assert!(out.contains("varying vec2 v_TexCoord_2;"));
        assert!(out.contains("varying vec2 v_TexCoord_3;"));
        assert!(out.contains("varying vec2 v_TexCoordBase;"));
        assert!(!out.contains('['));
        assert!(out.contains("v_TexCoord_0 = a;"));
        assert!(out.contains("v_TexCoord_3 = d;"));
    }

    #[test]
    fn leaves_source_without_array_varyings_untouched() {
        let src = "varying vec2 v_TexCoord;\nvoid main() { gl_FragColor = vec4(1.0); }\n";
        assert_eq!(unroll_array_varyings(src), src);
    }

    #[test]
    fn leaves_dynamically_sized_arrays_alone() {
        let src = "varying vec2 v_TexCoord[LIGHTS_POINT];\n";
        let out = unroll_array_varyings(src);
        assert_eq!(out, src);
    }

    /// blur_downsample4.frag: a *fixed*-size array (`[4]`) but indexed by a
    /// loop variable (`v_TexCoord[i]`), not literals. Unrolling only the
    /// declaration here previously left `v_TexCoord[i]` referring to a name
    /// that no longer existed ("undeclared identifier"), instead of leaving
    /// the whole thing untouched for the honest "array varying not
    /// supported" bail-out.
    #[test]
    fn leaves_loop_indexed_fixed_size_arrays_alone() {
        let src = "varying vec2 v_TexCoord[4];\n\
                   void main() {\n\
                   \tfor (int i = 0; i < 4; ++i) {\n\
                   \t\tvec4 s = texSample2D(g_Texture0, v_TexCoord[i]);\n\
                   \t}\n}\n";
        let out = unroll_array_varyings(src);
        assert_eq!(out, src);
    }

    #[test]
    fn unroll_simple_for_loops_expands_fixed_bound_loop() {
        let src = "void main() {\n\tfor (int i = 0; i < 3; ++i) {\n\t\tx += arr[i];\n\t}\n}\n";
        let out = unroll_simple_for_loops(src);
        assert!(!out.contains("for ("));
        assert!(out.contains("x += arr[0];"));
        assert!(out.contains("x += arr[1];"));
        assert!(out.contains("x += arr[2];"));
        assert!(!out.contains("arr[3]"));
    }

    #[test]
    fn unroll_simple_for_loops_handles_post_increment_and_less_equal() {
        let src = "for (int i = 0; i <= 2; i++) { y += a[i]; }";
        let out = unroll_simple_for_loops(src);
        assert!(out.contains("y += a[0];"));
        assert!(out.contains("y += a[1];"));
        assert!(out.contains("y += a[2];"));
        assert!(!out.contains("a[3]"));
    }

    #[test]
    fn unroll_simple_for_loops_leaves_non_zero_start_untouched() {
        let src = "for (int i = 1; i < 4; ++i) { x += arr[i]; }";
        assert_eq!(unroll_simple_for_loops(src), src);
    }

    #[test]
    fn unroll_simple_for_loops_leaves_non_literal_bound_untouched() {
        let src = "for (int i = 0; i < N; ++i) { x += arr[i]; }";
        assert_eq!(unroll_simple_for_loops(src), src);
    }

    #[test]
    fn unroll_simple_for_loops_leaves_mismatched_loop_var_untouched() {
        let src = "for (int i = 0; j < 4; ++i) { x += arr[i]; }";
        assert_eq!(unroll_simple_for_loops(src), src);
    }

    #[test]
    fn unroll_simple_for_loops_ignores_braces_inside_line_comments() {
        let src = "for (int i = 0; i < 2; ++i) {\n\t// a stray brace: }\n\tx += arr[i];\n}\n";
        let out = unroll_simple_for_loops(src);
        assert!(!out.contains("for ("));
        assert!(out.contains("x += arr[0];"));
        assert!(out.contains("x += arr[1];"));
    }

    /// The exact real-world shape from blur_downsample4.frag: loop-unroll
    /// then array-varying-unroll together must fully eliminate the array.
    #[test]
    fn loop_then_array_unroll_fully_resolves_real_world_pattern() {
        let src = "varying vec2 v_TexCoord[4];\n\
                   uniform sampler2D g_Texture0;\n\
                   void main() {\n\
                   \tfloat weight = 0.0;\n\
                   \tvec4 result = CAST4(0.0);\n\
                   \tfor (int i = 0; i < 4; ++i)\n\
                   \t{\n\
                   \t\tvec4 sample = texSample2D(g_Texture0, v_TexCoord[i]);\n\
                   \t\tresult += sample * sample.a;\n\
                   \t\tweight += sample.a;\n\
                   \t}\n\
                   \tgl_FragColor.rgb = result.rgb / max(0.001, weight);\n\
                   }\n";
        let loop_unrolled = unroll_simple_for_loops(src);
        let fully_unrolled = unroll_array_varyings(&loop_unrolled);
        assert!(!fully_unrolled.contains('['));
        assert!(fully_unrolled.contains("varying vec2 v_TexCoord_0;"));
        assert!(fully_unrolled.contains("varying vec2 v_TexCoord_3;"));
        assert!(fully_unrolled.contains("v_TexCoord_0"));
        assert!(fully_unrolled.contains("v_TexCoord_3"));
    }

    /// The exact real-world case (nitro.frag): `max(0, albedo.rgb)` is wrong
    /// two ways — GLSL only defines `genType max(genType, float)` (vector
    /// first), and the literal must be a genuine float. Both need fixing.
    #[test]
    fn coerce_int_literal_fixes_bare_integer_and_swaps_order_in_max_call() {
        let src = "gl_FragColor = vec4(max(0, albedo.rgb), albedo.a);";
        let out = coerce_int_literal_builtin_args(src);
        assert_eq!(out, "gl_FragColor = vec4(max(albedo.rgb, 0.0), albedo.a);");
    }

    #[test]
    fn coerce_int_literal_handles_min_and_clamp_and_negative() {
        assert_eq!(coerce_int_literal_builtin_args("min(0, x)"), "min(x, 0.0)");
        assert_eq!(
            coerce_int_literal_builtin_args("clamp(x, 0, 1)"),
            "clamp(x, 0.0, 1.0)"
        );
        assert_eq!(
            coerce_int_literal_builtin_args("max(-1, x)"),
            "max(x, -1.0)"
        );
    }

    /// A literal that's already a float still needs reordering if it's in
    /// the (invalid) first position — order matters independently of type.
    /// `clamp`'s args are never reordered (not commutative), and calls that
    /// aren't max/min/clamp at all are left alone entirely.
    #[test]
    fn coerce_int_literal_reorders_already_float_literal_and_ignores_other_calls() {
        assert_eq!(
            coerce_int_literal_builtin_args("max(0.0, x)"),
            "max(x, 0.0)"
        );
        assert_eq!(
            coerce_int_literal_builtin_args("clamp(y, a, b)"),
            "clamp(y, a, b)"
        );
        assert_eq!(coerce_int_literal_builtin_args("foo(0, 1)"), "foo(0, 1)");
    }

    #[test]
    fn coerce_int_literal_leaves_correctly_ordered_call_untouched() {
        assert_eq!(
            coerce_int_literal_builtin_args("max(albedo.rgb, 0.0)"),
            "max(albedo.rgb, 0.0)"
        );
    }

    #[test]
    fn coerce_int_literal_handles_nested_calls() {
        let src = "max(0, texture(tex, uv).rgb)";
        assert_eq!(
            coerce_int_literal_builtin_args(src),
            "max(texture(tex, uv).rgb, 0.0)"
        );
    }

    /// godrays_gaussian.frag declares the same varying at three different
    /// sizes under mutually-exclusive `#if KERNEL==N` branches; only one
    /// (max-sized) declaration block should survive, with no duplicates.
    #[test]
    fn merges_same_name_declared_at_multiple_sizes_across_if_branches() {
        let src = "#if KERNEL == 0\nvarying vec2 v_TexCoord[13];\n#endif\n\
                   #if KERNEL == 1\nvarying vec2 v_TexCoord[7];\n#endif\n\
                   #if KERNEL == 2\nvarying vec2 v_TexCoord[3];\n#endif\n\
                   void main() {\n\
                   #if KERNEL == 2\n\tvec2 a = v_TexCoord[0] + v_TexCoord[2];\n#endif\n}\n";
        let out = unroll_array_varyings(src);
        assert_eq!(out.matches("varying vec2 v_TexCoord_0;").count(), 1);
        assert_eq!(out.matches("varying vec2 v_TexCoord_12;").count(), 1);
        assert!(!out.contains("v_TexCoord_13"));
        assert!(out.contains("v_TexCoord_0 + v_TexCoord_2"));
        assert!(!out.contains('['));
    }

    #[test]
    fn coerce_truncates_unswizzled_texture_sample_to_declared_width() {
        let mut vw = HashMap::new();
        let out = coerce_vector_widths(
            "vec3 albedo = texSample2D(g_Texture0, v_TexCoord.xy);",
            &mut vw,
        );
        assert_eq!(
            out,
            "vec3 albedo = texSample2D(g_Texture0, v_TexCoord.xy).xyz;"
        );
        assert_eq!(vw.get("albedo"), Some(&3));
    }

    #[test]
    fn coerce_truncates_plain_reassignment_using_tracked_width() {
        let mut vw = HashMap::new();
        vw.insert("albedo".to_string(), 3u8);
        let out = coerce_vector_widths("albedo = texSample2D(g_Texture0, uv);", &mut vw);
        assert_eq!(out, "albedo = texSample2D(g_Texture0, uv).xyz;");
    }

    #[test]
    fn coerce_truncates_wide_varying_against_cast_broadcast() {
        let mut vw = HashMap::new();
        vw.insert("v_TexCoord".to_string(), 3u8);
        let out = coerce_vector_widths(
            "float scale = pow(length(abs(v_TexCoord - CAST2(u_offset)) * 1.0), 3.0);",
            &mut vw,
        );
        assert_eq!(
            out,
            "float scale = pow(length(abs(v_TexCoord.xy - CAST2(u_offset)) * 1.0), 3.0);"
        );
    }

    #[test]
    fn coerce_leaves_matching_widths_untouched() {
        let mut vw = HashMap::new();
        vw.insert("v_TexCoord".to_string(), 2u8);
        let line = "float scale = length(v_TexCoord - CAST2(u_offset));";
        assert_eq!(coerce_vector_widths(line, &mut vw), line);
    }

    #[test]
    fn coerce_leaves_swizzled_texture_sample_untouched() {
        let mut vw = HashMap::new();
        let line = "vec3 albedo = texSample2D(g_Texture0, uv).rgb;";
        assert_eq!(coerce_vector_widths(line, &mut vw), line);
    }

    #[test]
    fn coerce_is_idempotent_on_already_truncated_operand() {
        let mut vw = HashMap::new();
        vw.insert("v_TexCoord".to_string(), 3u8);
        let once = coerce_vector_widths(
            "float scale = length(v_TexCoord - CAST2(u_offset));",
            &mut vw,
        );
        let twice = coerce_vector_widths(&once, &mut vw);
        assert_eq!(once, twice);
    }
}
