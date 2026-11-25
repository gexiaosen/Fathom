//! Sprint 3 gate: 2D plan rendering at 5000 primitives, offscreen.
//!
//! Two modes are measured:
//!   static  – batch uploaded once, only GPU draw per frame (pan/zoom already
//!             applied by a vertex uniform in the real app, here just draw)
//!   dynamic – rebuild the vertex batch every frame (move/select/add edits)
//!
//! Run with:
//!   BENCH_WALLS=5000 cargo run --release -p fp-desktop --bin bench_2d
//!
//! Knobs: BENCH_WALLS, BENCH_FRAMES, BENCH_WIDTH, BENCH_HEIGHT, BENCH_MODE

use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use fp_core::{EntityId, Pt};
use fp_model::{Wall, WallKind};
use fp_render2d::{Batch, Camera2D, Color, build_grid, build_wall_batch};

const WARMUP_FRAMES: u32 = 20;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct V {
    pos: [f32; 2],
    color: [f32; 4],
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

fn main() {
    let walls = env_usize("BENCH_WALLS", 5000);
    let frames = env_u32("BENCH_FRAMES", 600);
    let width = env_u32("BENCH_WIDTH", 1600);
    let height = env_u32("BENCH_HEIGHT", 1000);
    let mode = std::env::var("BENCH_MODE").unwrap_or_else(|_| "dynamic".into());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no compatible adapter");
    let info = adapter.get_info();
    eprintln!("[bench2d] adapter={} backend={:?}", info.name, info.backend);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bench_2d"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .expect("device");

    let plan = make_plan(walls);
    let mut cam = Camera2D::new([width as f32, height as f32]);
    cam.fit(Pt::new(0, 0), Pt::new(12_000, 9_000), 40.0);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench2d target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bench2d shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bench2d bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let vp_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench2d viewport"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &vp_buf,
        0,
        bytemuck::bytes_of(&[width as f32, height as f32, 0.0, 0.0]),
    );
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bench2d bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: vp_buf.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bench2d layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bench2d pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<V>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 8,
                        shader_location: 1,
                    },
                ],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let max_bytes = (walls * 6 + 500 * 6 + 64) * std::mem::size_of::<V>();
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench2d vertices"),
        size: max_bytes as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let static_batch = scene_batch(&plan, &cam);
    let mut samples = Vec::with_capacity(frames as usize);
    for frame in 0..frames + WARMUP_FRAMES {
        let t0 = Instant::now();

        let batch = if mode == "static" {
            None
        } else {
            Some(scene_batch(&plan, &cam))
        };
        if let Some(b) = &batch {
            queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&b.vertices));
        } else if frame == 0 {
            queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&static_batch.vertices));
        }
        let count = batch
            .as_ref()
            .map(|b| b.vertices.len())
            .unwrap_or(static_batch.vertices.len()) as u32;

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench2d enc"),
        });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench2d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.97,
                            g: 0.96,
                            b: 0.94,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.draw(0..count, 0..1);
        }
        queue.submit(Some(enc.finish()));
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .expect("poll");

        if frame >= WARMUP_FRAMES {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }

    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let avg = samples.iter().sum::<f64>() / samples.len() as f64;
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    println!(
        "BENCH_RESULT {{\"walls\":{},\"vertices\":{},\"width\":{},\"height\":{},\"mode\":\"{}\",\"frames\":{},\"avg_ms\":{:.3},\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"max_ms\":{:.3},\"fps_equiv\":{:.1},\"adapter\":\"{}\",\"backend\":\"{:?}\"}}",
        walls,
        static_batch.vertices.len(),
        width,
        height,
        mode,
        samples.len(),
        avg,
        p50,
        p95,
        *sorted.last().unwrap(),
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

/// Deterministic plan: `count` walls spread over a 12 m × 9 m sheet.
fn make_plan(count: usize) -> Vec<Wall> {
    let mut seed: u64 = 0x5eed_1234;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let x = (200.0 + next() * 11_600.0) as i32;
        let y = (200.0 + next() * 8_600.0) as i32;
        let len = (800.0 + next() * 3_000.0) as i32;
        let horizontal = i % 2 == 0;
        let end = if horizontal {
            Pt::new(x + len, y)
        } else {
            Pt::new(x, y + len)
        };
        let kind = match i % 4 {
            0 => WallKind::Bearing,
            1 => WallKind::Exterior,
            2 => WallKind::Partition,
            _ => WallKind::Low,
        };
        out.push(Wall::new(
            EntityId(i as u128 + 1),
            Pt::new(x, y),
            end,
            if i % 3 == 0 { 240 } else { 100 },
            kind,
        ));
    }
    out
}

fn scene_batch(plan: &[Wall], cam: &Camera2D) -> Batch {
    let mut batch = build_wall_batch(plan, &[], cam);
    let grid = build_grid(cam, 8.0);
    for line in &grid.minor {
        push_thick_line(&mut batch, cam, line, 1.0, Color::GRID_MINOR);
    }
    for line in &grid.major {
        push_thick_line(&mut batch, cam, line, 1.0, Color::GRID_MAJOR);
    }
    batch
}

fn push_thick_line(batch: &mut Batch, cam: &Camera2D, line: &[Pt; 2], width_px: f32, color: Color) {
    let a = cam.world_to_screen(line[0]);
    let b = cam.world_to_screen(line[1]);
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-3 {
        return;
    }
    let n = [-d[1] / len * width_px * 0.5, d[0] / len * width_px * 0.5];
    batch.push_quad(
        [
            [a[0] + n[0], a[1] + n[1]],
            [b[0] + n[0], b[1] + n[1]],
            [b[0] - n[0], b[1] - n[1]],
            [a[0] - n[0], a[1] - n[1]],
        ],
        color,
    );
}

const SHADER: &str = r#"
struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> viewport: vec4<f32>;

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    let ndc = vec2(
        in.pos.x / viewport.x * 2.0 - 1.0,
        1.0 - in.pos.y / viewport.y * 2.0,
    );
    out.clip = vec4(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}
@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;
