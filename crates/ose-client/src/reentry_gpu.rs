//! The original single-vessel 2048² radial-depth occlusion pass for Reentry/Shockwave.
use glam::{DMat4, DVec3};
use wgpu::util::DeviceExt;
pub struct Occluder<'a> {
    pub vertices: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub count: u32,
    pub model: DMat4,
    pub bounds: [[f64; 3]; 2],
}
pub struct Gpu {
    pub layout: wgpu::BindGroupLayout,
    texture: wgpu::TextureView,
    pub image: wgpu::Texture,
    depth: wgpu::TextureView,
    sampler: wgpu::Sampler,
    depth_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
}
fn buffer_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Gpu {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = |format, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 2048,
                    height: 2048,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let image = texture(wgpu::TextureFormat::R16Float, "reentry radial depth");
        let texture = image.create_view(&Default::default());
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("reentry depth attachment"),
                size: wgpu::Extent3d {
                    width: 2048,
                    height: 2048,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reentry vessel and occlusion data"),
            entries: &[
                buffer_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("original reentry point sampling"),
            ..Default::default()
        });
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reentry depth matrices"),
            entries: &[buffer_entry(0)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reentry depth"),
            bind_group_layouts: &[Some(&depth_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("original radial depth replacement"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../assets/own/fx/reentry_depth.wgsl").into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reentry occlusion"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 104,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            texture,
            image,
            depth,
            sampler,
            depth_layout,
            pipeline,
            slots: Vec::new(),
        }
    }
    pub fn slot(&self, device: &wgpu::Device) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reentry record"),
            size: 272,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reentry record"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.texture),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        (buffer, bind)
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[Occluder<'_>],
        heading: DVec3,
    ) -> [f32; 28] {
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for shape in shapes {
            for i in 0..8 {
                let point = DVec3::new(
                    shape.bounds[usize::from(i & 1 != 0)][0],
                    shape.bounds[usize::from(i & 2 != 0)][1],
                    shape.bounds[usize::from(i & 4 != 0)][2],
                );
                let point = shape.model.transform_point3(point);
                lo = lo.min(point);
                hi = hi.max(point);
            }
        }
        if shapes.is_empty() {
            lo = DVec3::ZERO;
            hi = DVec3::ONE;
        }
        let center = (lo + hi) * 0.5;
        let radius = (hi - lo).length() * 0.5;
        let camera = center + heading * (radius + 1.0);
        let size = radius.max(10.0);
        let far = (radius * 2.0 + 2.0).max(100.0);
        let near = -0.999999;
        let up = if heading.dot(DVec3::Y).abs() > 0.99 {
            DVec3::Z
        } else {
            DVec3::Y
        };
        let view = glam::dcamera::rh::view::look_at_mat4(camera, center, up);
        let proj = DMat4::from_cols(
            glam::DVec4::new(1.0 / size, 0.0, 0.0, 0.0),
            glam::DVec4::new(0.0, 1.0 / size, 0.0, 0.0),
            glam::DVec4::new(0.0, 0.0, 1.0 / (near - far), 0.0),
            glam::DVec4::new(0.0, 0.0, near / (near - far), 1.0),
        );
        let vp = proj * view;
        for (i, shape) in shapes.iter().enumerate() {
            if self.slots.len() <= i {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("reentry depth slot"),
                    contents: &[0; 160],
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("reentry depth slot"),
                    layout: &self.depth_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                self.slots.push((buffer, bind));
            }
            let mut data = Vec::from(vp.as_mat4().to_cols_array());
            data.extend(shape.model.as_mat4().to_cols_array());
            data.extend([
                camera.x as f32,
                camera.y as f32,
                camera.z as f32,
                0.0,
                near as f32,
                far as f32,
                0.0,
                0.0,
            ]);
            queue.write_buffer(&self.slots[i].0, 0, bytemuck::cast_slice(&data));
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reentry depth from upstream"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.texture,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
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
            pass.set_pipeline(&self.pipeline);
            for (i, shape) in shapes.iter().enumerate() {
                pass.set_bind_group(0, &self.slots[i].1, &[]);
                pass.set_vertex_buffer(0, shape.vertices.slice(..));
                pass.set_index_buffer(shape.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..shape.count, 0, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
        let mut data = [0.0; 28];
        data[..16].copy_from_slice(&vp.as_mat4().to_cols_array());
        data[16..19].copy_from_slice(&camera.as_vec3().to_array());
        data[19..23].copy_from_slice(&[1.0, near as f32, far as f32, 1.0 / far as f32]);
        data[27] = 2048.0;
        data
    }
}
