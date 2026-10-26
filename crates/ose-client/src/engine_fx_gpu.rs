//! Original mesh exhaust shaders, authored material curves and blend shapes on WebGPU.
use crate::engine_fx::{self, Layer, Library};
use glam::{DMat4, DVec3};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone)]
pub struct EngineDraw {
    pub instance: u64,
    pub part: String,
    /// The part's glTF frame in the camera-relative world. Assets retain original Unity coordinates.
    pub model: DMat4,
    pub throttle: f64,
    pub pressure: f64,
    pub atmosphere_density: f64,
    pub gravity_mps2: f64,
    pub relative_velocity: f64,
    pub mode: u32,
    pub time: f64,
    pub simulation_time: f64,
    pub commanded: f64,
    pub thrust_kn: f64,
    pub distance_from_spawn: f64,
    pub vertical_deviation: f64,
    pub reentry: Option<crate::reentry::State>,
    pub vapor: Option<crate::vapor::State>,
}

#[derive(Clone, Copy)]
pub struct FxView {
    pub view_proj: DMat4,
    pub view: DMat4,
    pub near: f64,
    pub width: u32,
    pub height: u32,
    pub camera: DVec3,
    pub light: DVec3,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    globals: [[f32; 4]; 64],
    camera: [[f32; 4]; 9],
    object: [[f32; 4]; 11],
    frame: [[f32; 4]; 23],
    weights: [f32; 4],
}
struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}
struct Draw {
    mesh: String,
    material: String,
    shader: String,
    slot: usize,
    refraction: bool,
}
#[derive(serde::Serialize)]
pub struct CaptureDraw {
    pub mesh: String,
    pub material: String,
    pub shader: String,
    pub uniform: Vec<f32>,
    pub refraction: bool,
    pub reentry_data: Vec<f32>,
}
enum Loaded {
    Library(Box<Library>),
    Engine(String, Vec<Layer>),
    Mesh(String, Vec<u8>),
    Shader(String, String),
    Texture(String, Vec<u8>),
    Error(String),
}

pub struct EngineFxGpu {
    library: Option<Rc<Library>>,
    engines: HashMap<String, Rc<Vec<Layer>>>,
    requested: HashSet<String>,
    inbox: Rc<RefCell<Vec<Loaded>>>,
    meshes: HashMap<String, Mesh>,
    shaders: HashMap<String, wgpu::RenderPipeline>,
    textures: HashMap<String, wgpu::TextureView>,
    materials: HashMap<String, wgpu::BindGroup>,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    screen_layout: wgpu::BindGroupLayout,
    screen: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    screen_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    draws: Vec<Draw>,
    capture: Vec<CaptureDraw>,
    capture_enabled: bool,
    active_assets: Vec<String>,
    particle_system: crate::authored_particles::System,
    trail_system: crate::authored_trails::System,
    particle_clock: crate::particles::ParticleClock,
    sprites: crate::authored_sprite_gpu::Gpu,
    sprite_materials: HashMap<String, wgpu::BindGroup>,
    reentry_gpu: Option<crate::reentry_gpu::Gpu>,
    reentry_slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    pub error: Option<String>,
}

impl EngineFxGpu {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: &wgpu::TextureView,
        scene: &wgpu::TextureView,
    ) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("authored FX uniforms"),
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
        let texture = |binding, depth| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: if depth {
                    wgpu::TextureSampleType::Depth
                } else {
                    wgpu::TextureSampleType::Float { filterable: true }
                },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("original exhaust maps"),
            entries: &[
                texture(0, false),
                texture(1, false),
                texture(2, false),
                sampler_entry(3),
            ],
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("heat refraction"),
            entries: &[texture(0, false), texture(1, true), sampler_entry(2)],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let screen_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let screen = Self::screen_bind(device, &screen_layout, &screen_sampler, depth, scene);
        let white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("FX white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white = white.create_view(&Default::default());
        let sprites = crate::authored_sprite_gpu::Gpu::new(device, &texture_layout, &screen_layout);
        Self {
            library: None,
            engines: HashMap::new(),
            requested: HashSet::new(),
            inbox: Default::default(),
            meshes: HashMap::new(),
            shaders: HashMap::new(),
            textures: HashMap::new(),
            materials: HashMap::new(),
            uniform_layout,
            texture_layout,
            screen_layout,
            screen,
            sampler,
            screen_sampler,
            white,
            slots: Vec::new(),
            draws: Vec::new(),
            capture: Vec::new(),
            capture_enabled: false,
            active_assets: Vec::new(),
            particle_system: Default::default(),
            trail_system: Default::default(),
            particle_clock: Default::default(),
            sprites,
            sprite_materials: HashMap::new(),
            reentry_gpu: None,
            reentry_slots: Vec::new(),
            error: None,
        }
    }
    fn screen_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        depth: &wgpu::TextureView,
        scene: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("refraction scene"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        scene: &wgpu::TextureView,
    ) {
        self.screen = Self::screen_bind(
            device,
            &self.screen_layout,
            &self.screen_sampler,
            depth,
            scene,
        );
    }
    fn bytes(&mut self, id: String, uri: String, kind: u32) {
        if !self.requested.insert(id.clone()) {
            return;
        }
        let inbox = self.inbox.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = async {
                let bytes = crate::render::fetch_bytes(&format!("assets/fx/{uri}")).await?;
                Ok(match kind {
                    0 => Loaded::Library(
                        serde_json::from_slice(&bytes).map_err(|e| format!("FX library: {e}"))?,
                    ),
                    1 => Loaded::Engine(
                        id[7..].into(),
                        serde_json::from_slice(&bytes).map_err(|e| format!("FX engine: {e}"))?,
                    ),
                    2 => Loaded::Mesh(id[5..].into(), bytes),
                    3 => Loaded::Shader(
                        id[7..].into(),
                        String::from_utf8(bytes).map_err(|e| e.to_string())?,
                    ),
                    _ => Loaded::Texture(id[8..].into(), bytes),
                })
            }
            .await;
            inbox
                .borrow_mut()
                .push(result.unwrap_or_else(|e: String| Loaded::Error(format!("{uri}: {e}"))));
        });
    }
    fn texture(&mut self, id: String, uri: String) {
        self.bytes(format!("texture:{id}"), uri, 4);
    }
    fn drain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let loaded: Vec<_> = self.inbox.borrow_mut().drain(..).collect();
        for item in loaded {
            match item {
                Loaded::Library(library) => self.library = Some(Rc::from(library)),
                Loaded::Engine(id, layers) => {
                    self.engines.insert(id, Rc::new(layers));
                }
                Loaded::Mesh(id, bytes) => {
                    let Some(spec) = self.library.as_ref().and_then(|l| l.meshes.get(&id)) else {
                        continue;
                    };
                    let vertex_bytes = (spec.vertices * spec.stride) as usize;
                    if bytes.len() != vertex_bytes + spec.indices as usize * 4 {
                        self.error = Some(format!("Corrupt FX mesh {id}"));
                        continue;
                    }
                    let buffer = |label, data: &[u8], usage| {
                        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some(label),
                            size: data.len() as u64,
                            usage,
                            mapped_at_creation: true,
                        });
                        buffer
                            .slice(..)
                            .get_mapped_range_mut()
                            .expect("mapped plume buffer")
                            .copy_from_slice(data);
                        buffer.unmap();
                        buffer
                    };
                    self.meshes.insert(
                        id,
                        Mesh {
                            vertices: buffer(
                                "original plume",
                                &bytes[..vertex_bytes],
                                wgpu::BufferUsages::VERTEX,
                            ),
                            indices: buffer(
                                "plume indices",
                                &bytes[vertex_bytes..],
                                wgpu::BufferUsages::INDEX,
                            ),
                            count: spec.indices,
                        },
                    );
                }
                Loaded::Shader(id, source) => {
                    let Some(spec) = self.library.as_ref().and_then(|l| l.shaders.get(&id)) else {
                        continue;
                    };
                    if spec.reentry && self.reentry_gpu.is_none() {
                        self.reentry_gpu = Some(crate::reentry_gpu::Gpu::new(device));
                    }
                    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some(&id),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    });
                    let mut bind_layouts = vec![
                        Some(&self.uniform_layout),
                        Some(&self.texture_layout),
                        Some(&self.screen_layout),
                    ];
                    if spec.reentry {
                        bind_layouts.push(Some(&self.reentry_gpu.as_ref().unwrap().layout));
                    }
                    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("original engine FX"),
                        bind_group_layouts: &bind_layouts,
                        immediate_size: 0,
                    });
                    let factor = |v| match v {
                        0 => wgpu::BlendFactor::Zero,
                        1 => wgpu::BlendFactor::One,
                        5 => wgpu::BlendFactor::SrcAlpha,
                        10 => wgpu::BlendFactor::OneMinusSrcAlpha,
                        _ => panic!("Unsupported original blend factor {v}"),
                    };
                    let blend = wgpu::BlendComponent {
                        src_factor: factor(spec.src_blend),
                        dst_factor: factor(spec.dst_blend),
                        operation: wgpu::BlendOperation::Add,
                    };
                    let pipeline=device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some(&id),layout:Some(&layout),
                        vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_plume"),compilation_options:Default::default(),buffers:&[Some(wgpu::VertexBufferLayout{array_stride:104,
                            step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x3,4=>Float32x3,5=>Float32x3,6=>Float32x3,7=>Float32x3,8=>Float32x3]})]},
                        fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_plume"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{format:crate::fx_post::FORMAT,blend:Some(wgpu::BlendState{color:blend,alpha:wgpu::BlendComponent::OVER}),write_mask:wgpu::ColorWrites::ALL})]}),
                        primitive:wgpu::PrimitiveState{cull_mode:None,..Default::default()},
                        depth_stencil:Some(wgpu::DepthStencilState{format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:Some(false),depth_compare:Some(wgpu::CompareFunction::Greater),stencil:Default::default(),bias:Default::default()}),
                        multisample:Default::default(),multiview_mask:None,cache:None});
                    self.shaders.insert(id, pipeline);
                }
                Loaded::Texture(id, bytes) => {
                    let Some(spec) = self.library.as_ref().and_then(|l| l.textures.get(&id)) else {
                        continue;
                    };
                    if spec.compression != "rgba"
                        && !device
                            .features()
                            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
                    {
                        self.error = Some(
                            "This adapter cannot sample the original block-compressed VFX textures"
                                .into(),
                        );
                        continue;
                    }
                    let format = match (spec.compression.as_str(), spec.srgb) {
                        ("bc1", false) => wgpu::TextureFormat::Bc1RgbaUnorm,
                        ("bc1", true) => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
                        ("bc3", false) => wgpu::TextureFormat::Bc3RgbaUnorm,
                        ("bc3", true) => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
                        ("bc6", _) => wgpu::TextureFormat::Bc6hRgbUfloat,
                        ("bc7", false) => wgpu::TextureFormat::Bc7RgbaUnorm,
                        ("bc7", true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
                        ("bc4", _) => wgpu::TextureFormat::Bc4RUnorm,
                        ("bc5", _) => wgpu::TextureFormat::Bc5RgUnorm,
                        (_, true) => wgpu::TextureFormat::Rgba8UnormSrgb,
                        (_, false) => wgpu::TextureFormat::Rgba8Unorm,
                    };
                    let size = wgpu::Extent3d {
                        width: spec.mips[0][0],
                        height: spec.mips[0][1],
                        depth_or_array_layers: 1,
                    };
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(&id),
                        size,
                        mip_level_count: spec.mips.len() as u32,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    let mut offset = 0;
                    for (level, [width, height]) in spec.mips.iter().copied().enumerate() {
                        let (block, bytes_per_block) = if spec.compression == "rgba" {
                            (1, 4)
                        } else {
                            (
                                4,
                                if spec.compression == "bc1" || spec.compression == "bc4" {
                                    8
                                } else {
                                    16
                                },
                            )
                        };
                        let rows = height.div_ceil(block);
                        let pitch = width.div_ceil(block) * bytes_per_block;
                        let length = (pitch * rows) as usize;
                        if offset + length > bytes.len() {
                            self.error = Some(format!("Truncated texture {id}"));
                            break;
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &texture,
                                mip_level: level as u32,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &bytes[offset..offset + length],
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(pitch),
                                rows_per_image: None,
                            },
                            wgpu::Extent3d {
                                width: width.div_ceil(block) * block,
                                height: rows * block,
                                depth_or_array_layers: 1,
                            },
                        );
                        offset += length;
                    }
                    self.textures
                        .insert(id, texture.create_view(&Default::default()));
                }
                Loaded::Error(error) => {
                    // A server without game assets (CI) answers the effect files with the page itself; the
                    // flight goes on without authored effects, so this is a warning, not a page error.
                    web_sys::console::warn_1(&error.clone().into());
                    self.error = Some(error);
                }
            }
        }
    }
    fn request_engine(&mut self, part: &str) {
        self.bytes("library".into(), "effects.json".into(), 0);
        let Some(library) = self.library.clone() else {
            return;
        };
        if let Some(uri) = library.engines.get(part) {
            self.bytes(format!("engine:{part}"), uri.clone(), 1);
        }
        let Some(layers) = self.engines.get(part).cloned() else {
            return;
        };
        for layer in layers.iter() {
            if let Some(mesh) = library.meshes.get(&layer.mesh) {
                self.bytes(format!("mesh:{}", layer.mesh), mesh.uri.clone(), 2);
            }
            let Some(material) = (if layer.kind == "mesh" {
                library.materials.get(&layer.material)
            } else {
                library.particle_materials.get(&layer.material)
            }) else {
                continue;
            };
            if let Some(shader) = library.shaders.get(&material.shader) {
                self.bytes(format!("shader:{}", material.shader), shader.uri.clone(), 3);
            }
            for key in material.textures.values() {
                self.texture(key.clone(), library.textures[key].raw_uri.clone());
            }
        }
    }
    pub fn ready_layers(&self) -> usize {
        self.draws.len()
    }
    pub fn asset_ready(&self, part: &str) -> bool {
        let Some(library) = &self.library else {
            return false;
        };
        let Some(layers) = self.engines.get(part) else {
            return false;
        };
        layers.iter().all(|l| {
            let material = if l.kind == "mesh" {
                library.materials.get(&l.material)
            } else {
                library.particle_materials.get(&l.material)
            };
            material.is_none_or(|m| m.textures.values().all(|t| self.textures.contains_key(t)))
                && (l.mesh.is_empty() || self.meshes.contains_key(&l.mesh))
                && (l.kind != "mesh"
                    || material.is_some_and(|m| self.shaders.contains_key(&m.shader)))
        })
    }
    pub fn reset_simulation(&mut self) {
        self.particle_system.clear();
        self.trail_system.clear();
        self.particle_clock.reset();
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        engines: &[EngineDraw],
        frame: &FxView,
    ) {
        let FxView {
            view_proj,
            view,
            near,
            width,
            height,
            camera,
            ..
        } = *frame;
        self.drain(device, queue);
        self.draws.clear();
        self.capture.clear();
        self.active_assets.clear();
        for engine in engines {
            self.request_engine(&engine.part);
            if engine.throttle > 0.001 && engine.reentry.is_none() && engine.vapor.is_none() {
                self.active_assets.push(engine.part.clone());
            }
        }
        if !engines.is_empty() {
            // Load crash recipes during normal flight, before the frame that needs their flash.
            let contextual: Vec<_> = self
                .library
                .as_ref()
                .map(|library| {
                    library
                        .engines
                        .keys()
                        .filter(|part| part.starts_with("fx_"))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for part in contextual {
                self.request_engine(&part);
            }
        }
        let Some(library) = self.library.clone() else {
            return;
        };
        for engine in engines {
            if let Some(body) = engine.reentry.and_then(|state| {
                ["", "Eve", "Jool", "Laythe"]
                    .get(state.gradient as usize)
                    .copied()
            }) {
                if let Some(id) = library.reentry_settings["GradientTextures"][body].as_str() {
                    self.texture(id.into(), library.textures[id].raw_uri.clone());
                }
            }
        }
        let mut reentry_records = Vec::new();
        for engine in engines {
            let Some(layers) = self.engines.get(&engine.part).cloned() else {
                continue;
            };
            if engine.throttle <= 0.001 {
                continue;
            }
            let intensity = engine
                .reentry
                .map(|s| crate::reentry::intensity(&library.reentry_settings, s))
                .unwrap_or(0.0);
            if engine.reentry.is_some() && intensity <= 0.0 {
                continue;
            }
            for (layer_index, layer) in layers.iter().enumerate().filter(|(_, l)| {
                l.kind == "mesh" && l.events.iter().any(|e| e[0] == engine.mode && e[1] == 10)
            }) {
                let material = &library.materials[&layer.material];
                let shader = &library.shaders[&material.shader];
                let gradient = engine
                    .reentry
                    .and_then(|state| {
                        ["", "Eve", "Jool", "Laythe"]
                            .get(state.gradient as usize)
                            .copied()
                    })
                    .and_then(|body| library.reentry_settings["GradientTextures"][body].as_str());
                if gradient.is_some_and(|id| !self.textures.contains_key(id)) {
                    continue;
                }
                let material_key = gradient
                    .map(|g| format!("{}:{g}", layer.material))
                    .unwrap_or_else(|| layer.material.clone());
                if !self.meshes.contains_key(&layer.mesh)
                    || !self.shaders.contains_key(&material.shader)
                    || material
                        .textures
                        .values()
                        .any(|t| !self.textures.contains_key(t))
                {
                    continue;
                }
                if !self.materials.contains_key(&material_key) {
                    let pick = |key| {
                        let id = if key == "_NoiseTexture" {
                            gradient.or_else(|| {
                                material
                                    .textures
                                    .get(key)
                                    .or_else(|| material.textures.get("_HeatGradient"))
                                    .map(String::as_str)
                            })
                        } else {
                            material
                                .textures
                                .get(key)
                                .or_else(|| {
                                    (key == "_DistortionTexture")
                                        .then(|| material.textures.get("_RefractionTex"))
                                        .flatten()
                                })
                                .map(String::as_str)
                        };
                        id.and_then(|t| self.textures.get(t)).unwrap_or(&self.white)
                    };
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&layer.material),
                        layout: &self.texture_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(pick(
                                    "_DistortionTexture",
                                )),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(pick("_NoiseTexture")),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(pick(
                                    "_TracesTexture",
                                )),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    });
                    self.materials.insert(material_key.clone(), bind);
                }
                let slot = self.draws.len();
                if slot >= self.slots.len() {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("plume uniforms"),
                        size: std::mem::size_of::<Uniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("plume uniforms"),
                        layout: &self.uniform_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    });
                    self.slots.push((buffer, bind));
                }
                let local = DMat4::from_cols_array(
                    &layer
                        .matrix
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap(),
                )
                .transpose();
                let model = engine.model * DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0)) * local;
                let mut uniform = Uniform {
                    globals: engine_fx::material_values(
                        material,
                        shader,
                        &layer.control,
                        engine.throttle,
                        engine.pressure,
                    ),
                    camera: [[0.0; 4]; 9],
                    object: [[0.0; 4]; 11],
                    frame: [[0.0; 4]; 23],
                    weights: engine_fx::weights(&layer.control, engine.throttle, engine.pressure),
                };
                // ThrottleBlendshapeData.OnEnable offsets every layer independently. A stable
                // per-instance seed preserves that variation without render-frame randomness.
                let mut seed =
                    engine.instance ^ (layer_index as u64).wrapping_mul(0x9e3779b97f4a7c15);
                for name in ["_TextureOffsetX", "_TextureOffsetY"] {
                    seed = seed.wrapping_add(0x9e3779b97f4a7c15);
                    let mut value = seed;
                    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
                    value ^= value >> 31;
                    if let Some([offset, _]) = shader.bindings.get(name) {
                        uniform.globals[offset / 4][offset % 4] = (value >> 40) as f32 / 16777216.0;
                    }
                }
                if let Some([offset, _]) = shader.bindings.get("_BendRotationOffset") {
                    uniform.globals[offset / 4][offset % 4] =
                        engine_fx::number(&layer.control, "bendRotationOffset", 0.0) as f32;
                }
                uniform.object[..4].copy_from_slice(&model.as_mat4().to_cols_array_2d());
                uniform.object[4..8].copy_from_slice(&model.inverse().as_mat4().to_cols_array_2d());
                uniform.frame[17..21].copy_from_slice(&view_proj.as_mat4().to_cols_array_2d());
                uniform.frame[9..13].copy_from_slice(&view.as_mat4().to_cols_array_2d());
                let t = engine.time as f32;
                uniform.camera[0] = [t / 20.0, t, t * 2.0, t * 3.0];
                uniform.camera[5] = [-1.0, near as f32, 1.0e9, 1.0e-9];
                uniform.camera[7] = [0.0, 0.0, 1.0 / near as f32, 0.0];
                if shader.reentry {
                    let Some(state) = engine.reentry else {
                        continue;
                    };
                    if let Some([offset, _]) = shader.bindings.get("ShockwaveDistance") {
                        uniform.globals[offset / 4][offset % 4] =
                            engine_fx::number(&layer.control, "shockwave_distance", 0.15) as f32;
                    }
                    reentry_records.push((
                        slot,
                        crate::reentry::vessel_data(&library.reentry_settings, state, intensity),
                    ));
                    while self.reentry_slots.len() <= slot {
                        self.reentry_slots
                            .push(self.reentry_gpu.as_ref().unwrap().slot(device));
                    }
                }
                if let Some([offset, _]) = shader.bindings.get("_CameraDepthTexture_TexelSize") {
                    uniform.globals[offset / 4] = [
                        1.0 / width as f32,
                        1.0 / height as f32,
                        width as f32,
                        height as f32,
                    ];
                }
                queue.write_buffer(&self.slots[slot].0, 0, bytemuck::bytes_of(&uniform));
                if self.capture_enabled {
                    self.capture.push(CaptureDraw {
                        mesh: layer.mesh.clone(),
                        material: layer.material.clone(),
                        shader: material.shader.clone(),
                        uniform: bytemuck::cast_slice(std::slice::from_ref(&uniform)).to_vec(),
                        refraction: shader.refraction,
                        reentry_data: Vec::new(),
                    });
                }
                self.draws.push(Draw {
                    mesh: layer.mesh.clone(),
                    material: material_key,
                    shader: material.shader.clone(),
                    slot,
                    refraction: shader.refraction,
                });
            }
        }
        if !reentry_records.is_empty() {
            let mut shapes = Vec::new();
            let mut heading = DVec3::Z;
            for engine in engines.iter().filter(|e| e.reentry.is_some()) {
                let state = engine.reentry.unwrap();
                if crate::reentry::intensity(&library.reentry_settings, state) <= 0.0 {
                    continue;
                }
                heading = state.heading;
                if let Some(layers) = self.engines.get(&engine.part) {
                    for layer in layers.iter().filter(|l| l.kind == "occlusion") {
                        if let Some(mesh) = self.meshes.get(&layer.mesh) {
                            let local = DMat4::from_cols_array(
                                &layer
                                    .matrix
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .unwrap(),
                            )
                            .transpose();
                            shapes.push(crate::reentry_gpu::Occluder {
                                vertices: &mesh.vertices,
                                indices: &mesh.indices,
                                count: mesh.count,
                                model: engine.model
                                    * DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0))
                                    * local,
                                bounds: library.meshes[&layer.mesh].bounds,
                            });
                        }
                    }
                }
            }
            let occlusion = self
                .reentry_gpu
                .as_mut()
                .unwrap()
                .prepare(device, queue, &shapes, heading);
            for (slot, vessel) in reentry_records {
                let mut data = Vec::from(vessel);
                data.extend(occlusion);
                if self.capture_enabled {
                    self.capture[slot].reentry_data = data.clone();
                }
                queue.write_buffer(&self.reentry_slots[slot].0, 0, bytemuck::cast_slice(&data));
            }
        }
        let mut inputs = Vec::new();
        let mut trail_inputs = Vec::new();
        for engine in engines {
            let Some(layers) = self.engines.get(&engine.part).cloned() else {
                continue;
            };
            for (index, layer) in layers.iter().enumerate().filter(|(_, l)| {
                (l.kind == "particle" || l.kind == "trail")
                    && !l.material.is_empty()
                    && l.events.iter().any(|e| e[0] == engine.mode)
            }) {
                let material = &library.particle_materials[&layer.material];
                // This source pass needs its own grab/depth shader. Never render it as an opaque
                // white billboard while the fire, flash, smoke and sparks use their authored maps.
                if material.shader.ends_with("/Distortion") {
                    continue;
                }
                if material
                    .textures
                    .values()
                    .any(|t| !self.textures.contains_key(t))
                    || (!layer.mesh.is_empty() && !self.meshes.contains_key(&layer.mesh))
                {
                    continue;
                }
                if !self.sprite_materials.contains_key(&layer.material) {
                    let pick = |key| {
                        material
                            .textures
                            .get(key)
                            .and_then(|t| self.textures.get(t))
                            .unwrap_or(&self.white)
                    };
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&layer.material),
                        layout: &self.texture_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(pick("_MainTex")),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(pick("_BumpMap")),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(pick("_EmissionMap")),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    });
                    self.sprite_materials.insert(layer.material.clone(), bind);
                }
                let local = DMat4::from_cols_array(
                    &layer
                        .matrix
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap(),
                )
                .transpose();
                let mut matrix =
                    engine.model * DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0)) * local;
                matrix.w_axis += camera.extend(0.0);
                if engine_fx::number(&layer.particle, "scalingMode", 0.0) == 1.0 {
                    matrix.x_axis = (matrix.x_axis.truncate().normalize_or_zero()
                        * layer.local_scale[0].abs())
                    .extend(0.0);
                    matrix.y_axis = (matrix.y_axis.truncate().normalize_or_zero()
                        * layer.local_scale[1].abs())
                    .extend(0.0);
                    matrix.z_axis = (matrix.z_axis.truncate().normalize_or_zero()
                        * layer.local_scale[2].abs())
                    .extend(0.0);
                }
                if layer.kind == "trail" {
                    trail_inputs.push(crate::authored_trails::Input {
                        key: (engine.instance, index),
                        layers: layers.clone(),
                        layer: index,
                        matrix,
                        intensity: engine.throttle
                            * engine
                                .vapor
                                .map(|v| crate::vapor::intensity(&library.vapor_settings, v))
                                .unwrap_or(1.0),
                    });
                    continue;
                }
                inputs.push(crate::authored_particles::Input {
                    key: (engine.instance, index),
                    layers: layers.clone(),
                    layer: index,
                    matrix,
                    throttle: engine.throttle,
                    commanded: engine.commanded,
                    pressure: engine.pressure,
                    mode: engine.mode,
                    context: crate::authored_particles::Context {
                        normalized_thrust: (engine.thrust_kn
                            / library
                                .context_settings
                                .get("MaxThrustForceForNormalizedParameterkN")
                                .copied()
                                .unwrap_or(1500.0))
                        .clamp(0.0, 1.0),
                        normalized_thrust_distance: (engine.thrust_kn
                            / library
                                .context_settings
                                .get("MaxThrustForceForNormalizedParameterkN")
                                .copied()
                                .unwrap_or(1500.0))
                        .clamp(0.0, 1.0)
                            * (1.0
                                - engine.distance_from_spawn
                                    / library
                                        .context_settings
                                        .get("MaxThrustForceDistanceForNormalizedParameterM")
                                        .copied()
                                        .unwrap_or(40.0))
                            .clamp(0.0, 1.0),
                        vertical_deviation: engine.vertical_deviation,
                        explosion_potential: 0.5, // KSP PartProperties' authored default.
                        atmosphere_density: engine.atmosphere_density,
                        relative_velocity: engine.relative_velocity,
                        gravity: -(engine.model.w_axis.truncate() + camera)
                            .try_normalize()
                            .unwrap_or(DVec3::Y)
                            * engine.gravity_mps2,
                    },
                });
            }
        }
        let dt = if let Some(first) = engines.first() {
            self.particle_clock
                .sample(first.simulation_time, first.time * 1000.0)
        } else {
            self.particle_clock.reset();
            self.particle_system.clear();
            self.trail_system.clear();
            0.0
        };
        self.particle_system.step(dt, &inputs);
        self.trail_system.step(dt, &trail_inputs);
        let inverse_view = view.inverse();
        let forward = -inverse_view.z_axis.truncate();
        let up = inverse_view.y_axis.truncate();
        let mut sprites = self.particle_system.sprites(camera, forward, up);
        sprites.extend(self.trail_system.sprites(camera));
        sprites
            .sort_unstable_by(|a, b| b.position.dot(forward).total_cmp(&a.position.dot(forward)));
        self.sprites
            .prepare(device, queue, &sprites, &library, frame);
    }
    pub fn capture(&self) -> &[CaptureDraw] {
        &self.capture
    }
    pub fn active_assets(&self) -> String {
        self.active_assets.join(",")
    }
    pub fn occlusion_texture(&self) -> Option<&wgpu::Texture> {
        self.reentry_gpu.as_ref().map(|g| &g.image)
    }
    pub fn enable_capture(&mut self) {
        self.capture_enabled = true;
    }
    pub fn particle_count(&self) -> usize {
        self.particle_system.particles.len() + self.trail_system.count()
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        for refraction in [true, false] {
            for draw in self.draws.iter().filter(|d| d.refraction == refraction) {
                let mesh = &self.meshes[&draw.mesh];
                pass.set_pipeline(&self.shaders[&draw.shader]);
                pass.set_bind_group(0, &self.slots[draw.slot].1, &[]);
                pass.set_bind_group(1, &self.materials[&draw.material], &[]);
                pass.set_bind_group(2, &self.screen, &[]);
                if self.library.as_ref().unwrap().shaders[&draw.shader].reentry {
                    pass.set_bind_group(3, &self.reentry_slots[draw.slot].1, &[]);
                }
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
        }
        for batch in &self.sprites.batches {
            let mesh = self.sprites.begin(pass, batch);
            pass.set_bind_group(1, &self.sprite_materials[&batch.material], &[]);
            pass.set_bind_group(2, &self.screen, &[]);
            if mesh {
                let geometry = &self.meshes[&batch.mesh];
                pass.set_vertex_buffer(0, geometry.vertices.slice(..));
                pass.set_index_buffer(geometry.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..geometry.count, 0, batch.start..batch.end);
            } else {
                pass.draw(0..6, batch.start..batch.end);
            }
        }
    }
}
