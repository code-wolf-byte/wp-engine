//! Wallpaper Engine puppet-warp models (`.mdl`, `MDLV0021`/`MDLV0023`).
//!
//! A model JSON with a `"puppet"` field stores its texture as a packed UV
//! atlas; drawing that atlas as a plain quad shows scrambled body parts.
//! The reference (CImage.cpp `loadPuppetMesh`) extracts the rest-pose
//! triangle mesh — position + UV per vertex, skinning data ignored — and
//! renders the layer as that static mesh. Since the mesh never animates
//! there, pre-rasterizing it into an assembled RGBA image at load time is
//! visually equivalent and lets every downstream path (effects, GPU
//! compositing) keep treating the layer as a plain image.

use image::RgbaImage;

/// 9-byte version marker ("MDLV0021\0").
const MARKER_SIZE: usize = 9;
/// `[u32 unknown][u32 vertexBytes]` prefix before the vertex data.
const MESH_HEADER_SIZE: usize = 8;
/// Vertex layouts by format version: position is always 3xf32 at offset 0,
/// UV is 2xf32 at the tail, skinning data in between (ignored, like the
/// reference). MDLV0021/0023 use 80-byte vertices (UV at 72) — the only
/// versions the reference accepts; MDLV0016 (older workshop content, e.g.
/// 2952574984) uses 52-byte vertices (UV at 44), reverse-engineered from
/// real data since the reference just rejects it.
const LAYOUTS: [(usize, usize); 2] = [(80, 72), (52, 44)];

pub struct PuppetMesh {
    /// Rest-pose positions, centered: pixel = (w/2 + x, h/2 - y).
    pub positions: Vec<[f32; 2]>,
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list into `positions`/`uvs`.
    pub indices: Vec<u16>,
}

fn f32_at(data: &[u8], offset: usize) -> Option<f32> {
    Some(f32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// Parse a puppet `.mdl`. Mirrors the reference's heuristic scan
/// (CImage.cpp `findPuppetMeshBlock`): the vertex/index block has no fixed
/// offset, so every byte offset before the `MDLS` marker is tested for a
/// `[u32][u32 vertexBytes][vertices][u32 indexBytes][u16 indices]` shape
/// whose sizes divide evenly and fit.
pub fn parse_mdl(data: &[u8]) -> Option<PuppetMesh> {
    let version = data.get(..8)?;
    if !version.starts_with(b"MDLV00") {
        return None;
    }

    let mdls_offset = (MARKER_SIZE..data.len().saturating_sub(4))
        .find(|&o| &data[o..o + 4] == b"MDLS")
        .unwrap_or(data.len());

    // Try the newer layout first; the block scan is self-validating (sizes
    // must divide evenly, fit before MDLS, and every index must be in
    // range), so a wrong stride simply finds nothing.
    LAYOUTS
        .iter()
        .find_map(|&(stride, uv_offset)| parse_mesh_block(data, mdls_offset, stride, uv_offset))
}

fn parse_mesh_block(
    data: &[u8],
    mdls_offset: usize,
    stride: usize,
    uv_offset: usize,
) -> Option<PuppetMesh> {
    for offset in MARKER_SIZE..mdls_offset.saturating_sub(MESH_HEADER_SIZE + 4) {
        let Some(vertex_bytes) = u32_at(data, offset + 4).map(|v| v as usize) else {
            continue;
        };
        let vertices_offset = offset + MESH_HEADER_SIZE;
        let index_len_offset = vertices_offset + vertex_bytes;
        if vertex_bytes < stride * 3 || vertex_bytes % stride != 0 || index_len_offset + 4 > mdls_offset
        {
            continue;
        }
        let Some(index_bytes) = u32_at(data, index_len_offset).map(|v| v as usize) else {
            continue;
        };
        let indices_offset = index_len_offset + 4;
        if index_bytes == 0 || index_bytes % 6 != 0 || indices_offset + index_bytes > mdls_offset {
            continue;
        }

        let vertex_count = vertex_bytes / stride;
        let mut positions = Vec::with_capacity(vertex_count);
        let mut uvs = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let v = vertices_offset + i * stride;
            positions.push([f32_at(data, v)?, f32_at(data, v + 4)?]);
            uvs.push([f32_at(data, v + uv_offset)?, f32_at(data, v + uv_offset + 4)?]);
        }

        let index_count = index_bytes / 2;
        let mut indices = Vec::with_capacity(index_count);
        let mut valid = true;
        for i in 0..index_count {
            let o = indices_offset + i * 2;
            let idx = u16::from_le_bytes([data[o], data[o + 1]]);
            if idx as usize >= vertex_count {
                valid = false;
                break;
            }
            indices.push(idx);
        }
        if !valid {
            continue;
        }

        return Some(PuppetMesh {
            positions,
            uvs,
            indices,
        });
    }

    None
}

/// Rasterize the rest-pose mesh: assembles the packed `atlas` into a
/// `width` x `height` image via each triangle's UV mapping. Mesh positions
/// are centered, y-up (the reference's `updatePuppetPositionBuffer` maps
/// them as `(w/2 + x, h/2 - y)`); triangles composite alpha-over in index
/// order, matching the reference's draw order for overlapping parts.
pub fn rasterize(mesh: &PuppetMesh, atlas: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let mut out = RgbaImage::new(width.max(1), height.max(1));
    let (w, h) = (width as f32, height as f32);

    let to_px = |p: [f32; 2]| -> [f32; 2] { [w / 2.0 + p[0], h / 2.0 - p[1]] };

    for tri in mesh.indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let p0 = to_px(mesh.positions[i0]);
        let p1 = to_px(mesh.positions[i1]);
        let p2 = to_px(mesh.positions[i2]);
        let (uv0, uv1, uv2) = (mesh.uvs[i0], mesh.uvs[i1], mesh.uvs[i2]);

        // Signed double-area; degenerate triangles contribute nothing.
        let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
        if area.abs() < 1e-6 {
            continue;
        }

        let min_x = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as u32;
        let max_x = (p0[0].max(p1[0]).max(p2[0]).ceil() as i64).clamp(0, width as i64 - 1) as u32;
        let min_y = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as u32;
        let max_y = (p0[1].max(p1[1]).max(p2[1]).ceil() as i64).clamp(0, height as i64 - 1) as u32;

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let x = px as f32 + 0.5;
                let y = py as f32 + 0.5;
                // Barycentric weights, normalized by the signed area so
                // either winding works.
                let w0 = ((p1[0] - x) * (p2[1] - y) - (p1[1] - y) * (p2[0] - x)) / area;
                let w1 = ((p2[0] - x) * (p0[1] - y) - (p2[1] - y) * (p0[0] - x)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }

                let u = w0 * uv0[0] + w1 * uv1[0] + w2 * uv2[0];
                let v = w0 * uv0[1] + w1 * uv1[1] + w2 * uv2[1];
                let src = sample_bilinear(atlas, u, v);
                if src[3] == 0 {
                    continue;
                }

                // Alpha-over in triangle order.
                let dst = out.get_pixel_mut(px, py);
                let sa = src[3] as f32 / 255.0;
                let da = dst[3] as f32 / 255.0;
                let oa = sa + da * (1.0 - sa);
                if oa > 0.0 {
                    for c in 0..3 {
                        let sc = src[c] as f32 / 255.0;
                        let dc = dst[c] as f32 / 255.0;
                        dst[c] = (((sc * sa + dc * da * (1.0 - sa)) / oa) * 255.0) as u8;
                    }
                }
                dst[3] = (oa * 255.0) as u8;
            }
        }
    }

    out
}

fn sample_bilinear(tex: &RgbaImage, u: f32, v: f32) -> [u8; 4] {
    let (tw, th) = (tex.width(), tex.height());
    if tw == 0 || th == 0 {
        return [0; 4];
    }
    let fx = (u.clamp(0.0, 1.0) * (tw as f32 - 1.0).max(0.0)).max(0.0);
    let fy = (v.clamp(0.0, 1.0) * (th as f32 - 1.0).max(0.0)).max(0.0);
    let x0 = fx.floor() as u32;
    let y0 = fy.floor() as u32;
    let x1 = (x0 + 1).min(tw - 1);
    let y1 = (y0 + 1).min(th - 1);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);

    let p00 = tex.get_pixel(x0, y0);
    let p10 = tex.get_pixel(x1, y0);
    let p01 = tex.get_pixel(x0, y1);
    let p11 = tex.get_pixel(x1, y1);

    let mut o = [0u8; 4];
    for i in 0..4 {
        let top = p00[i] as f32 * (1.0 - tx) + p10[i] as f32 * tx;
        let bot = p01[i] as f32 * (1.0 - tx) + p11[i] as f32 * tx;
        o[i] = (top * (1.0 - ty) + bot * ty).clamp(0.0, 255.0) as u8;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal MDLV blob: marker, filler, one discoverable mesh
    /// block (3 vertices, 1 triangle), MDLS terminator.
    fn minimal_mdl(positions: [[f32; 2]; 3], uvs: [[f32; 2]; 3]) -> Vec<u8> {
        const STRIDE: usize = 80; // MDLV0021 layout, UV at 72
        const UV: usize = 72;
        let mut d = Vec::new();
        d.extend_from_slice(b"MDLV0021\0");
        d.extend_from_slice(&0u32.to_le_bytes()); // block's leading u32
        d.extend_from_slice(&(3 * STRIDE as u32).to_le_bytes());
        for i in 0..3 {
            let mut v = [0u8; STRIDE];
            v[0..4].copy_from_slice(&positions[i][0].to_le_bytes());
            v[4..8].copy_from_slice(&positions[i][1].to_le_bytes());
            v[UV..UV + 4].copy_from_slice(&uvs[i][0].to_le_bytes());
            v[UV + 4..UV + 8].copy_from_slice(&uvs[i][1].to_le_bytes());
            d.extend_from_slice(&v);
        }
        d.extend_from_slice(&6u32.to_le_bytes()); // index bytes
        for idx in [0u16, 1, 2] {
            d.extend_from_slice(&idx.to_le_bytes());
        }
        d.extend_from_slice(b"MDLS");
        d
    }

    #[test]
    fn parses_minimal_mdl() {
        let data = minimal_mdl(
            [[-10.0, -10.0], [10.0, -10.0], [0.0, 10.0]],
            [[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]],
        );
        let mesh = parse_mdl(&data).expect("should parse");
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.uvs[2], [0.5, 0.0]);
    }

    #[test]
    fn rejects_wrong_marker() {
        assert!(parse_mdl(b"NOTAMODEL blah blah blah").is_none());
    }

    /// A triangle covering the canvas center must paint atlas content
    /// there and leave the corners (outside the mesh) transparent — the
    /// exact difference between mesh rendering and the scrambled-atlas
    /// quad draw this replaces.
    #[test]
    fn rasterizes_triangle_with_uv_sampling() {
        let data = minimal_mdl(
            [[-20.0, -20.0], [20.0, -20.0], [0.0, 20.0]],
            [[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]],
        );
        let mesh = parse_mdl(&data).unwrap();
        let atlas = RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 30, 255]));
        let out = rasterize(&mesh, &atlas, 50, 50);
        assert_eq!(out.get_pixel(25, 25).0, [10, 200, 30, 255]);
        assert_eq!(out.get_pixel(0, 0).0[3], 0, "corner must stay transparent");
        assert_eq!(out.get_pixel(49, 0).0[3], 0);
    }
}
