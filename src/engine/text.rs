//! Text object rendering (`CText.cpp` port).
//!
//! Matches the reference's own stated "Phase 1" scope: a single line of text
//! (no wrapping/padding), rasterized once from a system or wallpaper-bundled
//! TrueType font. We have no JS scripting engine, so scripted/dynamic text
//! renders whatever its initial value is and stays static — the reference's
//! own live-update path (`ScriptEngine::tickLayer`) has no equivalent here.
//!
//! Glyphs are rasterized in white with coverage as alpha; the object's own
//! `color`/`alpha` tint is applied later by the normal image-layer compositing
//! path (`Layer::color`/`Layer::alpha`), not baked in here — this lets a text
//! object reuse 100% of the existing image-layer machinery in both render
//! paths once it's turned into an `RgbaImage`.

use image::RgbaImage;

/// Rasterize a single line of `text` at `point_size` pixels using `font_data`.
/// Returns `None` for empty text (nothing to draw).
pub fn rasterize(font_data: &[u8], text: &str, point_size: f32) -> Option<RgbaImage> {
    if text.is_empty() {
        return None;
    }
    let font = fontdue::Font::from_bytes(font_data, fontdue::FontSettings::default()).ok()?;

    struct Glyph {
        metrics: fontdue::Metrics,
        bitmap: Vec<u8>,
        pen_x: f32,
    }

    let mut glyphs = Vec::with_capacity(text.chars().count());
    let mut pen_x: f32 = 0.0;
    let mut max_ascent: i32 = 1;
    let mut max_descent: i32 = 0;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, point_size);
        max_ascent = max_ascent.max(metrics.ymin + metrics.height as i32);
        max_descent = max_descent.max(-metrics.ymin);
        glyphs.push(Glyph {
            metrics,
            bitmap,
            pen_x,
        });
        pen_x += metrics.advance_width;
    }

    let width = (pen_x.ceil() as u32).max(1);
    let height = ((max_ascent + max_descent).max(1)) as u32;
    let mut img = RgbaImage::new(width, height);

    for g in &glyphs {
        let dst_x0 = g.pen_x as i32 + g.metrics.xmin;
        let dst_y0 = max_ascent - (g.metrics.ymin + g.metrics.height as i32);
        for row in 0..g.metrics.height {
            for col in 0..g.metrics.width {
                let coverage = g.bitmap[row * g.metrics.width + col];
                if coverage == 0 {
                    continue;
                }
                let dx = dst_x0 + col as i32;
                let dy = dst_y0 + row as i32;
                if dx < 0 || dy < 0 || dx as u32 >= width || dy as u32 >= height {
                    continue;
                }
                // "over" onto a transparent canvas: glyphs from neighboring
                // chars essentially never overlap for a normal font/spacing,
                // but max (not overwrite) avoids any anti-aliased seam if they
                // do (e.g. slightly negative advance/kerning-like fonts).
                let dst = img.get_pixel_mut(dx as u32, dy as u32);
                if coverage > dst[3] {
                    *dst = image::Rgba([255, 255, 255, coverage]);
                }
            }
        }
    }
    Some(img)
}

/// Resolve font bytes for a text object's `font` field: a wallpaper-bundled
/// path (checked in the loose directory, then the pkg) if given and not a
/// `"systemfont_*"` placeholder, else a system font.
pub fn resolve_font_data(
    font_ref: Option<&str>,
    dir: Option<&std::path::Path>,
    pkg: Option<&super::pkg::Package>,
) -> Option<Vec<u8>> {
    if let Some(font_ref) = font_ref {
        if !font_ref.is_empty() && !font_ref.starts_with("systemfont_") {
            if let Some(dir) = dir {
                if let Ok(data) = std::fs::read(dir.join(font_ref)) {
                    return Some(data);
                }
            }
            if let Some(pkg) = pkg {
                if let Some(data) = pkg.get(font_ref) {
                    return Some(data.to_vec());
                }
            }
        }
    }
    find_system_font()
}

/// `fc-match` (fontconfig) works across distros without hardcoding paths;
/// the hardcoded list is a fallback for systems without it installed.
fn find_system_font() -> Option<Vec<u8>> {
    if let Ok(output) = std::process::Command::new("fc-match")
        .arg("--format=%{file}")
        .arg("sans-serif")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                if let Ok(data) = std::fs::read(&path) {
                    return Some(data);
                }
            }
        }
    }

    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/liberation-fonts/LiberationSans-Regular.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(data) = std::fs::read(path) {
            return Some(data);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> Vec<u8> {
        find_system_font().expect("a system font must be available to test rasterization")
    }

    #[test]
    fn rasterize_empty_text_returns_none() {
        assert!(rasterize(&test_font(), "", 32.0).is_none());
    }

    #[test]
    fn rasterize_produces_nonempty_bitmap_with_visible_glyphs() {
        let img = rasterize(&test_font(), "Hi", 32.0).expect("should rasterize");
        assert!(img.width() > 0 && img.height() > 0);
        let has_visible_pixel = img.pixels().any(|p| p.0[3] > 0);
        assert!(
            has_visible_pixel,
            "expected at least one non-transparent pixel"
        );
    }

    #[test]
    fn rasterize_glyphs_are_white_with_coverage_alpha() {
        let img = rasterize(&test_font(), "I", 48.0).expect("should rasterize");
        for p in img.pixels() {
            if p.0[3] > 0 {
                assert_eq!([p.0[0], p.0[1], p.0[2]], [255, 255, 255]);
            }
        }
    }

    #[test]
    fn longer_text_produces_wider_bitmap() {
        let short = rasterize(&test_font(), "I", 32.0).unwrap();
        let long = rasterize(&test_font(), "IIIIIIIIII", 32.0).unwrap();
        assert!(long.width() > short.width());
    }
}
