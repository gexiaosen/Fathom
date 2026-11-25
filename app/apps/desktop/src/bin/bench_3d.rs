//! Sprint 1 benchmark: N instanced furniture proxies rendered offscreen at 1080p
//! with a directional shadow map.
//!
//! Offscreen (no window, no vsync) so the numbers are real frame times and the
//! benchmark also runs headless in CI.
//!
//! Run with:
//!   BENCH_INSTANCES=200 cargo run --release -p fp-desktop --bin bench_3d
//!
//! Knobs:
//!   BENCH_INSTANCES  furniture proxies (default 200)
//!   BENCH_FRAMES     measured frames after 30 warmup frames (default 600)
//!   BENCH_WIDTH      default 1920
//!   BENCH_HEIGHT     default 1080
//!   BENCH_SHADOW     "0" disables the shadow pass
//!
//! Prints a machine-readable result line:
//!   BENCH_RESULT {"instances":200,"width":1920,...,"avg_ms":...,"fps_equiv":...}

use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

const SHADOW_MAP: u32 = 2048;
const WARMUP_FRAMES: u32 = 30;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    position: [f32; 3],
    scale: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    camera_pos: [f32; 4],
    params: [f32; 4],
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key).map(|v| v != "0").unwrap_or(default)
}

fn main() {
    let instances = env_usize("BENCH_INSTANCES", 200);
    let frames = env_u32("BENCH_FRAMES", 600);
    let width = env_u32("BENCH_WIDTH", 1920);
    let height = env_u32("BENCH_HEIGHT", 1080);
    let shadows = env_bool("BENCH_SHADOW", true);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no compatible adapter");

    let info = adapter.get_info();
    eprintln!(
        "[bench] adapter={} backend={:?} type={:?}",
        info.name, info.backend, info.device_type
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bench_3d"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .expect("failed to create device");

    let samples = render(device, queue, width, height, instances, frames, shadows);

    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = samples.iter().sum::<f64>() / samples.len() as f64;
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    let max = *sorted.last().unwrap();

    println!(
        "BENCH_RESULT {{\"instances\":{},\"width\":{},\"height\":{},\"shadow\":{},\"shadow_map\":{},\"frames\":{},\"avg_ms\":{:.3},\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"max_ms\":{:.3},\"fps_equiv\":{:.1},\"adapter\":\"{}\",\"backend\":\"{:?}\"}}",
        instances,
        width,
        height,
        shadows,
        if shadows { SHADOW_MAP } else { 0 },
        samples.len(),
        avg,
        p50,
        p95,
        max,
        1000.0 / avg,
        info.name.replace('"', " "),
        info.backend
    );
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn cube_vertices() -> Vec<Vertex> {
    // Unit cube, 36 vertices, per-face normals.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [-0.5, -0.5, 0.5],
            [
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
        ),
        (
            [0.5, -0.5, -0.5],
            [
                [0.5, -0.5, -0.5],
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
            ],
        ),
        (
            [0.5, -0.5, 0.5],
            [
                [0.5, -0.5, 0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
        ),
        (
            [-0.5, -0.5, -0.5],
            [
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [-0.5, 0.5, 0.5],
            [
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [-0.5, -0.5, -0.5],
            [
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
            ],
        ),
    ];
    let mut out = Vec::with_capacity(36);
    for (normal, corners) in faces {
        for index in [0usize, 1, 2, 0, 2, 3] {
            out.push(Vertex {
                position: corners[index],
                normal,
            });
        }
    }
    out
}

fn make_instances(count: usize) -> Vec<Instance> {
    // A deterministic pseudo-random scatter over a 30m x 20m floor.
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / 16777216.0
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let col = (i % 14) as f32;
        let row = (i / 14) as f32;
        let x = (col - 6.5) * 2.0 + (next() - 0.5) * 0.8;
        let z = (row - 7.0) * 2.0 + (next() - 0.5) * 0.8;
        let w = 0.3 + next() * 1.2;
        let h = 0.4 + next() * 1.0;
        let d = 0.3 + next() * 1.2;
        out.push(Instance {
            position: [x, h * 0.5, z],
            scale: [w, h, d],
            color: [0.45 + next() * 0.4, 0.35 + next() * 0.4, 0.3 + next() * 0.4],
        });
    }
    out
}

fn render(
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,
    instances: usize,
    frames: u32,
    shadows: bool,
) -> Vec<f64> {
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;

    let color_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let vertices = cube_vertices();
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench vertices"),
        size: (vertices.len() * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));

    let instance_data = make_instances(instances);
    let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench instances"),
        size: (instance_data.len() * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&instance_buffer, 0, bytemuck::cast_slice(&instance_data));

    let eye = Vec3::new(0.0, 12.0, 26.0);
    let target = Vec3::new(0.0, 1.0, 0.0);
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_rh(45f32.to_radians(), width as f32 / height as f32, 0.1, 120.0);
    let view_proj = proj * view;

    // Directional light shadow volume covering the 30x20m scatter area.
    let light_dir = Vec3::new(-0.35, -0.85, -0.4).normalize();
    let light_eye = -light_dir * 40.0;
    let light_view = Mat4::look_at_rh(light_eye, Vec3::ZERO, Vec3::Y);
    let light_proj = Mat4::orthographic_rh(-26.0, 26.0, -22.0, 22.0, 1.0, 90.0);
    let light_view_proj = light_proj * light_view;

    let uniforms = Uniforms {
        view_proj: view_proj.to_cols_array_2d(),
        light_view_proj: light_view_proj.to_cols_array_2d(),
        light_dir: [light_dir.x, light_dir.y, light_dir.z, 0.0],
        camera_pos: [eye.x, eye.y, eye.z, 1.0],
        params: [if shadows { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
    };
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

    let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench shadow map"),
        size: wgpu::Extent3d {
            width: SHADOW_MAP,
            height: SHADOW_MAP,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("bench shadow sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });

    let bgl_uniform = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bench uniform bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bench full bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let bind_group_uniform = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bench uniform bind group"),
        layout: &bgl_uniform,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bench bind group"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
        ],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bench shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout_uniform = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bench uniform layout"),
        bind_group_layouts: &[Some(&bgl_uniform)],
        immediate_size: 0,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bench layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });

    let vertex_layouts: [Option<wgpu::VertexBufferLayout>; 2] = [
        Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }),
    ];

    let main_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bench pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &vertex_layouts,
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bench shadow pipeline"),
        layout: Some(&layout_uniform),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_shadow"),
            compilation_options: Default::default(),
            buffers: &vertex_layouts,
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    });

    let mut samples = Vec::with_capacity(frames as usize);
    for frame in 0..frames + WARMUP_FRAMES {
        let frame_start = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench encoder"),
        });

        if shadows {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&shadow_pipeline);
            pass.set_bind_group(0, &bind_group_uniform, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, instance_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..instance_data.len() as u32);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.06,
                            b: 0.09,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&main_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, instance_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..instance_data.len() as u32);
        }

        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("device poll failed");

        if frame >= WARMUP_FRAMES {
            samples.push(frame_start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    samples
}

const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    camera_pos: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) instance_position: vec3<f32>,
    @location(3) instance_scale: vec3<f32>,
    @location(4) instance_color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
    @location(3) light_pos: vec4<f32>,
};

fn world_position(in: VertexInput) -> vec4<f32> {
    return vec4<f32>(in.position * in.instance_scale + in.instance_position, 1.0);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let world = world_position(in);
    var out: VertexOutput;
    out.position = u.view_proj * world;
    out.normal = normalize(in.normal / in.instance_scale);
    out.color = in.instance_color;
    out.world_pos = world.xyz;
    out.light_pos = u.light_view_proj * world;
    return out;
}

@vertex
fn vs_shadow(in: VertexInput) -> @builtin(position) vec4<f32> {
    let world = world_position(in);
    return u.light_view_proj * world;
}

fn shadow_factor(light_pos: vec4<f32>) -> f32 {
    if (u.params.x < 0.5) {
        return 1.0;
    }
    let ndc = light_pos.xyz / light_pos.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let reference = ndc.z - 0.0015;
    let texel = 1.0 / 2048.0;
    var sum = 0.0;
    for (var x = -1; x <= 1; x = x + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            sum = sum + textureSampleCompare(
                shadow_map,
                shadow_sampler,
                uv + vec2<f32>(f32(x), f32(y)) * texel,
                reference,
            );
        }
    }
    return sum / 9.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let light = normalize(-u.light_dir.xyz);
    let ndotl = max(dot(normalize(in.normal), light), 0.0);
    let shadow = shadow_factor(in.light_pos);
    let ambient = 0.2;
    let diffuse = 0.8 * ndotl * shadow;
    let color = in.color * (ambient + diffuse);
    return vec4<f32>(color, 1.0);
}
"#;
