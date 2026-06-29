use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::io::{Cursor, Read};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TexFormat {
    Rgba8,
    R8,
    Rg88,
    Rgb888,
    Rgb565,
    Dxt1,
    Dxt3,
    Dxt5,
    Bc4,
    Bc7,
}

impl TexFormat {
    fn from_u32(v: u32) -> Result<Self> {
        match v {
            0 => Ok(Self::Rgba8),   // ARGB8888 (stored BGRA — swap in to_rgba)
            1 => Ok(Self::Rgb888),  // RGB888
            2 => Ok(Self::Rgb565),  // RGB565
            4 => Ok(Self::Dxt5),   // DXT5
            6 => Ok(Self::Dxt3),   // DXT3
            7 => Ok(Self::Dxt1),   // DXT1
            8 => Ok(Self::Rg88),   // RG88
            9 => Ok(Self::R8),     // R8
            12 => Ok(Self::Bc7),   // BC7
            other => bail!("unknown .tex format id: {other}"),
        }
    }

    fn block_size(self) -> usize {
        match self {
            Self::Dxt1 | Self::Bc4 => 8,
            Self::Dxt3 | Self::Dxt5 | Self::Bc7 => 16,
            Self::Rgba8 | Self::R8 | Self::Rg88 | Self::Rgb888 | Self::Rgb565 => 0,
        }
    }
}

struct Mipmap {
    uncompressed_size: u32,
    data: Vec<u8>,
}

pub struct TexFile {
    pub format: TexFormat,
    pub image_width: u32,
    pub image_height: u32,
    pub texture_width: u32,
    pub texture_height: u32,
    mipmaps: Vec<Mipmap>,
}

impl TexFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        // Check for embedded PNG/JPEG first (fastest path).
        let embedded_offset = find_marker(data, b"\x89PNG")
            .or_else(|| find_marker(data, b"\xFF\xD8\xFF"));
        if let Some(img_offset) = embedded_offset {
            if img_offset > 20 {
                let img_data = &data[img_offset..];
                if let Ok(img) = image::load_from_memory(img_data) {
                    let img = img.into_rgba8();
                    return Ok(Self {
                        format: TexFormat::Rgba8,
                        image_width: img.width(),
                        image_height: img.height(),
                        texture_width: img.width(),
                        texture_height: img.height(),
                        mipmaps: vec![Mipmap {
                            uncompressed_size: img.as_raw().len() as u32,
                            data: img.into_raw(),
                        }],
                    });
                }
            }
        }

        let mut cur = Cursor::new(data);

        let mut magic = [0u8; 4];
        cur.read_exact(&mut magic).context("reading TEXV magic")?;
        if &magic != b"TEXV" {
            bail!("not a .tex file: expected TEXV, got {:?}", magic);
        }

        let mut version = [0u8; 4];
        cur.read_exact(&mut version).context("reading TEXV version")?;
        skip_null(&mut cur);

        // Check for TEXI section (V0005 format).
        let mut next_section = [0u8; 4];
        cur.read_exact(&mut next_section).context("reading section")?;

        let texi_meta = if &next_section == b"TEXI" {
            let mut texi_ver = [0u8; 4];
            cur.read_exact(&mut texi_ver).ok();
            skip_null(&mut cur);

            let format_id = read_u32(&mut cur)?;
            let _flags = read_u32(&mut cur)?;
            let texture_width = read_u32(&mut cur)?;
            let texture_height = read_u32(&mut cur)?;
            let image_width = read_u32(&mut cur)?;
            let image_height = read_u32(&mut cur)?;

            let texb_pos = find_marker(data, b"TEXB")
                .context("TEXB section not found after TEXI")?;
            cur.set_position(texb_pos as u64);
            cur.read_exact(&mut next_section).context("reading TEXB")?;

            Some((format_id, texture_width, texture_height, image_width, image_height))
        } else {
            None
        };

        if &next_section != b"TEXB" {
            bail!("expected TEXB section, got {:?}", next_section);
        }

        let mut container_ver = [0u8; 4];
        cur.read_exact(&mut container_ver).context("reading TEXB version")?;
        skip_null(&mut cur);

        if let Some((format_id, texture_width, texture_height, image_width, image_height)) = texi_meta {
            let mipmap_count = match &container_ver {
                b"0001" | b"0002" => {
                    let _image_count = read_u32(&mut cur)?;
                    let mipmap_count = read_u32(&mut cur)?;
                    let _tw = read_u32(&mut cur)?;
                    let _th = read_u32(&mut cur)?;
                    let _unk = read_u32(&mut cur)?;
                    mipmap_count
                }
                b"0003" => {
                    let _image_count = read_u32(&mut cur)?;
                    let _flags = read_u32(&mut cur)?;
                    let mipmap_count = read_u32(&mut cur)?;
                    let _tw = read_u32(&mut cur)?;
                    let _th = read_u32(&mut cur)?;
                    let _unk = read_u32(&mut cur)?;
                    mipmap_count
                }
                b"0004" => {
                    let _free_image_format = read_u32(&mut cur)?;
                    let _is_video_mp4 = read_u32(&mut cur)?;
                    let _image_count = read_u32(&mut cur)?;
                    let _flags = read_u32(&mut cur)?;
                    let mipmap_count = read_u32(&mut cur)?;
                    let _tw = read_u32(&mut cur)?;
                    let _th = read_u32(&mut cur)?;
                    let _unk = read_u32(&mut cur)?;
                    mipmap_count
                }
                _ => {
                    let _image_count = read_u32(&mut cur)?;
                    let _flags = read_u32(&mut cur)?;
                    let _unk0 = read_u32(&mut cur)?;
                    let mipmap_count = read_u32(&mut cur)?;
                    let _tw = read_u32(&mut cur)?;
                    let _th = read_u32(&mut cur)?;
                    let _unk1 = read_u32(&mut cur)?;
                    mipmap_count
                }
            };

            let format = TexFormat::from_u32(format_id)?;
            let mipmaps = read_mipmaps_v5(&mut cur, mipmap_count)?;

            return Ok(Self { format, image_width, image_height, texture_width, texture_height, mipmaps });
        }

        // V0007/V0008: standard TEXB layout
        let format_id = read_u32(&mut cur)?;
        let _flags = read_u32(&mut cur)?;
        let texture_width = read_u32(&mut cur)?;
        let texture_height = read_u32(&mut cur)?;
        let _unknown1 = read_u32(&mut cur)?;
        let image_width = read_u32(&mut cur)?;
        let image_height = read_u32(&mut cur)?;
        let _unknown2 = read_u32(&mut cur)?;

        let format = TexFormat::from_u32(format_id)?;
        let mipmap_count = read_u32(&mut cur)?;
        let mipmaps = read_mipmaps(&mut cur, mipmap_count)?;

        Ok(Self { format, image_width, image_height, texture_width, texture_height, mipmaps })
    }

    pub fn to_rgba(&self) -> Result<RgbaImage> {
        let mip = self.mipmaps.first().context("no mipmaps in .tex file")?;

        let rgba = match self.format {
            // WE stores ARGB8888 as BGRA in memory; swap R and B to produce RGBA.
            TexFormat::Rgba8 => mip.data.chunks_exact(4)
                .flat_map(|bgra| [bgra[2], bgra[1], bgra[0], bgra[3]])
                .collect(),
            TexFormat::R8 => {
                mip.data.iter().flat_map(|&r| [r, r, r, 255]).collect()
            }
            TexFormat::Rg88 => {
                mip.data.chunks(2).flat_map(|rg| {
                    let r = rg.first().copied().unwrap_or(0);
                    let g = rg.get(1).copied().unwrap_or(0);
                    [r, g, 0, 255]
                }).collect()
            }
            TexFormat::Rgb888 => {
                mip.data.chunks(3).flat_map(|rgb| {
                    let r = rgb.first().copied().unwrap_or(0);
                    let g = rgb.get(1).copied().unwrap_or(0);
                    let b = rgb.get(2).copied().unwrap_or(0);
                    [r, g, b, 255]
                }).collect()
            }
            TexFormat::Rgb565 => mip.data.chunks_exact(2)
                .flat_map(|b| rgb565_to_rgba(u16::from_le_bytes([b[0], b[1]])))
                .collect(),
            TexFormat::Dxt1 => decode_dxt1(&mip.data, self.texture_width, self.texture_height),
            TexFormat::Dxt3 => decode_dxt3(&mip.data, self.texture_width, self.texture_height),
            TexFormat::Dxt5 => decode_dxt5(&mip.data, self.texture_width, self.texture_height),
            TexFormat::Bc4 => decode_bc4(&mip.data, self.texture_width, self.texture_height),
            TexFormat::Bc7 => decode_bc7(&mip.data, self.texture_width, self.texture_height),
        };

        let img = RgbaImage::from_raw(self.texture_width, self.texture_height, rgba)
            .context("failed to create RgbaImage from decoded texture data")?;

        if self.image_width != self.texture_width || self.image_height != self.texture_height {
            Ok(image::imageops::crop_imm(&img, 0, 0, self.image_width, self.image_height)
                .to_image())
        } else {
            Ok(img)
        }
    }

    /// Decode every mipmap as a separate RGBA image (animation frame).
    /// WE stores sprite animation frames as sequential mipmaps at the same resolution.
    /// Falls back to a single frame (same as `to_rgba`) for non-animated textures.
    pub fn to_rgba_frames(&self) -> Result<Vec<RgbaImage>> {
        let mut frames = Vec::with_capacity(self.mipmaps.len());
        for mip in &self.mipmaps {
            let rgba: Vec<u8> = match self.format {
                TexFormat::Rgba8 => mip.data.chunks_exact(4)
                    .flat_map(|bgra| [bgra[2], bgra[1], bgra[0], bgra[3]])
                    .collect(),
                TexFormat::R8 => mip.data.iter().flat_map(|&r| [r, r, r, 255]).collect(),
                TexFormat::Rg88 => mip.data.chunks(2).flat_map(|rg| {
                    let r = rg.first().copied().unwrap_or(0);
                    let g = rg.get(1).copied().unwrap_or(0);
                    [r, g, 0, 255]
                }).collect(),
                TexFormat::Rgb888 => mip.data.chunks(3).flat_map(|rgb| {
                    [rgb.first().copied().unwrap_or(0),
                     rgb.get(1).copied().unwrap_or(0),
                     rgb.get(2).copied().unwrap_or(0), 255]
                }).collect(),
                TexFormat::Rgb565 => mip.data.chunks_exact(2)
                    .flat_map(|b| rgb565_to_rgba(u16::from_le_bytes([b[0], b[1]])))
                    .collect(),
                TexFormat::Dxt1 => decode_dxt1(&mip.data, self.texture_width, self.texture_height),
                TexFormat::Dxt3 => decode_dxt3(&mip.data, self.texture_width, self.texture_height),
                TexFormat::Dxt5 => decode_dxt5(&mip.data, self.texture_width, self.texture_height),
                TexFormat::Bc4 => decode_bc4(&mip.data, self.texture_width, self.texture_height),
                TexFormat::Bc7 => decode_bc7(&mip.data, self.texture_width, self.texture_height),
            };
            if let Some(img) = RgbaImage::from_raw(self.texture_width, self.texture_height, rgba) {
                let cropped = if self.image_width != self.texture_width || self.image_height != self.texture_height {
                    image::imageops::crop_imm(&img, 0, 0, self.image_width, self.image_height).to_image()
                } else {
                    img
                };
                frames.push(cropped);
            }
        }
        if frames.is_empty() {
            anyhow::bail!("no decodable frames in .tex file");
        }
        Ok(frames)
    }
}

// ── DXT decoders ──────────────────────────────────────────────────────────────

fn rgb565_to_rgba(c: u16) -> [u8; 4] {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
        255,
    ]
}

fn lerp_rgb(a: [u8; 4], b: [u8; 4], num_a: u16, num_b: u16, den: u16) -> [u8; 4] {
    [
        ((a[0] as u16 * num_a + b[0] as u16 * num_b) / den) as u8,
        ((a[1] as u16 * num_a + b[1] as u16 * num_b) / den) as u8,
        ((a[2] as u16 * num_a + b[2] as u16 * num_b) / den) as u8,
        255,
    ]
}

fn decode_dxt1(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bw = ((width + 3) / 4) as usize;
    let bh = ((height + 3) / 4) as usize;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let stride = width as usize * 4;

    for by in 0..bh {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 8;
            if offset + 8 > data.len() {
                break;
            }
            let block = &data[offset..offset + 8];

            let c0 = u16::from_le_bytes([block[0], block[1]]);
            let c1 = u16::from_le_bytes([block[2], block[3]]);
            let lut = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);

            let p0 = rgb565_to_rgba(c0);
            let p1 = rgb565_to_rgba(c1);

            let palette = if c0 > c1 {
                [p0, p1, lerp_rgb(p0, p1, 2, 1, 3), lerp_rgb(p0, p1, 1, 2, 3)]
            } else {
                [p0, p1, lerp_rgb(p0, p1, 1, 1, 2), [0, 0, 0, 0]]
            };

            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bx as u32 * 4 + px;
                    let y = by as u32 * 4 + py;
                    if x >= width || y >= height {
                        continue;
                    }
                    let idx = ((lut >> (2 * (py * 4 + px))) & 3) as usize;
                    let dst = y as usize * stride + x as usize * 4;
                    out[dst..dst + 4].copy_from_slice(&palette[idx]);
                }
            }
        }
    }
    out
}

fn decode_dxt3(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bw = ((width + 3) / 4) as usize;
    let bh = ((height + 3) / 4) as usize;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let stride = width as usize * 4;

    for by in 0..bh {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 16;
            if offset + 16 > data.len() {
                break;
            }
            let block = &data[offset..offset + 16];

            // First 8 bytes: explicit 4-bit alpha for each pixel
            let alpha_bits = u64::from_le_bytes(block[0..8].try_into().unwrap());

            // Next 8 bytes: DXT1 color block
            let c0 = u16::from_le_bytes([block[8], block[9]]);
            let c1 = u16::from_le_bytes([block[10], block[11]]);
            let lut = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

            let p0 = rgb565_to_rgba(c0);
            let p1 = rgb565_to_rgba(c1);
            let palette = [
                p0,
                p1,
                lerp_rgb(p0, p1, 2, 1, 3),
                lerp_rgb(p0, p1, 1, 2, 3),
            ];

            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bx as u32 * 4 + px;
                    let y = by as u32 * 4 + py;
                    if x >= width || y >= height {
                        continue;
                    }
                    let idx = ((lut >> (2 * (py * 4 + px))) & 3) as usize;
                    let pixel_index = py * 4 + px;
                    let a4 = ((alpha_bits >> (pixel_index * 4)) & 0xF) as u8;
                    let alpha = a4 | (a4 << 4);

                    let dst = y as usize * stride + x as usize * 4;
                    out[dst] = palette[idx][0];
                    out[dst + 1] = palette[idx][1];
                    out[dst + 2] = palette[idx][2];
                    out[dst + 3] = alpha;
                }
            }
        }
    }
    out
}

fn decode_dxt5(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bw = ((width + 3) / 4) as usize;
    let bh = ((height + 3) / 4) as usize;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let stride = width as usize * 4;

    for by in 0..bh {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 16;
            if offset + 16 > data.len() {
                break;
            }
            let block = &data[offset..offset + 16];

            // Alpha block: 2 reference alphas + 6 bytes of 3-bit indices
            let a0 = block[0];
            let a1 = block[1];
            let alpha_bits = u64::from(block[2]) as u64
                | (u64::from(block[3]) << 8)
                | (u64::from(block[4]) << 16)
                | (u64::from(block[5]) << 24)
                | (u64::from(block[6]) << 32)
                | (u64::from(block[7]) << 40);

            let alpha_palette = if a0 > a1 {
                [
                    a0,
                    a1,
                    ((6 * a0 as u16 + 1 * a1 as u16) / 7) as u8,
                    ((5 * a0 as u16 + 2 * a1 as u16) / 7) as u8,
                    ((4 * a0 as u16 + 3 * a1 as u16) / 7) as u8,
                    ((3 * a0 as u16 + 4 * a1 as u16) / 7) as u8,
                    ((2 * a0 as u16 + 5 * a1 as u16) / 7) as u8,
                    ((1 * a0 as u16 + 6 * a1 as u16) / 7) as u8,
                ]
            } else {
                [
                    a0,
                    a1,
                    ((4 * a0 as u16 + 1 * a1 as u16) / 5) as u8,
                    ((3 * a0 as u16 + 2 * a1 as u16) / 5) as u8,
                    ((2 * a0 as u16 + 3 * a1 as u16) / 5) as u8,
                    ((1 * a0 as u16 + 4 * a1 as u16) / 5) as u8,
                    0,
                    255,
                ]
            };

            // Color block (same as DXT1)
            let c0 = u16::from_le_bytes([block[8], block[9]]);
            let c1 = u16::from_le_bytes([block[10], block[11]]);
            let lut = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

            let p0 = rgb565_to_rgba(c0);
            let p1 = rgb565_to_rgba(c1);
            let palette = [
                p0,
                p1,
                lerp_rgb(p0, p1, 2, 1, 3),
                lerp_rgb(p0, p1, 1, 2, 3),
            ];

            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bx as u32 * 4 + px;
                    let y = by as u32 * 4 + py;
                    if x >= width || y >= height {
                        continue;
                    }
                    let cidx = ((lut >> (2 * (py * 4 + px))) & 3) as usize;
                    let pixel_index = py * 4 + px;
                    let aidx = ((alpha_bits >> (pixel_index * 3)) & 7) as usize;

                    let dst = y as usize * stride + x as usize * 4;
                    out[dst] = palette[cidx][0];
                    out[dst + 1] = palette[cidx][1];
                    out[dst + 2] = palette[cidx][2];
                    out[dst + 3] = alpha_palette[aidx];
                }
            }
        }
    }
    out
}

fn decode_bc4(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bw = ((width + 3) / 4) as usize;
    let bh = ((height + 3) / 4) as usize;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let stride = width as usize * 4;

    for by in 0..bh {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 8;
            if offset + 8 > data.len() {
                break;
            }
            let block = &data[offset..offset + 8];

            let a0 = block[0];
            let a1 = block[1];
            let alpha_bits = u64::from(block[2])
                | (u64::from(block[3]) << 8)
                | (u64::from(block[4]) << 16)
                | (u64::from(block[5]) << 24)
                | (u64::from(block[6]) << 32)
                | (u64::from(block[7]) << 40);

            let palette = if a0 > a1 {
                [
                    a0, a1,
                    ((6 * a0 as u16 + 1 * a1 as u16) / 7) as u8,
                    ((5 * a0 as u16 + 2 * a1 as u16) / 7) as u8,
                    ((4 * a0 as u16 + 3 * a1 as u16) / 7) as u8,
                    ((3 * a0 as u16 + 4 * a1 as u16) / 7) as u8,
                    ((2 * a0 as u16 + 5 * a1 as u16) / 7) as u8,
                    ((1 * a0 as u16 + 6 * a1 as u16) / 7) as u8,
                ]
            } else {
                [
                    a0, a1,
                    ((4 * a0 as u16 + 1 * a1 as u16) / 5) as u8,
                    ((3 * a0 as u16 + 2 * a1 as u16) / 5) as u8,
                    ((2 * a0 as u16 + 3 * a1 as u16) / 5) as u8,
                    ((1 * a0 as u16 + 4 * a1 as u16) / 5) as u8,
                    0, 255,
                ]
            };

            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bx as u32 * 4 + px;
                    let y = by as u32 * 4 + py;
                    if x >= width || y >= height {
                        continue;
                    }
                    let idx = ((alpha_bits >> ((py * 4 + px) * 3)) & 7) as usize;
                    let v = palette[idx];
                    let dst = y as usize * stride + x as usize * 4;
                    out[dst] = v;
                    out[dst + 1] = v;
                    out[dst + 2] = v;
                    out[dst + 3] = 255;
                }
            }
        }
    }
    out
}

fn decode_bc7(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bw = ((width + 3) / 4) as usize;
    let bh = ((height + 3) / 4) as usize;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let stride = width as usize * 4;

    for by in 0..bh {
        for bx in 0..bw {
            let offset = (by * bw + bx) * 16;
            if offset + 16 > data.len() {
                break;
            }
            let block = &data[offset..offset + 16];

            let mode = block[0].trailing_zeros();
            let rgba = if mode == 6 {
                decode_bc7_mode6(block)
            } else {
                [[128, 128, 128, 255]; 16]
            };

            for py in 0..4u32 {
                for px in 0..4u32 {
                    let x = bx as u32 * 4 + px;
                    let y = by as u32 * 4 + py;
                    if x >= width || y >= height {
                        continue;
                    }
                    let src = (py * 4 + px) as usize;
                    let dst = y as usize * stride + x as usize * 4;
                    out[dst..dst + 4].copy_from_slice(&rgba[src]);
                }
            }
        }
    }
    out
}

fn decode_bc7_mode6(block: &[u8]) -> [[u8; 4]; 16] {
    const WEIGHTS: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

    let bits = u128::from_le_bytes(block[0..16].try_into().unwrap());
    let mut pos = 7; // skip mode bits (bit 6 set)

    let mut endpoints = [[0u8; 4]; 2];
    for ch in 0..4 {
        for ep in 0..2 {
            endpoints[ep][ch] = ((bits >> pos) & 0x7F) as u8;
            pos += 7;
        }
    }
    let p0 = ((bits >> pos) & 1) as u8; pos += 1;
    let p1 = ((bits >> pos) & 1) as u8; pos += 1;

    for ch in 0..4 {
        endpoints[0][ch] = (endpoints[0][ch] << 1) | p0;
        endpoints[1][ch] = (endpoints[1][ch] << 1) | p1;
    }

    let mut result = [[0u8; 4]; 16];
    for i in 0..16 {
        let nbits = if i == 0 { 3 } else { 4 };
        let idx = ((bits >> pos) & ((1 << nbits) - 1)) as usize;
        pos += nbits;
        let w = WEIGHTS[idx];
        for ch in 0..4 {
            let e0 = endpoints[0][ch] as u32;
            let e1 = endpoints[1][ch] as u32;
            result[i][ch] = (((64 - w) * e0 + w * e1 + 32) >> 6) as u8;
        }
    }
    result
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn read_mipmaps_v5(cur: &mut Cursor<&[u8]>, count: u32) -> Result<Vec<Mipmap>> {
    let mut mipmaps = Vec::with_capacity(count as usize);
    for i in 0..count {
        if i > 0 {
            let _width = read_u32(cur)?;
            let _height = read_u32(cur)?;
            let _unk = read_u32(cur)?;
        }
        let uncompressed_size = read_u32(cur)
            .with_context(|| format!("reading mipmap {i} uncompressed_size"))?;
        let compressed_size = read_u32(cur)
            .with_context(|| format!("reading mipmap {i} compressed_size"))?;

        if uncompressed_size == 0 {
            bail!("embedded video texture (not a static image)");
        }

        let mut compressed = vec![0u8; compressed_size as usize];
        cur.read_exact(&mut compressed)
            .with_context(|| format!("reading mipmap {i} data ({compressed_size} bytes)"))?;

        let decompressed = if compressed_size == uncompressed_size {
            compressed
        } else {
            lz4_flex::decompress(&compressed, uncompressed_size as usize)
                .with_context(|| format!("LZ4 decompress mipmap {i}"))?
        };

        mipmaps.push(Mipmap { uncompressed_size, data: decompressed });
    }
    Ok(mipmaps)
}

fn read_mipmaps(cur: &mut Cursor<&[u8]>, count: u32) -> Result<Vec<Mipmap>> {
    let mut mipmaps = Vec::with_capacity(count as usize);
    for i in 0..count {
        let compressed_size = read_u32(cur)
            .with_context(|| format!("reading mipmap {i} compressed_size"))?;
        let uncompressed_size = read_u32(cur)
            .with_context(|| format!("reading mipmap {i} uncompressed_size"))?;

        let mut compressed = vec![0u8; compressed_size as usize];
        cur.read_exact(&mut compressed)
            .with_context(|| format!("reading mipmap {i} data ({compressed_size} bytes)"))?;

        let decompressed = if compressed_size == uncompressed_size {
            compressed
        } else {
            lz4_flex::decompress(&compressed, uncompressed_size as usize)
                .with_context(|| format!("LZ4 decompress mipmap {i}"))?
        };

        mipmaps.push(Mipmap { uncompressed_size, data: decompressed });
    }
    Ok(mipmaps)
}

fn read_u32(cur: &mut Cursor<&[u8]>) -> Result<u32> {
    let mut buf = [0u8; 4];
    cur.read_exact(&mut buf).context("unexpected EOF reading u32")?;
    Ok(u32::from_le_bytes(buf))
}

fn skip_null(cur: &mut Cursor<&[u8]>) {
    let pos = cur.position() as usize;
    let data = cur.get_ref();
    if pos < data.len() && data[pos] == 0 {
        cur.set_position((pos + 1) as u64);
    }
}

fn find_marker(data: &[u8], marker: &[u8]) -> Option<usize> {
    data.windows(marker.len()).position(|w| w == marker)
}
