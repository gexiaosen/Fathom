//! 3D pass: depth-tested shaded mesh for the plan extruded upward.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bytemuck::{Pod, Zeroable};
use fp_render3d::MeshVertex;

use crate::envmap;
use glam::Mat4;

const MAX_VERTICES: usize = 600_000;
const SHADOW_SIZE: u32 = 2048;
const MSAA: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    light: [f32; 4],
    eye: [f32; 4],
    shade: [f32; 4],
    tint: [f32; 4],
    lamps: [[f32; 4]; 8],
    light_vp: [[f32; 4]; 4],
    screen: [f32; 4],
}

/// Lighting preset for the 3D pass: sun direction, ambient/diffuse weights,
/// colour tint and background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub light: [f32; 3],
    pub ambient: f32,
    pub diffuse: f32,
    pub tint: [f32; 3],
    pub clear: [f32; 3],
    /// Up to eight warm lamps: xyz position + strength. Used at night.
    pub lamps: [[f32; 4]; 8],
    pub lamp_count: f32,
    /// World -> sun clip space, used for the shadow map.
    pub light_vp: [[f32; 4]; 4],
    /// 0 = no shadow look-up, 1 = sample the sun shadow map.
    pub shadows: f32,
}

/// Everything the 3D pass needs besides the mesh itself.
#[derive(Clone, Copy, Debug)]
pub struct Scene3dView {
    pub view_proj: Mat4,
    pub eye: [f32; 3],
    pub look: Look,
}

impl Look {
    pub fn day() -> Self {
        Self {
            light: [0.45, -1.0, 0.35],
            ambient: 0.36,
            diffuse: 0.64,
            tint: [1.0, 1.0, 1.0],
            clear: [0.08, 0.09, 0.12],
            lamps: [[0.0; 4]; 8],
            lamp_count: 0.0,
            light_vp: Mat4::IDENTITY.to_cols_array_2d(),
            shadows: 0.0,
        }
    }
}

pub struct Renderer3d {
    pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    glass_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    depth: wgpu::TextureView,
    depth_size: (u32, u32),
    uploaded: Option<u64>,
    glass_key: Option<(u64, u32, [u32; 3])>,
    glass_len: u32,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_views: [wgpu::TextureView; 2],
    shadow_bind_group: wgpu::BindGroup,
    main_bind_groups: [wgpu::BindGroup; 2],
    opaque_bind_groups: [wgpu::BindGroup; 2],
    shadow_slot: usize,
    shadow_primed: Option<usize>,
    opaque_key: Option<u64>,
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    msaa_view: wgpu::TextureView,
    opaque_view: wgpu::TextureView,
    bgl: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
    env: envmap::EnvMap,
}

impl Renderer3d {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        // Two shadow maps, ping-ponged: a frame writes one and samples the
        // previous frame's, so the same texture is never read and written in
        // one submission.
        let mut shadow_views = Vec::new();
        for i in 0..2 {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(if i == 0 {
                    "scene3d shadow a"
                } else {
                    "scene3d shadow b"
                }),
                size: wgpu::Extent3d {
                    width: SHADOW_SIZE,
                    height: SHADOW_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            shadow_views.push(tex.create_view(&Default::default()));
        }
        let shadow_views: [wgpu::TextureView; 2] = shadow_views.try_into().unwrap();
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let env = envmap::create(device, queue);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let opaque_view = create_opaque(device, format, (width, height));
        let dummy_view = create_dummy(device);
        let opaque_bind_groups = make_main_bind_groups(
            device,
            &bgl,
            &uniform,
            &shadow_views,
            &shadow_sampler,
            &env,
            &dummy_view,
        );
        let main_bind_groups = make_main_bind_groups(
            device,
            &bgl,
            &uniform,
            &shadow_views,
            &shadow_sampler,
            &env,
            &opaque_view,
        );
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d shadow bgl"),
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
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d shadow bg"),
            layout: &shadow_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut clear_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene3d shadow clear"),
        });
        for view in &shadow_views {
            let _pass = clear_enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d shadow clear pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
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
        }
        queue.submit(Some(clear_enc.finish()));
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d shadow layout"),
            bind_group_layouts: &[Some(&shadow_bgl)],
            immediate_size: 0,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = scene_pipeline(device, &layout, &shader, format, false);
        let glass_pipeline = scene_pipeline(device, &layout, &shader, format, true);
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d sky pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
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
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d shadow shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d shadow pipeline"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d vertices"),
            size: (MAX_VERTICES * std::mem::size_of::<MeshVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d glass indices"),
            size: (MAX_VERTICES * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let depth_size = (width, height);
        let depth = create_depth(device, depth_size, MSAA);
        let msaa_view = create_msaa(device, format, depth_size);

        queue.write_buffer(
            &uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                light: [0.45, -1.0, 0.35, 0.0],
                eye: [0.0, 0.0, 0.0, 0.0],
                shade: [0.36, 0.64, 0.0, 0.0],
                tint: [1.0, 1.0, 1.0, 1.0],
                lamps: [[0.0; 4]; 8],
                light_vp: Mat4::IDENTITY.to_cols_array_2d(),
                screen: [width as f32, height as f32, 0.0, 0.0],
            }),
        );

        Self {
            pipeline,
            sky_pipeline,
            glass_pipeline,
            uniform,
            vertices,
            indices,
            depth,
            depth_size,
            uploaded: None,
            glass_key: None,
            glass_len: 0,
            shadow_pipeline,
            shadow_views,
            shadow_bind_group,
            main_bind_groups,
            opaque_bind_groups,
            shadow_slot: 0,
            shadow_primed: None,
            opaque_key: None,
            device: device.clone(),
            format,
            msaa_view,
            opaque_view,
            bgl,
            shadow_sampler,
            env,
        }
    }

    /// Recreate the depth buffer when the surface size changes (Retina scales
    /// the back view to physical pixels).
    pub fn ensure_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.depth_size == size {
            return;
        }
        self.depth = create_depth(device, size, MSAA);
        self.msaa_view = create_msaa(device, self.format, size);
        self.opaque_view = create_opaque(device, self.format, size);
        self.main_bind_groups = make_main_bind_groups(
            device,
            &self.bgl,
            &self.uniform,
            &self.shadow_views,
            &self.shadow_sampler,
            &self.env,
            &self.opaque_view,
        );
        self.depth_size = size;
        self.opaque_key = None;
    }

    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        mesh: &[MeshVertex],
        epoch: u64,
        scene: Scene3dView,
    ) -> usize {
        let count = mesh.len().min(MAX_VERTICES) as u32;
        if self.uploaded != Some(epoch) {
            queue.write_buffer(
                &self.vertices,
                0,
                bytemuck::cast_slice(&mesh[..count as usize]),
            );
            self.uploaded = Some(epoch);
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&Uniforms {
                view_proj: scene.view_proj.to_cols_array_2d(),
                light: [
                    scene.look.light[0],
                    scene.look.light[1],
                    scene.look.light[2],
                    0.0,
                ],
                eye: [scene.eye[0], scene.eye[1], scene.eye[2], 0.0],
                shade: [
                    scene.look.ambient,
                    scene.look.diffuse,
                    scene.look.lamp_count,
                    scene.look.shadows,
                ],
                tint: [
                    scene.look.tint[0],
                    scene.look.tint[1],
                    scene.look.tint[2],
                    1.0,
                ],
                lamps: scene.look.lamps,
                light_vp: scene.look.light_vp,
                screen: [self.depth_size.0 as f32, self.depth_size.1 as f32, 0.0, 0.0],
            }),
        );
        mesh.iter()
            .position(|v| v.alpha < 0.999)
            .unwrap_or(mesh.len())
    }

    fn render_shadow(&self, queue: &wgpu::Queue, slot: usize, split: usize) {
        let mut shadow_enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene3d shadow enc"),
            });
        let mut pass = shadow_enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene3d shadow pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_views[slot],
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
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.shadow_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        // The ground slab is the first six vertices; it must not shadow itself.
        pass.draw(6..split as u32, 0..1);
        drop(pass);
        queue.submit(Some(shadow_enc.finish()));
    }

    /// Render the sun depth map before the caller opens its main encoder, so a
    /// single-shot export can sample real shadows without ping-pong lag.
    pub fn prime_shadow(
        &mut self,
        queue: &wgpu::Queue,
        mesh: &[MeshVertex],
        epoch: u64,
        scene: Scene3dView,
    ) {
        if mesh.is_empty() {
            return;
        }
        let split = self.upload(queue, mesh, epoch, scene);
        let write = self.shadow_slot;
        if split > 6 {
            self.render_shadow(queue, write, split);
            self.shadow_primed = Some(write);
            self.shadow_slot = 1 - write;
        }
    }

    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        mesh: &[MeshVertex],
        epoch: u64,
        scene: Scene3dView,
    ) {
        if mesh.is_empty() {
            return;
        }
        let count = mesh.len().min(MAX_VERTICES) as u32;
        let split = self.upload(queue, mesh, epoch, scene);
        let write = self.shadow_slot;
        let read = self.shadow_primed.take().unwrap_or(1 - write);
        if scene.look.shadows > 0.0 && split > 6 && self.shadow_primed.is_none() {
            self.render_shadow(queue, write, split);
        }
        self.shadow_slot = 1 - write;

        let split = mesh
            .iter()
            .position(|v| v.alpha < 0.999)
            .unwrap_or(mesh.len());
        let split = (split as u32).min(count);

        // Opaque pass: resolve into a sampleable copy so the glass pass can
        // read the scene behind each pane for refraction. This needs its own
        // submission: wgpu forbids sampling a texture in the same encoder
        // where it is used as a render target. When the camera and mesh are
        // unchanged the previous resolve is still valid, so idle frames skip
        // the whole opaque submit.
        let mut hasher = DefaultHasher::new();
        epoch.hash(&mut hasher);
        scene.eye.map(f32::to_bits).hash(&mut hasher);
        scene
            .view_proj
            .to_cols_array()
            .map(f32::to_bits)
            .hash(&mut hasher);
        // Lighting is not part of the vertex buffer, but it is part of the
        // opaque image. Without hashing it, toggling night or moving the sun
        // would leave the previous resolve on screen until the camera moved.
        scene.look.light.map(f32::to_bits).hash(&mut hasher);
        scene.look.ambient.to_bits().hash(&mut hasher);
        scene.look.diffuse.to_bits().hash(&mut hasher);
        scene.look.tint.map(f32::to_bits).hash(&mut hasher);
        scene.look.clear.map(f32::to_bits).hash(&mut hasher);
        scene.look.shadows.to_bits().hash(&mut hasher);
        scene.look.lamp_count.to_bits().hash(&mut hasher);
        for lamp in &scene.look.lamps {
            lamp.map(f32::to_bits).hash(&mut hasher);
        }
        self.depth_size.hash(&mut hasher);
        let frame_key = hasher.finish();
        if self.opaque_key != Some(frame_key) {
            let mut opaque_enc =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("scene3d opaque enc"),
                    });
            {
                let mut pass = opaque_enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene3d opaque pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.msaa_view,
                        resolve_target: Some(&self.opaque_view),
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: scene.look.clear[0] as f64,
                                g: scene.look.clear[1] as f64,
                                b: scene.look.clear[2] as f64,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
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
                pass.set_bind_group(0, &self.opaque_bind_groups[read], &[]);
                // Sky paper behind the studio ground, visible only at the horizon.
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..3, 0..1);
                pass.set_vertex_buffer(0, self.vertices.slice(..));
                pass.set_pipeline(&self.pipeline);
                if split > 0 {
                    pass.draw(0..split, 0..1);
                }
            }
            queue.submit(Some(opaque_enc.finish()));
            self.opaque_key = Some(frame_key);
        }

        if split < count {
            let eye = scene.eye;
            let key = (
                epoch,
                split,
                [
                    (eye[0] * 256.0) as u32,
                    (eye[1] * 256.0) as u32,
                    (eye[2] * 256.0) as u32,
                ],
            );
            if self.glass_key != Some(key) {
                let n = (count - split) as usize;
                let mut order: Vec<(f32, u32, u32, u32)> = Vec::with_capacity(n / 3);
                for t in (0..n.saturating_sub(2)).step_by(3) {
                    let (a, b, c) = (
                        split as usize + t,
                        split as usize + t + 1,
                        split as usize + t + 2,
                    );
                    let pa = mesh[a].pos;
                    let pb = mesh[b].pos;
                    let pc = mesh[c].pos;
                    let cx = (pa[0] + pb[0] + pc[0]) / 3.0 - eye[0];
                    let cy = (pa[1] + pb[1] + pc[1]) / 3.0 - eye[1];
                    let cz = (pa[2] + pb[2] + pc[2]) / 3.0 - eye[2];
                    // Far to near: painter's order for overlapping panes.
                    order.push((cx * cx + cy * cy + cz * cz, a as u32, b as u32, c as u32));
                }
                order.sort_by(|l, r| r.0.partial_cmp(&l.0).unwrap_or(std::cmp::Ordering::Equal));
                let mut idx = Vec::with_capacity(order.len() * 3);
                for (_, a, b, c) in &order {
                    idx.push(*a);
                    idx.push(*b);
                    idx.push(*c);
                }
                queue.write_buffer(&self.indices, 0, bytemuck::cast_slice(&idx));
                self.glass_len = idx.len() as u32;
                self.glass_key = Some(key);
            }
        }

        // Glass pass loads the opaque MSAA colour, blends glass on top and
        // resolves to the final view.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d glass pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    resolve_target: Some(view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.main_bind_groups[read], &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            if split < count {
                pass.set_pipeline(&self.glass_pipeline);
                pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.glass_len, 0, 0..1);
            }
        }
    }
}

fn make_main_bind_groups(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    shadow_views: &[wgpu::TextureView; 2],
    shadow_sampler: &wgpu::Sampler,
    env: &envmap::EnvMap,
    opaque: &wgpu::TextureView,
) -> [wgpu::BindGroup; 2] {
    let mut groups = Vec::new();
    for (i, view) in shadow_views.iter().enumerate() {
        groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(if i == 0 {
                "scene3d bg a"
            } else {
                "scene3d bg b"
            }),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&env.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&env.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(opaque),
                },
            ],
        }));
    }
    groups.try_into().unwrap()
}

/// Opaque pass writes depth; the glass pass blends and leaves depth untouched
/// so anything behind a pane still shows through.
fn scene_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    glass: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if glass {
            "scene3d glass pipeline"
        } else {
            "scene3d pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<MeshVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 12,
                        shader_location: 1,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 24,
                        shader_location: 2,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 36,
                        shader_location: 3,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32,
                        offset: 48,
                        shader_location: 4,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32,
                        offset: 52,
                        shader_location: 5,
                    },
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(!glass),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: if glass {
                    Some(wgpu::BlendState::ALPHA_BLENDING)
                } else {
                    None
                },
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_depth(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    samples: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_msaa(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d msaa"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: MSAA,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_dummy(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d dummy sample"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_opaque(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d opaque resolve"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

const SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
    eye: vec4<f32>,
    shade: vec4<f32>,
    tint: vec4<f32>,
    lamps: array<vec4<f32>, 8>,
    light_vp: mat4x4<f32>,
    screen: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var shadow_tex: texture_depth_2d;
@group(0) @binding(2) var shadow_samp: sampler_comparison;
@group(0) @binding(3) var env_tex: texture_cube<f32>;
@group(0) @binding(4) var env_samp: sampler;
@group(0) @binding(5) var opaque_tex: texture_2d<f32>;

struct SkyOut {
    @builtin(position) frag: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: SkyOut;
    let p = corners[vi];
    out.frag = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // Daytime keeps the same warm paper field as the 2D sheet; night keeps
    // the original moody studio gradient. The tint is blue-dominant only in
    // the night look, so it is a reliable discriminator here.
    let t = clamp(in.uv.y, 0.0, 1.0);
    let gbase = mix(vec3<f32>(0.30, 0.325, 0.355), vec3<f32>(0.055, 0.065, 0.090), pow(t, 0.85));
    let warm = mix(vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 0.95, 0.88), 0.12);
    let night_sky = gbase * warm * u.tint.xyz;
    let paper = vec3<f32>(0.965, 0.957, 0.941);
    let is_night = u.tint.b > u.tint.r + 0.04;
    return vec4<f32>(select(paper, night_sky, is_night), 1.0);
}

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) mat: vec3<f32>,
    @location(4) alpha: f32,
    @location(5) pattern: f32,
};
struct VOut {
    @builtin(position) frag: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) albedo: vec3<f32>,
    @location(3) mat: vec3<f32>,
    @location(4) alpha: f32,
    @location(5) pattern: f32,
};

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    out.frag = u.view_proj * vec4<f32>(in.pos, 1.0);
    out.world = in.pos;
    out.normal = in.normal;
    out.albedo = in.color;
    out.mat = in.mat;
    out.alpha = in.alpha;
    out.pattern = in.pattern;
    return out;
}

fn sun_shadow(wp: vec3<f32>) -> f32 {
    let clip = u.light_vp * vec4<f32>(wp, 1.0);
    let ndc = clip.xyz / clip.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // Orthographic NDC is -1..1; the depth texture uses 0..1 with the origin
    // at the top-left. The old 0..1 test clipped half the scene and made the
    // shadow lookup silently return full sun.
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
    var lit = 0.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            let off = vec2<f32>(f32(dx), f32(dy)) * (1.0 / 2048.0);
            lit = lit + textureSampleCompare(shadow_tex, shadow_samp, uv + off, ndc.z - 0.0012);
        }
    }
    return lit / 25.0;
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn floor_albedo(c: vec3<f32>, kind: f32, p: vec2<f32>) -> vec3<f32> {
    // Studio ground (pattern 7): the old mid-dark slab filled the whole
    // viewport and read as "night" even in daylight mode. Follow the plan
    // sheet's warm paper in day mode, keep the moody slab at night.
    if (kind > 6.5 && kind < 7.5) {
        let is_night = u.tint.b > u.tint.r + 0.04;
        let paper = vec3<f32>(0.94, 0.93, 0.915);
        let night = vec3<f32>(0.07, 0.074, 0.08);
        return select(paper, night, is_night);
    }
    // Furniture surfaces: 10 fabric, 11 wood, 12 brushed metal,
    // 13 glazed ceramic, 14 mirror.
    if (kind >= 9.5) {
        if (kind < 10.5) {
            let weave = sin(p.x * 320.0) * sin(p.y * 320.0);
            let slub = sin(p.x * 62.0 + sin(p.y * 41.0) * 1.7);
            return c * (0.955 + weave * 0.042 + slub * 0.024);
        }
        if (kind < 11.5) {
            let warp = sin(p.x * 11.0 + p.y * 4.0) * 2.0 + sin(p.y * 23.0) * 0.7;
            let grain = sin(p.x * 92.0 + warp) * 0.5 + 0.5;
            let ring = sin(p.y * 51.0 + warp * 1.9) * 0.5 + 0.5;
            let tone = 0.83 + grain * 0.10 + ring * 0.05;
            return c * tone * (0.97 + hash21(floor(p * 55.0)) * 0.035);
        }
        if (kind < 12.5) {
            let brush = sin(p.y * 2100.0 + p.x * 19.0) * 0.016
                + sin(p.x * 470.0 + p.y * 31.0) * 0.008;
            return c * (0.965 + brush);
        }
        if (kind < 13.5) {
            let glaze = hash21(floor(p * 420.0)) * 0.018
                + sin(p.x * 18.0 + p.y * 13.0) * 0.012;
            return c * (0.985 + glaze);
        }
        return c * (0.97 + sin(p.y * 900.0 + p.x * 7.0) * 0.018);
    }
    if (kind < 0.5) {
        // Plain walls and matte fronts get a faint plaster grain instead of
        // reading as flat plastic under the sun.
        let grain = (hash21(floor(p * 360.0)) - 0.5) * 0.014
            + sin(p.x * 41.0 + p.y * 37.0) * 0.004;
        return c * (1.0 + grain);
    }
    // Wood planks: staggered rows with a darker joint and fine grain.
    if (kind < 2.5) {
        let row = floor(p.y / 0.16);
        let off = hash21(vec2<f32>(row, 1.0)) * 1.1;
        let px = fract((p.x + off) / 1.25);
        let py = fract(p.y / 0.16);
        let joint = select(1.0, 0.68, px < 0.01 || py < 0.014);
        let grain = sin(p.x * 7.0 + p.y * 52.0 + row * 1.7) * 0.04;
        return c * joint * (1.0 + grain);
    }
    // Marble: soft grey veining.
    if (kind < 3.5) {
        let v = sin(p.x * 2.2 + sin(p.y * 1.6) * 2.3) + sin(p.y * 5.4 + p.x * 0.7) * 0.45;
        let vein = smoothstep(0.82, 1.0, abs(v));
        return c * (1.0 - 0.14 * vein) * (0.98 + hash21(floor(p * 8.0)) * 0.04);
    }
    // Carpet: short noisy pile.
    if (kind < 4.5) {
        let n = hash21(p * 220.0) * 0.10 + hash21(p * 40.0) * 0.05;
        return c * (0.94 + n);
    }
    // Terrazzo: dark chips on a warm base.
    if (kind < 5.5) {
        let cell = floor(p / 0.09);
        let h = hash21(cell);
        let chip = select(0.0, 1.0, h < 0.22);
        let tone = mix(0.55, 1.08, hash21(cell + 3.0));
        return mix(c, c * tone, chip) * (0.97 + hash21(p * 60.0) * 0.05);
    }
    // Ceramic tile: 0.6 m grid with thin grout and per-tile tint.
    if (kind < 6.5) {
        let u = fract(p.x / 0.6);
        let v = fract(p.y / 0.6);
        let grout = select(1.0, 0.70, u < 0.012 || v < 0.012);
        let tint = 0.97 + hash21(floor(p / 0.6)) * 0.06;
        return c * grout * tint;
    }
    // Stone: broad mottling.
    let n = sin(p.x * 8.0) * cos(p.y * 7.0) * 0.05 + sin((p.x + p.y) * 22.0) * 0.03;
    return c * (1.0 + n);
}

@fragment
fn fs_main(in: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Optical glass: sample the resolved opaque pass through a refraction
    // offset, tint it and add a Fresnel edge. Replaces alpha blending so the
    // background is never composited twice.
    if (in.pattern > 7.5 && in.pattern < 8.5) {
        var n = normalize(in.normal);
        if (!front) {
            n = -n;
        }
        let v = normalize(u.eye.xyz - in.world);
        let thick = clamp(in.pattern - 8.0, 0.0, 0.2);
        let scale = 0.012 + thick * 0.55;
        let uv = in.frag.xy / u.screen.xy;
        // Dispersion: red bends least, blue most. The three offsets converge
        // at normal incidence and separate towards grazing angles.
        let dr = refract(-v, n, 1.0 / 1.42);
        let dg = refract(-v, n, 1.0 / 1.45);
        let db = refract(-v, n, 1.0 / 1.49);
        let off_r = (u.view_proj * vec4<f32>(dr + v, 0.0)).xy * scale;
        let off_g = (u.view_proj * vec4<f32>(dg + v, 0.0)).xy * scale;
        let off_b = (u.view_proj * vec4<f32>(db + v, 0.0)).xy * scale;
        let lo = vec2<f32>(0.001, 0.001);
        let hi = vec2<f32>(0.999, 0.999);
        let r = textureSampleLevel(opaque_tex, env_samp, clamp(uv + off_r, lo, hi), 0.0).r;
        let g = textureSampleLevel(opaque_tex, env_samp, clamp(uv + off_g, lo, hi), 0.0).g;
        let b = textureSampleLevel(opaque_tex, env_samp, clamp(uv + off_b, lo, hi), 0.0).b;
        let bg = vec3<f32>(r, g, b);
        let tint = mix(vec3<f32>(1.0, 1.0, 1.0), in.albedo, 0.22 + thick * 3.0);
        let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0);
        let refl = textureSampleLevel(env_tex, env_samp, reflect(-v, n), 0.6).rgb * u.tint.xyz;
        return vec4<f32>(bg * tint + refl * fres * 0.85, 1.0);
    }
    // Materials are built as single-sided shells but drawn without culling:
    // flip the normal on back faces or the inside of a wall reads as a black
    // slab whenever the camera looks at it from the other side.
    var n = normalize(in.normal);
    if (!front && (in.pattern < 0.5 || in.pattern >= 9.5)) {
        n = -n;
    }
    var albedo = floor_albedo(in.albedo, in.pattern, in.world.xz);
    // A slim timber skirting reads as a baseboard and gives the large white
    // wall planes a grounded edge.
    if (in.pattern < 0.5) {
        albedo = mix(albedo, vec3<f32>(0.34, 0.26, 0.19),
            0.52 * (1.0 - smoothstep(0.08, 0.105, in.world.y)));
    }
    let l = normalize(-u.light.xyz);
    let v = normalize(u.eye.xyz - in.world);
    let diff = max(dot(n, l), 0.0);
    var rough = clamp(in.mat.x, 0.04, 1.0);
    let metal = clamp(in.mat.y, 0.0, 1.0);
    let glow = in.mat.z;
    if (in.pattern >= 9.5) {
        if (in.pattern < 10.5) {
            rough = max(rough, 0.94);
        } else if (in.pattern < 11.5) {
            rough = clamp(rough * 1.05 + 0.02, 0.0, 1.0);
        } else if (in.pattern < 12.5) {
            rough = clamp(rough * 0.8, 0.04, 1.0);
        }
    }
    // Furniture sits darker close to the floor: a cheap contact/ambient
    // occlusion that grounds legs, plinths and rugs without an SSAO pass.
    var occ = 1.0;
    if (in.pattern >= 9.5 && in.world.y > 0.035) {
        occ = mix(0.66, 1.0, smoothstep(0.03, 0.55, in.world.y));
    } else if (in.pattern < 0.5) {
        // Wall bases pick up a soft corner shadow where they meet the floor.
        occ = mix(0.74, 1.0, smoothstep(0.0, 0.6, in.world.y));
    }
    // Diffuse irradiance from the blurred room probe: ceiling-facing surfaces
    // pick up the softbox, walls pick up the window side, floors stay warm.
    let amb = textureSampleLevel(env_tex, env_samp, n, 3.6).rgb
        * u.tint.xyz * (u.shade.x * 0.95) * occ;
    let sh = sun_shadow(in.world + n * 0.03);
    let direct = u.tint.xyz * (u.shade.y * diff) * mix(1.0, sh, u.shade.w)
        * mix(occ, 1.0, 0.55);
    // Blinn-Phong highlight; tight for smooth surfaces, broad for rough ones.
    let h = normalize(l + v);
    let spec_pow = mix(72.0, 6.0, rough);
    let spec = pow(max(dot(n, h), 0.0), spec_pow) * (1.0 - rough * 0.85);
    let spec_tint = mix(vec3<f32>(1.0, 1.0, 1.0), albedo, metal);
    var base = albedo * (amb + direct);
    // Warm point lamps (night mode): inverse-square-ish falloff, no shadows.
    for (var i = 0u; i < 8u; i = i + 1u) {
        if (f32(i) >= u.shade.z) { break; }
        let lp = u.lamps[i].xyz;
        let to = lp - in.world;
        let d2 = dot(to, to);
        let att = u.lamps[i].w / (1.0 + d2 * 5.0);
        let ld = max(dot(n, normalize(to)), 0.0);
        base = base + albedo * vec3<f32>(1.0, 0.82, 0.60) * (att * ld);
    }
    // Prefiltered procedural room probe, sampled at a roughness LOD.
    let refl = reflect(-v, n);
    let env_col = textureSampleLevel(env_tex, env_samp, refl, rough * 5.0).rgb * u.tint.xyz;
    let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 4.0);
    let env = env_col * (fres * (0.35 + metal * 0.65) + (1.0 - rough) * 0.22) * 0.60;
    let plus_spec = spec_tint * spec * (0.5 + metal * 1.2) + env;
    // Soft fabric sheen at grazing angles, stronger on velvet-like covers.
    let sheen = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0)
        * select(0.0, 0.065, in.pattern >= 9.5 && in.pattern < 10.5)
        * (1.0 - metal);
    let emissive = albedo * glow;
    // ACES filmic curve (the same look the web viewer uses), exposure 1.05.
    let e = (base + plus_spec + albedo * sheen + emissive) * 1.05;
    let toned = (e * (2.51 * e + 0.03)) / (e * (2.43 * e + 0.59) + 0.14);
    return vec4<f32>(toned, in.alpha);
}
"#;

const SHADOW_SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
    eye: vec4<f32>,
    shade: vec4<f32>,
    tint: vec4<f32>,
    lamps: array<vec4<f32>, 8>,
    light_vp: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs_shadow(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return u.light_vp * vec4<f32>(pos, 1.0);
}
"#;
