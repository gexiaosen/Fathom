//! Procedural PBR surface maps for the 3D pass.
//!
//! Two texture arrays are baked at start-up so the app stays asset-free:
//! a normal map for relief (plaster trowel marks, wood pores, fabric weave,
//! mineral chips) and a linear albedo/roughness map whose alpha channel
//! carries the material's micro-roughness. Both are tileable and mipmapped,
//! so close-ups get grain without distant shimmer.

const SIZE: usize = 256;
const LAYERS: usize = 5;
const ALBEDO_LAYERS: usize = 8;
const MIPS: u32 = 9; // 256 → 1

pub struct SurfaceDetail {
    pub view: wgpu::TextureView,
    pub albedo_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub fn create(device: &wgpu::Device, queue: &wgpu::Queue) -> SurfaceDetail {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d surface detail"),
        size: wgpu::Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: LAYERS as u32,
        },
        mip_level_count: MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let mut heights: Vec<Vec<f32>> = (0..LAYERS).map(|layer| height_map(layer, SIZE)).collect();
    for level in 0..MIPS {
        let size = SIZE >> level;
        for layer in 0..LAYERS {
            if level > 0 {
                heights[layer] = downsample(&heights[layer], size * 2);
            }
            upload(queue, &texture, layer as u32, level, size, &heights[layer]);
        }
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });

    // Linear albedo/roughness atlas. RGB multiplies the entity colour; alpha
    // is the micro-roughness used to break up the flat scalar fallback.
    let albedo = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d pbr albedo"),
        size: wgpu::Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: ALBEDO_LAYERS as u32,
        },
        mip_level_count: MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut albedo_maps: Vec<Vec<[f32; 4]>> = (0..ALBEDO_LAYERS)
        .map(|layer| albedo_map(layer, SIZE))
        .collect();
    for level in 0..MIPS {
        let size = SIZE >> level;
        for layer in 0..ALBEDO_LAYERS {
            if level > 0 {
                albedo_maps[layer] = downsample_rgba(&albedo_maps[layer], size * 2);
            }
            upload_albedo(queue, &albedo, layer as u32, level, size, &albedo_maps[layer]);
        }
    }
    let albedo_view = albedo.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene3d surface detail sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 4,
        ..Default::default()
    });
    SurfaceDetail {
        view,
        albedo_view,
        sampler,
    }
}

/// Tileable linear albedo + roughness for each PBR family.
///
/// Layer order is stable and mirrored by `pbr_layer()` in the WGSL shader:
/// plaster, wood, marble, fabric/carpet, stone/terrazzo, tile, shingle,
/// wallpaper.
fn albedo_map(layer: usize, size: usize) -> Vec<[f32; 4]> {
    let mut out = vec![[1.0, 1.0, 1.0, 0.95]; size * size];
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let n = noise(u, v, 5, 101 + layer as u32);
            let fine = hash01(x as u32, y as u32, 211 + layer as u32);
            let (r, g, b, rough) = match layer {
                0 => {
                    let tone = 0.94 + (n - 0.5) * 0.10 + (fine - 0.5) * 0.035;
                    (tone, tone, tone * 0.99, 0.93 + (n - 0.5) * 0.08)
                }
                1 => {
                    let grain = wood(u, v, x, y);
                    let tone = 0.88 + grain * 0.42 + (n - 0.5) * 0.05;
                    (tone, tone * 0.88, tone * 0.71, 0.62 + grain.abs() * 0.08)
                }
                2 => {
                    let vein = (std::f32::consts::TAU * (2.0 * u + 1.7 * v)).sin()
                        + (std::f32::consts::TAU * (5.0 * u - 3.0 * v)).sin() * 0.4;
                    let tone = 0.94 - vein.abs().min(1.0) * 0.05 + (fine - 0.5) * 0.025;
                    (tone, tone, tone * 0.98, 0.36 + (n - 0.5) * 0.08)
                }
                3 => {
                    let tone = 0.88 + fabric(u, v) * 0.50 + (n - 0.5) * 0.05;
                    (tone, tone * 0.98, tone * 0.94, 0.96 + (fine - 0.5) * 0.025)
                }
                4 => {
                    let chip = hash01(x as u32 / 3, y as u32 / 3, 19);
                    let tone = 0.86 + chip * 0.12 + (n - 0.5) * 0.05;
                    (tone, tone, tone * 0.97, 0.55 + (n - 0.5) * 0.10)
                }
                5 => {
                    let tone = 0.94 + (n - 0.5) * 0.045 + (fine - 0.5) * 0.025;
                    (tone, tone, tone, 0.38 + (n - 0.5) * 0.06)
                }
                6 => {
                    let chip = hash01(x as u32 / 2, y as u32 / 2, 41);
                    let tone = 0.72 + chip * 0.14 + (n - 0.5) * 0.05;
                    (tone, tone * 0.98, tone * 0.94, 0.93 + (fine - 0.5) * 0.04)
                }
                _ => {
                    let rib = (std::f32::consts::TAU * 18.0 * u).sin() * 0.012;
                    let tone = 0.93 + rib + (n - 0.5) * 0.035;
                    (tone, tone, tone * 0.99, 0.90 + (n - 0.5) * 0.04)
                }
            };
            out[y * size + x] = [
                r.clamp(0.0, 1.0),
                g.clamp(0.0, 1.0),
                b.clamp(0.0, 1.0),
                rough.clamp(0.0, 1.0),
            ];
        }
    }
    out
}

fn height_map(layer: usize, size: usize) -> Vec<f32> {
    let mut out = vec![0.0; size * size];
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let h = match layer {
                0 => plaster(u, v, x, y),
                1 => wood(u, v, x, y),
                2 => fabric(u, v),
                3 => mineral(x, y),
                _ => shingle(x, y),
            };
            out[y * size + x] = h;
        }
    }
    out
}

fn plaster(u: f32, v: f32, x: usize, y: usize) -> f32 {
    // Tileable value noise gives soft trowel mottling in several sizes; a
    // little pixel stipple supplies the tooth of the roller.
    let broad = (noise(u, v, 3, 11) - 0.5) * 0.55
        + (noise(u, v, 7, 13) - 0.5) * 0.28
        + (noise(u, v, 21, 17) - 0.5) * 0.12;
    let stipple = hash01(x as u32, y as u32, 7) * 0.10 - 0.05;
    broad + stipple
}

fn noise(u: f32, v: f32, freq: u32, seed: u32) -> f32 {
    let x = u * freq as f32;
    let y = v * freq as f32;
    let xi = x.floor() as u32;
    let yi = y.floor() as u32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;
    let sx = xf * xf * (3.0 - 2.0 * xf);
    let sy = yf * yf * (3.0 - 2.0 * yf);
    let x0 = xi % freq;
    let y0 = yi % freq;
    let x1 = (xi + 1) % freq;
    let y1 = (yi + 1) % freq;
    let a = hash01(x0, y0, seed);
    let b = hash01(x1, y0, seed);
    let c = hash01(x0, y1, seed);
    let d = hash01(x1, y1, seed);
    (a * (1.0 - sx) + b * sx) * (1.0 - sy) + (c * (1.0 - sx) + d * sx) * sy
}

fn wood(u: f32, v: f32, x: usize, y: usize) -> f32 {
    let warp = (std::f32::consts::TAU * (2.0 * u + v) + 1.4).sin() * 0.010
        + (std::f32::consts::TAU * (5.0 * u + 3.0 * v) + 0.3).sin() * 0.006;
    let grain = (std::f32::consts::TAU * (28.0 * u + warp)).sin() * 0.10
        + (std::f32::consts::TAU * (57.0 * u + 17.0 * v + warp)).sin() * 0.035;
    let pore = if hash01(x as u32 / 2, y as u32 / 2, 31) > 0.86 {
        -0.055
    } else {
        0.0
    };
    grain + pore
}

fn fabric(u: f32, v: f32) -> f32 {
    let weave =
        (std::f32::consts::TAU * 24.0 * u).sin() * (std::f32::consts::TAU * 24.0 * v).sin() * 0.12;
    let slub = (std::f32::consts::TAU * (6.0 * u + v) + (std::f32::consts::TAU * 3.0 * v).sin())
        .sin()
        * 0.055;
    weave + slub
}

fn mineral(x: usize, y: usize) -> f32 {
    hash01(x as u32, y as u32, 19) * 0.12 - 0.06 + hash01(x as u32 / 3, y as u32 / 3, 23) * 0.10
        - 0.05
}

fn shingle(x: usize, y: usize) -> f32 {
    hash01(x as u32 / 2, y as u32 / 2, 41) * 0.16 - 0.08
        + hash01(x as u32 / 7, y as u32 / 7, 43) * 0.10
        - 0.05
}

fn hash01(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

fn downsample(src: &[f32], src_size: usize) -> Vec<f32> {
    let dst = src_size / 2;
    let mut out = vec![0.0; dst * dst];
    for y in 0..dst {
        for x in 0..dst {
            let sx = x * 2;
            let sy = y * 2;
            out[y * dst + x] = (src[sy * src_size + sx]
                + src[sy * src_size + sx + 1]
                + src[(sy + 1) * src_size + sx]
                + src[(sy + 1) * src_size + sx + 1])
                * 0.25;
        }
    }
    out
}

fn upload(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    level: u32,
    size: usize,
    heights: &[f32],
) {
    let rgba = normal_rgba(heights, size);
    let row_bytes = size * 4;
    let padded = row_bytes.div_ceil(256) * 256;
    let mut data = vec![0u8; padded * size];
    for y in 0..size {
        data[y * padded..y * padded + row_bytes]
            .copy_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(size as u32),
        },
        wgpu::Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: 1,
        },
    );
}

fn downsample_rgba(src: &[[f32; 4]], src_size: usize) -> Vec<[f32; 4]> {
    let dst = src_size / 2;
    let mut out = vec![[0.0; 4]; dst * dst];
    for y in 0..dst {
        for x in 0..dst {
            let sx = x * 2;
            let sy = y * 2;
            let mut acc = [0.0; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let px = src[(sy + dy) * src_size + sx + dx];
                for c in 0..4 {
                    acc[c] += px[c] * 0.25;
                }
            }
            out[y * dst + x] = acc;
        }
    }
    out
}

fn upload_albedo(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    level: u32,
    size: usize,
    pixels: &[[f32; 4]],
) {
    let row_bytes = size * 16;
    let padded = row_bytes.div_ceil(256) * 256;
    let mut data = vec![0u8; padded * size];
    for (y, row) in pixels.chunks_exact(size).enumerate() {
        for (x, px) in row.iter().enumerate() {
            let i = y * padded + x * 4;
            for c in 0..4 {
                data[i + c] = (px[c] * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(size as u32),
        },
        wgpu::Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: 1,
        },
    );
}

fn normal_rgba(heights: &[f32], size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let xl = (x + size - 1) % size;
            let xr = (x + 1) % size;
            let yl = (y + size - 1) % size;
            let yr = (y + 1) % size;
            let dx = (heights[y * size + xr] - heights[y * size + xl]) * 2.0;
            let dy = (heights[yr * size + x] - heights[yl * size + x]) * 2.0;
            let n = normalize3(-dx, -dy, 1.0);
            let i = (y * size + x) * 4;
            out[i] = ((n.0 * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            out[i + 1] = ((n.1 * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            out[i + 2] = ((n.2 * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            out[i + 3] = 255;
        }
    }
    out
}

fn normalize3(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    let len = (x * x + y * y + z * z).sqrt().max(1e-6);
    (x / len, y / len, z / len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_atlas_layer_has_visible_relief() {
        for layer in 0..LAYERS {
            let heights = height_map(layer, 128);
            let rgba = normal_rgba(&heights, 128);
            let mut lo = 255u8;
            let mut hi = 0u8;
            for px in rgba.chunks_exact(4) {
                lo = lo.min(px[0]);
                hi = hi.max(px[0]);
            }
            assert!(
                hi.saturating_sub(lo) > 24,
                "layer {layer} normal map is too flat: {lo}..{hi}"
            );
        }
    }

    #[test]
    fn albedo_layers_have_colour_and_roughness_variation() {
        for layer in 0..ALBEDO_LAYERS {
            let pixels = albedo_map(layer, 96);
            let lo = pixels.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
            let hi = pixels.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
            let rlo = pixels.iter().map(|p| p[3]).fold(f32::INFINITY, f32::min);
            let rhi = pixels.iter().map(|p| p[3]).fold(f32::NEG_INFINITY, f32::max);
            assert!(hi - lo > 0.015, "layer {layer} albedo is flat: {lo}..{hi}");
            assert!(rhi - rlo > 0.005, "layer {layer} roughness is flat: {rlo}..{rhi}");
        }
    }

    #[test]
    fn downsampling_keeps_the_height_range() {
        let src = height_map(0, 128);
        let small = downsample(&src, 128);
        let lo = small.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = small.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi - lo > 0.2);
    }
}
