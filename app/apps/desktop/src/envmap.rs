//! Procedural room environment probe for the 3D pass.
//!
//! The web viewer builds a `RoomEnvironment` cube map and runs it through
//! PMREM. We generate a small HDR cube map on the CPU instead: a warm studio
//! room with a ceiling softbox, a window strip and a floor bounce, then
//! prefilter it into roughness mips. No image assets, no GPU bake pass.

use glam::Vec3;

const SIZE: u32 = 64;
const MIPS: u32 = 6;
const SAMPLES: usize = 28;

pub struct EnvMap {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub fn create(device: &wgpu::Device, queue: &wgpu::Queue) -> EnvMap {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d env cube"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 6,
        },
        mip_level_count: MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    for mip in 0..MIPS {
        let size = (SIZE >> mip).max(1);
        let rough = if MIPS > 1 {
            mip as f32 / (MIPS - 1) as f32
        } else {
            0.0
        };
        for face in 0..6u32 {
            let mut texels: Vec<u16> = Vec::with_capacity((size * size * 4) as usize);
            for y in 0..size {
                for x in 0..size {
                    let u = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                    let v = (y as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                    let dir = face_dir(face, u, v);
                    // The lowest mip is the average of the room. Filling every
                    // face with the same value removes cube-edge seams at the
                    // roughest end of the chain.
                    let color = if mip + 1 >= MIPS {
                        room_average()
                    } else if mip == 0 {
                        room_color(dir)
                    } else {
                        prefilter(dir, rough)
                    };
                    texels.push(half::f16::from_f32(color.x).to_bits());
                    texels.push(half::f16::from_f32(color.y).to_bits());
                    texels.push(half::f16::from_f32(color.z).to_bits());
                    texels.push(half::f16::from_f32(1.0).to_bits());
                }
            }
            upload_face(queue, &texture, mip, face, size, &texels);
        }
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("scene3d env view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: 0,
        mip_level_count: Some(MIPS),
        base_array_layer: 0,
        array_layer_count: Some(6),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene3d env sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    EnvMap { view, sampler }
}

fn upload_face(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip: u32,
    face: u32,
    size: u32,
    texels: &[u16],
) {
    let row_bytes = (size as usize * 8).max(1);
    let padded_row = row_bytes.div_ceil(256) * 256;
    let mut data = vec![0u8; padded_row * size as usize];
    for row in 0..size as usize {
        let src = &texels[row * size as usize * 4..(row + 1) * size as usize * 4];
        let dst = row * padded_row;
        data[dst..dst + row_bytes].copy_from_slice(bytemuck::cast_slice(src));
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: face,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded_row as u32),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
}

/// Standard cube face directions: +X, -X, +Y, -Y, +Z, -Z.
fn face_dir(face: u32, u: f32, v: f32) -> Vec3 {
    let d = match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    };
    d.normalize()
}

/// A neutral warm room: light walls, a bright ceiling panel, a window strip at
/// head height and a darker floor.
fn room_color(d: Vec3) -> Vec3 {
    let up = d.y.clamp(-1.0, 1.0);
    let wall = Vec3::new(0.50, 0.50, 0.52) * (1.0 - up.abs() * 0.10);
    let floor = Vec3::new(0.26, 0.24, 0.22);
    let ceiling = Vec3::new(0.92, 0.90, 0.84);
    let mut c = if up >= 0.0 {
        wall.lerp(ceiling, smoothstep(0.20, 0.92, up))
    } else {
        wall.lerp(floor, smoothstep(0.02, 0.70, -up))
    };
    // Main window, cool daylight.
    let window = Vec3::new(0.38, 0.20, 0.90).normalize();
    c += Vec3::new(0.62, 0.72, 0.82) * lobe(d, window, 22.0) * 0.95;
    // Side window, slightly warmer.
    let side = Vec3::new(-0.92, 0.16, 0.36).normalize();
    c += Vec3::new(0.55, 0.52, 0.46) * lobe(d, side, 18.0) * 0.55;
    // Ceiling softbox.
    let box_lobe = Vec3::new(0.05, 0.98, 0.18).normalize();
    c += Vec3::new(0.38, 0.37, 0.34) * lobe(d, box_lobe, 9.0) * 0.70;
    c
}

fn lobe(d: Vec3, dir: Vec3, power: f32) -> f32 {
    d.dot(dir).max(0.0).powf(power)
}

fn room_average() -> Vec3 {
    let mut sum = Vec3::ZERO;
    let n = 96;
    for i in 0..n {
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - z * z).sqrt();
        let phi = i as f32 * 2.399963;
        sum += room_color(Vec3::new(r * phi.cos(), z, r * phi.sin()));
    }
    sum / n as f32
}

fn radical_inverse(mut i: u32) -> f32 {
    i = i.rotate_right(16);
    i = i.rotate_right(1);
    i = i.rotate_right(2);
    i = i.rotate_right(4);
    i = i.rotate_right(8);
    i as f32 * 2.328_306_4e-10
}

/// Cosine-lobe convolution: a rough surface sees a wider cone of the room.
fn prefilter(dir: Vec3, rough: f32) -> Vec3 {
    let cone = (rough * 1.35).clamp(0.0, 1.35);
    let mut tangent = Vec3::Y.cross(dir);
    if tangent.length_squared() < 1e-4 {
        tangent = Vec3::X.cross(dir);
    }
    let tangent = tangent.normalize();
    let bitangent = dir.cross(tangent).normalize();
    let mut sum = Vec3::ZERO;
    let mut weight = 0.0;
    for i in 0..SAMPLES {
        let u1 = (i as f32 + 0.5) / SAMPLES as f32;
        let u2 = radical_inverse(i as u32);
        let theta = cone * u1.sqrt();
        let phi = std::f32::consts::TAU * u2;
        let sample = (dir * theta.cos()
            + tangent * (theta.sin() * phi.cos())
            + bitangent * (theta.sin() * phi.sin()))
        .normalize();
        let w = theta.cos().max(0.0);
        sum += room_color(sample) * w;
        weight += w;
    }
    sum / weight.max(1e-5)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_cover_six_axes() {
        let mut points = Vec::new();
        for face in 0..6 {
            points.push(face_dir(face, 0.0, 0.0));
        }
        assert!(points.iter().all(|d| (d.length() - 1.0).abs() < 1e-5));
        assert!(points[0].x > 0.99);
        assert!(points[1].x < -0.99);
        assert!(points[2].y > 0.99);
        assert!(points[3].y < -0.99);
        assert!(points[4].z > 0.99);
    }

    #[test]
    fn mips_stay_positive() {
        for mip in 0..MIPS {
            let rough = mip as f32 / (MIPS - 1) as f32;
            let c = prefilter(Vec3::new(0.0, 1.0, 0.0), rough);
            assert!(c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0);
        }
        assert!(room_average().x > 0.0);
    }
}
