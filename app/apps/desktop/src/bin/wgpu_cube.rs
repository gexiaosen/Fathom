//! Sprint 0 proof: a wgpu-rendered spinning cube composited into a WGPUI window.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use wgpui::{
    App, Application, Bounds, Context, Render, WgpuSurfaceHandle, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size, wgpu_surface,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 3],
}

struct CubeView {
    surface: WgpuSurfaceHandle,
    fps: f64,
    fps_rx: Receiver<f64>,
}

impl Render for CubeView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        while let Ok(fps) = self.fps_rx.try_recv() {
            self.fps = fps;
        }
        // The producer renders on its own thread; keep the UI presenting.
        window.request_animation_frame();

        div()
            .relative()
            .size_full()
            .bg(rgb(0x0b0d12))
            .child(wgpu_surface(self.surface.clone()).absolute().inset_0())
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left(px(12.0))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0x11141c))
                    .text_sm()
                    .text_color(rgb(0x9aa3b8))
                    .child(format!("wgpu cube · {:.0} fps", self.fps)),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(10.0))
                    .left(px(12.0))
                    .text_sm()
                    .text_color(rgb(0x6b7385))
                    .child("Sprint 0 · WGPUI WgpuSurface 合成验证"),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(720.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window: &mut Window, cx: &mut App| {
                let surface = window
                    .create_wgpu_surface(1280, 720, wgpu::TextureFormat::Rgba8UnormSrgb)
                    .expect("WGPUI WGPU surface is unavailable on this platform");
                let surface_for_thread = surface.clone();
                let (fps_tx, fps_rx) = mpsc::channel();
                thread::spawn(move || render_loop(surface_for_thread, fps_tx));
                cx.new(|_| CubeView {
                    surface,
                    fps: 0.0,
                    fps_rx,
                })
            },
        )
        .expect("failed to open the 3D window");
        cx.activate(true);
    });
}

type Face = ([f32; 3], [f32; 3], [f32; 3], [f32; 3], [f32; 3]);

fn cube_vertices() -> Vec<Vertex> {
    // Six faces, two triangles each, with a distinct colour per face.
    let faces: [Face; 6] = [
        (
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
            [0.95, 0.35, 0.35],
        ),
        (
            [0.5, -0.5, -0.5],
            [-0.5, -0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [0.5, 0.5, -0.5],
            [0.35, 0.6, 0.95],
        ),
        (
            [0.5, -0.5, 0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [0.5, 0.5, 0.5],
            [0.4, 0.85, 0.55],
        ),
        (
            [-0.5, -0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [-0.5, 0.5, 0.5],
            [-0.5, 0.5, -0.5],
            [0.95, 0.75, 0.3],
        ),
        (
            [-0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [0.75, 0.55, 0.95],
        ),
        (
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, -0.5, 0.5],
            [-0.5, -0.5, 0.5],
            [0.35, 0.85, 0.85],
        ),
    ];

    let mut out = Vec::with_capacity(36);
    for (a, b, c, d, color) in faces {
        for p in [a, b, c, a, c, d] {
            out.push(Vertex { position: p, color });
        }
    }
    out
}

fn render_loop(surface: WgpuSurfaceHandle, fps_tx: mpsc::Sender<f64>) {
    eprintln!("[cube] render thread started");
    while surface.back_buffer_view().is_none() {
        if surface.is_closed() {
            eprintln!("[cube] surface closed while waiting");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    eprintln!("[cube] surface ready: {:?}", surface.size());

    let device = surface.device().clone();
    let queue = surface.queue().clone();
    let format = surface.format();

    let vertices = cube_vertices();
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cube vertices"),
        size: (vertices.len() * std::mem::size_of::<Vertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));

    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cube time"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cube bind group layout"),
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

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cube bind group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cube shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cube pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cube pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
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
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let mut frame: u32 = 0;
    let mut last_report = Instant::now();
    let mut frames_since_report = 0u32;

    loop {
        if surface.is_closed() {
            return;
        }
        surface.wait_for_present();

        let Some((view, (width, height))) = surface.back_view_with_size() else {
            eprintln!("[cube] back view unavailable");
            thread::sleep(Duration::from_millis(2));
            continue;
        };
        if width == 0 || height == 0 {
            continue;
        }

        let time = frame as f32 * 0.016;
        queue.write_buffer(
            &uniform_buffer,
            0,
            bytemuck::cast_slice(&[time, 0.0, 0.0, 0.0]),
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cube encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cube pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.045,
                            g: 0.055,
                            b: 0.08,
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
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..1);
        }
        queue.submit([encoder.finish()]);
        surface.present();

        frame = frame.wrapping_add(1);
        frames_since_report += 1;
        if last_report.elapsed() >= Duration::from_secs(1) {
            let fps = frames_since_report as f64 / last_report.elapsed().as_secs_f64();
            eprintln!("[cube] fps {:.1}", fps);
            let _ = fps_tx.send(fps);
            last_report = Instant::now();
            frames_since_report = 0;
        }
    }
}

const SHADER: &str = r#"
struct Uniforms {
    time: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

fn rotate_y(p: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
}

fn rotate_x(p: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3<f32>(p.x, p.y * c - p.z * s, p.y * s + p.z * c);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var p = rotate_y(in.position, u.time);
    p = rotate_x(p, u.time * 0.7);
    let view = p + vec3<f32>(0.0, 0.0, -4.0);

    let aspect = 1280.0 / 720.0;
    let fovy = 45.0 * 3.14159265 / 180.0;
    let f = 1.0 / tan(fovy * 0.5);
    let znear = 0.1;
    let zfar = 100.0;
    let proj = mat4x4<f32>(
        vec4<f32>(f / aspect, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, f, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, (zfar + znear) / (znear - zfar), -1.0),
        vec4<f32>(0.0, 0.0, (2.0 * zfar * znear) / (znear - zfar), 0.0),
    );

    var out: VertexOutput;
    out.position = proj * vec4<f32>(view, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;
