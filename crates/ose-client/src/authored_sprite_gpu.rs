//! Batched original particle sprites and spark meshes, animated using their authored recipes.
use crate::{
    authored_particles::Sprite,
    engine_fx::{srgb_to_linear, Library},
};
use glam::DVec3;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    position: [f32; 4],
    x: [f32; 4],
    y: [f32; 4],
    z: [f32; 4],
    color: [f32; 4],
    uv: [f32; 4],
    next: [f32; 4],
    emission: [f32; 4],
    params: [f32; 4],
    lighting: [f32; 4],
}
pub struct Batch {
    pub material: String,
    pub mesh: String,
    pub start: u32,
    pub end: u32,
    pub opaque: bool,
}
pub struct Gpu {
    pub camera: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    camera_buffer: wgpu::Buffer,
    capacity: usize,
    pub batches: Vec<Batch>,
    pipelines: [wgpu::RenderPipeline; 4],
}
impl Gpu {
    pub fn new(
        device: &wgpu::Device,
        textures: &wgpu::BindGroupLayout,
        screen: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle camera"),
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
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle camera"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("authored particles"),
            bind_group_layouts: &[Some(&layout), Some(textures), Some(screen)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("authored particle shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../assets/own/fx/authored_particles.wgsl").into(),
            ),
        });
        let instances = wgpu::VertexBufferLayout {
            array_stride: 160,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![3=>Float32x4,4=>Float32x4,5=>Float32x4,6=>Float32x4,7=>Float32x4,8=>Float32x4,9=>Float32x4,10=>Float32x4,11=>Float32x4,12=>Float32x4],
        };
        let geometry = wgpu::VertexBufferLayout {
            array_stride: 104,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2],
        };
        let pipelines = std::array::from_fn(|index| {
            let mesh = index >= 2;
            let opaque = index % 2 == 1;
            let buffers = if mesh {
                vec![Some(geometry.clone()), Some(instances.clone())]
            } else {
                vec![Some(instances.clone())]
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("authored particles"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if mesh { "vs_mesh" } else { "vs_sprite" }),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: crate::fx_post::FORMAT,
                        blend: if opaque {
                            None
                        } else {
                            Some(wgpu::BlendState::ALPHA_BLENDING)
                        },
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Greater),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        let capacity = 256;
        let buffer = Self::buffer(device, capacity);
        Self {
            camera,
            buffer,
            camera_buffer,
            capacity,
            batches: Vec::new(),
            pipelines,
        }
    }
    fn buffer(device: &wgpu::Device, count: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("authored particles"),
            size: (count * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sprites: &[Sprite],
        library: &Library,
        frame: &crate::engine_fx_gpu::FxView,
    ) {
        let crate::engine_fx_gpu::FxView {
            camera,
            view_proj,
            near,
            light,
            ..
        } = *frame;
        self.batches.clear();
        let mut data = Vec::new();
        let vec4 = |v: DVec3| [v.x as f32, v.y as f32, v.z as f32, 0.0];
        for sprite in sprites {
            let Some(material) = library.particle_materials.get(&sprite.material) else {
                continue;
            };
            let f = |key, default| *material.floats.get(key).unwrap_or(&default);
            let c = material.colors.get("_Color").copied().unwrap_or([1.0; 4]);
            let emission = material
                .colors
                .get("_EmissionColor")
                .copied()
                .unwrap_or([0.0; 4]);
            let color = std::array::from_fn(|i| {
                if i == 3 {
                    sprite.color[i] * c[i] as f32
                } else {
                    (srgb_to_linear(sprite.color[i] as f64) * srgb_to_linear(c[i])) as f32
                }
            });
            let opaque = f("_DstBlend", 10.0) == 0.0;
            let start = data.len() as u32;
            if let Some(previous) = self
                .batches
                .last_mut()
                .filter(|b| b.material == sprite.material && b.mesh == sprite.mesh)
            {
                previous.end = start + 1;
            } else {
                self.batches.push(Batch {
                    material: sprite.material.clone(),
                    mesh: sprite.mesh.clone(),
                    start,
                    end: start + 1,
                    opaque,
                });
            }
            let mut position = vec4(sprite.position - camera);
            position[3] = 1.0;
            data.push(Instance {
                position,
                x: vec4(sprite.axis_x),
                y: vec4(sprite.axis_y),
                z: vec4(sprite.axis_z),
                color,
                uv: sprite.uv,
                next: sprite.next_uv,
                emission: std::array::from_fn(|i| {
                    if i < 3 {
                        (srgb_to_linear(emission[i]) * f("_EmissionEnabled", 0.0)) as f32
                    } else {
                        0.0
                    }
                }),
                params: [
                    f("_LightingEnabled", 0.0) as f32,
                    if f("_SoftParticlesEnabled", 0.0) > 0.0 {
                        1.0 / f("_SoftParticlesFarFadeDistance", 1.0).max(1e-6) as f32
                    } else {
                        0.0
                    },
                    if material.textures.contains_key("_BumpMap") {
                        1.0
                    } else {
                        0.0
                    },
                    if f("_FlipbookMode", 0.0) > 0.0 {
                        sprite.blend
                    } else {
                        0.0
                    },
                ],
                lighting: [
                    f("_DiffuseWrapPercentage", 0.0) as f32,
                    f("_LightIntensityPercentage", 1.0) as f32,
                    f("_ScatterPercentage", 0.0) as f32,
                    f("_BumpScale", 1.0) as f32,
                ],
            });
        }
        if data.len() > self.capacity {
            self.capacity = data.len().next_power_of_two();
            self.buffer = Self::buffer(device, self.capacity);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&data));
        }
        let mut uniform = Vec::from(view_proj.as_mat4().to_cols_array());
        uniform.extend(vec4(light));
        uniform.extend([near as f32, 0.25, 0.0, 0.0]);
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&uniform));
    }
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>, batch: &Batch) -> bool {
        let mesh = !batch.mesh.is_empty();
        pass.set_pipeline(&self.pipelines[(if mesh { 2 } else { 0 }) + usize::from(batch.opaque)]);
        pass.set_bind_group(0, &self.camera, &[]);
        pass.set_vertex_buffer(if mesh { 1 } else { 0 }, self.buffer.slice(..));
        mesh
    }
}
