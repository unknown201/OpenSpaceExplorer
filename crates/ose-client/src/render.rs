//! WebGPU renderer (wgpu) for the 3D views: textured meshes from the exported GLB files (`/assets/...`),
//! line strips for orbits, a camera-relative world so star-space distances fit in f32.
//!
//! World units are kilometres. Every draw gets a model matrix whose translation is `(position - camera)`,
//! computed in f64 by the caller, so precision near the camera is what matters and the system's 1e8 km extent
//! never reaches the GPU. Models load asynchronously: `request_model` starts a fetch, and a model that is not
//! ready yet is simply not drawn that frame.

use glam::{DMat4, DQuat, DVec3, Mat4, Vec3, Vec4};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    /// Direction towards the light in world (camera-relative) space, w: share of the light reaching the
    /// object (1 in full sun, less in an eclipse).
    light_dir: [f32; 4],
    /// Second, fill light: direction towards it (xyz) and intensity (w).
    light2: [f32; 4],
    /// x: emissive strength (1 = unlit, show the texture as is), y: use texture (1) or colour (0),
    /// z: 1 draws an atmosphere shell (rim glow from `color`, alpha by view angle), w: ambient level.
    params: [f32; 4],
    /// x: how strongly the material's emissive map lights up (window lights at night), yzw unused.
    extra: [f32; 4],
}

const SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32> };
struct Object { model: mat4x4<f32>, color: vec4<f32>, light_dir: vec4<f32>, light2: vec4<f32>, params: vec4<f32>, extra: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> object: Object;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var samp: sampler;
@group(2) @binding(2) var normal_tex: texture_2d<f32>;
@group(2) @binding(3) var mg_tex: texture_2d<f32>;
@group(2) @binding(4) var em_tex: texture_2d<f32>;

struct VsIn { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) world: vec3<f32> };

@vertex
fn vs_mesh(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world;
    // Animated rigs can contain nonuniform scale. Cofactors give the inverse-transpose normal
    // transform; the determinant sign preserves outward normals on mirrored draws.
    let a = object.model[0].xyz;
    let b = object.model[1].xyz;
    let c = object.model[2].xyz;
    let cofactor = mat3x3<f32>(cross(b,c), cross(c,a), cross(a,b));
    out.normal = normalize((cofactor * in.normal) * sign(dot(a,cross(b,c))));
    out.uv = in.uv;
    out.world = world.xyz;
    return out;
}

@fragment
fn fs_mesh(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(tex, samp, in.uv);
    let nm = textureSample(normal_tex, samp, in.uv);
    let mg = textureSample(mg_tex, samp, in.uv);
    let em = textureSample(em_tex, samp, in.uv).rgb * object.extra.x;
    let base = mix(object.color, texel * object.color, object.params.y);
    let n0 = normalize(in.normal);
    // Normal map: the original's maps are DXT5nm-style (x in alpha, y in green). The tangent frame comes
    // from screen-space derivatives of position and uv, so mirrored and flipped uvs work as they are; the
    // exporter flipped v, hence the minus on y.
    let dp1 = dpdx(in.world);
    let dp2 = dpdy(in.world);
    let duv1 = dpdx(in.uv);
    let duv2 = dpdy(in.uv);
    let dp2perp = cross(dp2, n0);
    let dp1perp = cross(n0, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let len2 = max(dot(t, t), dot(b, b));
    let tx = nm.a * 2.0 - 1.0;
    let ty = -(nm.g * 2.0 - 1.0);
    let tz = sqrt(max(1.0 - tx * tx - ty * ty, 0.0));
    var n = n0;
    if len2 > 1e-20 {
        let inv = inverseSqrt(len2);
        n = normalize(t * inv * tx + b * inv * ty + n0 * tz);
    }
    // The camera sits at the origin of the relative world.
    let view = normalize(-in.world);
    let ldir = normalize(object.light_dir.xyz);
    let diffuse = max(dot(n, ldir), 0.0) * object.light_dir.w;
    if object.params.z > 0.5 {
        // Atmosphere shell: a rim that brightens towards the limb and on the day side, see-through
        // elsewhere. The alpha of the colour sets the strength.
        let rim = pow(1.0 - clamp(dot(n, view), 0.0, 1.0), 3.0);
        let day = 0.15 + 0.85 * diffuse;
        return vec4<f32>(object.color.rgb * day, object.color.a * rim * day);
    }
    let fill = max(dot(n, normalize(object.light2.xyz)), 0.0) * object.light2.w;
    let ambient = max(object.params.w, 0.08);
    let lit = base.rgb * min(ambient + (1.0 - ambient) * diffuse + fill, 1.6);
    // Specular from the metallic-gloss map (metal in red, smoothness in alpha): a Blinn-Phong lobe whose
    // sharpness follows smoothness, tinted by the base colour on metal.
    let metal = mg.r * 0.8;
    let smooth_ = mg.a;
    let h = normalize(ldir + view);
    let spec_pow = mix(8.0, 256.0, smooth_ * smooth_);
    let spec = pow(max(dot(n, h), 0.0), spec_pow) * smooth_ * diffuse * (spec_pow + 8.0) / 64.0;
    let spec_col = mix(vec3<f32>(0.04), base.rgb, metal);
    let shaded = lit * (1.0 - metal * 0.5) + spec_col * spec;
    let rgb = mix(shaded, base.rgb, object.params.x) + em;
    return vec4<f32>(rgb, base.a);
}

struct LineIn { @location(0) pos: vec3<f32> };
struct LineOut { @builtin(position) clip: vec4<f32> };

@vertex
fn vs_line(in: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = camera.view_proj * (object.model * vec4<f32>(in.pos, 1.0));
    return out;
}

@fragment
fn fs_line(_in: LineOut) -> @location(0) vec4<f32> {
    return object.color;
}
"#;

/// A mesh primitive on the GPU plus the material texture it uses.
struct GpuPrimitive {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// Indices into `Model::textures` of the base colour, normal and metallic-gloss images.
    textures: [Option<usize>; 4],
    /// Bind group for those textures (defaults where missing or not loaded yet); rebuilt as images arrive.
    bind: Option<wgpu::BindGroup>,
    /// glTF `alphaMode: BLEND`: drawn after the opaque primitives without writing depth.
    blend: bool,
    /// The material's base colour factor, multiplied into the draw colour.
    color: [f32; 4],
}

/// A loaded GLB: each mesh primitive uploaded once, the node instances that draw them with their flattened
/// transforms, and textures by image index.
pub struct Model {
    meshes: Vec<GpuPrimitive>,
    /// (index into `meshes`, world transform within the model) per drawn node.
    primitives: Vec<(usize, Mat4, usize)>,
    pub rig: crate::animation::Rig,
    rest_vertices: Vec<Vec<Vertex>>,
    influences: Vec<Option<Influences>>,
    textures: Vec<Option<wgpu::TextureView>>,
    /// glTF mesh name of each entry of `meshes` (for `hide_meshes`).
    mesh_names: Vec<String>,
    /// Radius of the mesh in its own units (planet spheres), from the GLB extras.
    pub mesh_radius: f64,
    /// Bounds of all primitives in model units, for framing in the editor.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
}

impl Model {
    /// Pose bounds include skin deformation, so unfolding legs do not use the bind-pose ground anchor.
    pub fn pose_bounds(&self, sample: Option<crate::animation::ClipSample>) -> (Vec3, Vec3) {
        if self.rest_vertices.iter().all(Vec::is_empty) {
            return (self.bounds_min, self.bounds_max);
        }
        let world = self.rig.world_matrices(&self.rig.pose(sample));
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (mesh, _, node) in &self.primitives {
            let palette = self.rig.palette(*node, &world);
            for (i, v) in self.rest_vertices[*mesh].iter().enumerate() {
                let mut p = Vec3::from(v.pos);
                if let (Some(palette), Some(influences)) = (&palette, &self.influences[*mesh]) {
                    let (j, w) = influences[i];
                    let mut skinned = Vec3::ZERO;
                    let mut total = 0.0;
                    for (j, w) in j.into_iter().zip(w) {
                        if w > 0.0 {
                            if let Some(m) = palette.get(j as usize) {
                                skinned += m.transform_point3(p) * w;
                                total += w;
                            }
                        }
                    }
                    if total > 1e-12 {
                        p = skinned / total;
                    }
                }
                let p = world[*node].transform_point3(p);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        if lo.is_finite() {
            (lo, hi)
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        }
    }
}

struct PendingTexture {
    model: String,
    index: usize,
    bitmap: web_sys::ImageBitmap,
    /// Data texture (normal, metallic-gloss): uploaded without sRGB decoding.
    linear: bool,
}

/// Parsed GLB before GPU upload.
/// Vertices, indices, base-colour / normal / metallic-gloss image indices, alpha blending and base colour
/// factor of one mesh primitive before upload.
type CpuMesh = (Vec<Vertex>, Vec<u32>, [Option<usize>; 4], bool, [f32; 4]);
type Influences = Vec<([u16; 4], [f32; 4])>;

struct CpuModel {
    meshes: Vec<CpuMesh>,
    /// Per image: true when it is used as data (normal or metallic-gloss) rather than colour.
    image_linear: Vec<bool>,
    /// (mesh index, node transform) per instance.
    primitives: Vec<(usize, Mat4, usize)>,
    rig: crate::animation::Rig,
    influences: Vec<Option<Influences>>,
    image_uris: Vec<String>,
    mesh_radius: f64,
    /// glTF mesh name of each entry of `meshes`.
    mesh_names: Vec<String>,
}

pub struct DrawItem {
    pub animation: Option<crate::animation::ClipSample>,
    /// Extra turns on named nodes after the animation pose (a control surface on its hinge bone).
    pub node_turns: Vec<(&'static str, glam::Quat)>,
    pub node_rotations: Vec<(usize, glam::Quat)>,
    pub model: String,
    /// Position relative to the camera, kilometres.
    pub rel_pos_km: DVec3,
    pub rotation: DQuat,
    pub scale: f64,
    /// Direction towards the light, relative space.
    pub light_dir: DVec3,
    /// 1 for self-lit bodies (the star), 0 for lit surfaces.
    pub emissive: f32,
    pub color: [f32; 4],
    /// Fill light: direction towards it and intensity (0 for none).
    pub fill_dir: DVec3,
    pub fill: f32,
    /// Ambient level (0 keeps the default 0.08).
    pub ambient: f32,
    /// True draws the model as an atmosphere shell: `color` is the rim colour, its alpha the strength.
    pub atmosphere: bool,
    /// Mirror the model along its Z axis (celestial body spheres: their UVs run west-to-east the other way
    /// round from a right-handed frame, see flight3d).
    pub mirror_z: bool,
    /// Share of the light reaching the model, 0..1: 1 in full sun, less while a body eclipses the star.
    pub sun: f32,
    /// Emissive map strength, 0 (off) to 1: the KSC's window and flood lights at night.
    pub glow: f32,
}

impl DrawItem {
    /// A lit, textured model with no fill light, the common case.
    pub fn lit(
        model: String,
        rel_pos_km: DVec3,
        rotation: DQuat,
        scale: f64,
        light_dir: DVec3,
    ) -> DrawItem {
        DrawItem {
            animation: None,
            node_turns: Vec::new(),
            node_rotations: Vec::new(),
            model,
            rel_pos_km,
            rotation,
            scale,
            light_dir,
            emissive: 0.0,
            color: [1.0; 4],
            fill_dir: DVec3::ZERO,
            fill: 0.0,
            ambient: 0.0,
            atmosphere: false,
            mirror_z: false,
            sun: 1.0,
            glow: 0.0,
        }
    }
}

pub struct LineItem {
    /// Cache key for the vertex buffer (orbits are constant in their parent's frame).
    pub key: String,
    /// Points in the strip's own frame, kilometres; only used when the key is not cached yet.
    pub points_km: Vec<DVec3>,
    /// Strip origin relative to the camera, kilometres.
    pub rel_origin_km: DVec3,
    pub color: [f32; 4],
}

type ModelInbox = Rc<RefCell<Vec<(String, CpuModel)>>>;

/// Extra drawing inside the main pass (planet terrain, ocean, sky, clouds: `planet.rs`). It owns its pipelines,
/// built for `Renderer::format()` and a `Depth32Float` reversed-depth attachment.
pub trait ExtraPass {
    /// Before the meshes: sky and opaque ground.
    fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>);
    /// After the opaque meshes, before blended ones: water and clouds.
    fn draw_transparent(&self, pass: &mut wgpu::RenderPass<'_>);
}

#[derive(Default)]
pub struct FrameEffects<'a> {
    pub camera: DVec3,
    pub engines: &'a [crate::engine_fx_gpu::EngineDraw],
    pub extra: Option<&'a dyn ExtraPass>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    mesh_pipeline: wgpu::RenderPipeline,
    /// Same as `mesh_pipeline` but culling front faces, for mirrored instances (negative determinant).
    mirror_pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    engine_fx: crate::engine_fx_gpu::EngineFxGpu,
    post: crate::fx_post::FxPost,
    view: DMat4,
    near: f64,
    fixture_grid: bool,
    camera_forward: DVec3,
    camera_up: DVec3,
    camera_buf: wgpu::Buffer,
    camera_bind: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    /// Default texture views: white colour, flat normal, no metal or gloss.
    default_views: [wgpu::TextureView; 4],
    depth: wgpu::TextureView,
    objects: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    models: HashMap<String, Model>,
    pending: HashMap<String, ()>,
    inbox: ModelInbox,
    texture_inbox: Rc<RefCell<Vec<PendingTexture>>>,
    lines: HashMap<String, (wgpu::Buffer, u32)>,
    view_proj: DMat4,
    /// Meshes (by glTF mesh name) left out of a model for the next frame only, see `hide_meshes`.
    hidden: HashMap<String, Vec<String>>,
    pub width: u32,
    pub height: u32,
}

fn log(s: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(s));
}

impl Renderer {
    /// Create the device and surface for `canvas`. `None` when the browser has no WebGPU adapter.
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Option<Renderer> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::BROWSER_WEBGPU;
        let instance = wgpu::Instance::new(desc);
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| log(&format!("webgpu surface: {e}")))
            .ok()?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| log(&format!("webgpu adapter: {e}")))
            .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ose"),
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| log(&format!("webgpu device: {e}")))
            .ok()?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            color_space: wgpu::SurfaceColorSpace::Auto,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let surface_format = format;
        let format = crate::fx_post::FORMAT;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ose shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[uniform_entry(0)],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[uniform_entry(0)],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ose"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&object_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let depth_state = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            // Reversed depth (near 1, far 0) for precision from metres to thousands of kilometres.
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mirror_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-mirror"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Front),
                ..Default::default()
            },
            depth_stencil: Some(depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-blend"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                ..depth_state.clone()
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineStrip,
                strip_index_format: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                ..depth_state
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let solid = |rgba: [u8; 4], format: wgpu::TextureFormat| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("default"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &t,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
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
            t.create_view(&wgpu::TextureViewDescriptor::default())
        };
        let default_views = [
            solid([255, 255, 255, 255], wgpu::TextureFormat::Rgba8UnormSrgb),
            // Flat normal in the DXT5nm layout: x (alpha) and y (green) at the middle.
            solid([255, 128, 128, 128], wgpu::TextureFormat::Rgba8Unorm),
            solid([0, 0, 0, 0], wgpu::TextureFormat::Rgba8Unorm),
            // No emission.
            solid([0, 0, 0, 255], wgpu::TextureFormat::Rgba8UnormSrgb),
        ];
        let white = Self::bind_views(
            &device,
            &texture_layout,
            &sampler,
            [
                &default_views[0],
                &default_views[1],
                &default_views[2],
                &default_views[3],
            ],
        );
        let depth = Self::make_depth(&device, width, height);
        let post = crate::fx_post::FxPost::new(&device, width, height, surface_format);
        let engine_fx =
            crate::engine_fx_gpu::EngineFxGpu::new(&device, &queue, &depth, &post.snapshot_view);
        Some(Renderer {
            device,
            queue,
            surface,
            config,
            mesh_pipeline,
            mirror_pipeline,
            blend_pipeline,
            line_pipeline,
            engine_fx,
            post,
            view: DMat4::IDENTITY,
            near: 0.3,
            fixture_grid: false,
            camera_forward: -DVec3::Z,
            camera_up: DVec3::Y,
            camera_buf,
            camera_bind,
            object_layout,
            texture_layout,
            sampler,
            white,
            default_views,
            depth,
            objects: Vec::new(),
            models: HashMap::new(),
            pending: HashMap::new(),
            inbox: Rc::new(RefCell::new(Vec::new())),
            texture_inbox: Rc::new(RefCell::new(Vec::new())),
            lines: HashMap::new(),
            view_proj: DMat4::IDENTITY,
            hidden: HashMap::new(),
            width,
            height,
        })
    }

    fn bind_views(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        views: [&wgpu::TextureView; 4],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(views[3]),
                },
            ],
        })
    }

    /// (Re)build the material bind groups of a model from the textures it has so far.
    fn rebind_model(&mut self, name: &str) {
        let Some(m) = self.models.get_mut(name) else {
            return;
        };
        for prim in &mut m.meshes {
            if prim.textures.iter().all(|t| t.is_none()) {
                prim.bind = None;
                continue;
            }
            let pick = |k: usize| -> &wgpu::TextureView {
                prim.textures[k]
                    .and_then(|i| m.textures.get(i))
                    .and_then(|v| v.as_ref())
                    .unwrap_or(&self.default_views[k])
            };
            let views = [pick(0), pick(1), pick(2), pick(3)];
            prim.bind = Some(Self::bind_views(
                &self.device,
                &self.texture_layout,
                &self.sampler,
                views,
            ));
        }
    }

    fn make_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = Self::make_depth(&self.device, width, height);
        self.post = crate::fx_post::FxPost::new(&self.device, width, height, self.config.format);
        self.engine_fx
            .resize(&self.device, &self.depth, &self.post.snapshot_view);
    }

    /// Models still loading.
    pub fn models_loading(&self) -> usize {
        self.pending.len()
    }

    pub fn has_model(&self, name: &str) -> bool {
        self.models.contains_key(name)
    }

    pub fn model(&self, name: &str) -> Option<&Model> {
        self.models.get(name)
    }

    /// Start loading a GLB from `url` under `name` (no-op if loaded or in flight).
    pub fn request_model(&mut self, name: &str, url: &str) {
        if self.models.contains_key(name) || self.pending.contains_key(name) {
            return;
        }
        self.pending.insert(name.to_string(), ());
        let inbox = self.inbox.clone();
        let name = name.to_string();
        let url = url.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            match fetch_bytes(&url).await {
                Ok(bytes) => match parse_glb(&bytes) {
                    Ok(model) => inbox.borrow_mut().push((name, model)),
                    Err(e) => log(&format!("glb {url}: {e}")),
                },
                Err(e) => log(&format!("fetch {url}: {e}")),
            }
        });
    }

    /// Upload models and textures that finished loading since the last frame.
    fn drain_inbox(&mut self, asset_base: &str) {
        let ready: Vec<(String, CpuModel)> = self.inbox.borrow_mut().drain(..).collect();
        for (name, cpu) in ready {
            self.pending.remove(&name);
            let mut meshes = Vec::new();
            let mut local_bounds: Vec<(Vec3, Vec3)> = Vec::new();
            for (vertices, indices, textures, blend, color) in &cpu.meshes {
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for v in vertices {
                    lo = lo.min(Vec3::from(v.pos));
                    hi = hi.max(Vec3::from(v.pos));
                }
                local_bounds.push((lo, hi));
                let vb = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("vertices"),
                    size: (vertices.len() * std::mem::size_of::<Vertex>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&vb, 0, bytemuck::cast_slice(vertices));
                let ib = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("indices"),
                    size: (indices.len() * 4) as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&ib, 0, bytemuck::cast_slice(indices));
                meshes.push(GpuPrimitive {
                    vertices: vb,
                    indices: ib,
                    index_count: indices.len() as u32,
                    textures: *textures,
                    bind: None,
                    blend: *blend,
                    color: *color,
                });
            }
            let mut bounds_min = Vec3::splat(f32::INFINITY);
            let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
            for (mi, transform, _) in &cpu.primitives {
                let Some((lo, hi)) = local_bounds.get(*mi) else {
                    continue;
                };
                if !lo.is_finite() {
                    continue;
                }
                for corner in 0..8 {
                    let c = Vec3::new(
                        if corner & 1 == 0 { lo.x } else { hi.x },
                        if corner & 2 == 0 { lo.y } else { hi.y },
                        if corner & 4 == 0 { lo.z } else { hi.z },
                    );
                    let p = transform.transform_point3(c);
                    bounds_min = bounds_min.min(p);
                    bounds_max = bounds_max.max(p);
                }
            }
            let primitives = cpu.primitives.clone();
            let mesh_names = cpu.mesh_names.clone();
            let textures = vec![None; cpu.image_uris.len()];
            for (i, uri) in cpu.image_uris.iter().enumerate() {
                let url = resolve_uri(asset_base, uri);
                let inbox = self.texture_inbox.clone();
                let model = name.clone();
                let linear = cpu.image_linear.get(i).copied().unwrap_or(false);
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_bitmap(&url).await {
                        Ok(bitmap) => inbox.borrow_mut().push(PendingTexture {
                            model,
                            index: i,
                            bitmap,
                            linear,
                        }),
                        Err(e) => log(&format!("texture {url}: {e}")),
                    }
                });
            }
            if !bounds_min.is_finite() {
                bounds_min = Vec3::ZERO;
                bounds_max = Vec3::ZERO;
            }
            let retain_vertices = !cpu.rig.skins.is_empty() || !cpu.rig.clips.is_empty();
            let rest_vertices = cpu
                .meshes
                .iter()
                .map(|m| {
                    if retain_vertices {
                        m.0.clone()
                    } else {
                        Vec::new()
                    }
                })
                .collect();
            let mut model = Model {
                rig: cpu.rig,
                rest_vertices,
                influences: cpu.influences,
                meshes,
                primitives,
                textures,
                mesh_names,
                mesh_radius: cpu.mesh_radius,
                bounds_min,
                bounds_max,
            };
            if !model.rig.skins.is_empty() || model.rig.deployment.is_some() {
                (model.bounds_min, model.bounds_max) =
                    model.pose_bounds(model.rig.default_sample());
            }
            self.models.insert(name, model);
        }
        let textures: Vec<PendingTexture> = self.texture_inbox.borrow_mut().drain(..).collect();
        let mut touched: Vec<String> = Vec::new();
        for t in textures {
            let view = self.upload_bitmap(&t.bitmap, t.linear);
            if let Some(m) = self.models.get_mut(&t.model) {
                if t.index < m.textures.len() {
                    m.textures[t.index] = Some(view);
                    if !touched.contains(&t.model) {
                        touched.push(t.model.clone());
                    }
                }
            }
        }
        for name in touched {
            self.rebind_model(&name);
        }
    }

    fn upload_bitmap(&self, bitmap: &web_sys::ImageBitmap, linear: bool) -> wgpu::TextureView {
        let (w, h) = (bitmap.width().max(1), bitmap.height().max(1));
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let mips = (32 - w.max(h).leading_zeros()).max(1);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if linear {
                wgpu::TextureFormat::Rgba8Unorm
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let _ = mips;
        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: false,
            },
            size,
        );
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Make sure at least `n` object uniform slots exist (grown ahead of a frame so draws only borrow).
    fn ensure_slots(&mut self, n: usize) {
        while self.objects.len() < n {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("object"),
                size: std::mem::size_of::<ObjectUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("object"),
                layout: &self.object_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            self.objects.push((buf, bind));
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Colour attachment format of the main pass.
    pub fn format(&self) -> wgpu::TextureFormat {
        crate::fx_post::FORMAT
    }

    /// The camera's view-projection (camera at the origin of the relative world) set by `set_camera`.
    pub fn view_proj(&self) -> DMat4 {
        self.view_proj
    }

    /// Leave the named meshes of `model` out of the next frame (the flight view hides the KSC's flat ground
    /// plane where the planet's terrain is drawn).
    pub fn hide_meshes(&mut self, model: &str, names: &[&str]) {
        self.hidden.insert(
            model.to_string(),
            names.iter().map(|n| n.to_string()).collect(),
        );
    }

    /// Set the camera: it sits at the origin of the relative world looking along `forward` with `up`.
    /// `near` and `far` in kilometres.
    pub fn set_camera(&mut self, forward: DVec3, up: DVec3, fov_y_rad: f64, near: f64, far: f64) {
        self.camera_forward = forward.normalize();
        self.camera_up = up.normalize();
        let aspect = self.width as f64 / self.height.max(1) as f64;
        let view = look_to_rh(forward.normalize(), up.normalize());
        self.view = view;
        self.near = near;
        let proj = perspective_rh(fov_y_rad, aspect, near, far);
        self.view_proj = proj * view;
        let u = CameraUniform {
            view_proj: self.view_proj.as_mat4().to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&u));
    }

    /// Screen position in pixels of a camera-relative point, if in front of the camera.
    pub fn project(&self, rel_km: DVec3) -> Option<(f64, f64)> {
        let clip = self.view_proj * rel_km.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some((
            (ndc.x * 0.5 + 0.5) * self.width as f64,
            (0.5 - ndc.y * 0.5) * self.height as f64,
        ))
    }

    /// Draw one frame. `asset_base` is the URL prefix for relative texture URIs (e.g. `assets/parts/`).
    pub fn frame(
        &mut self,
        asset_base: &str,
        items: &[DrawItem],
        lines: &[LineItem],
        clear: [f64; 3],
    ) {
        self.frame_with(asset_base, items, lines, clear, None);
    }

    /// `frame` with extra drawing from `extra` inside the same pass.
    pub fn frame_with(
        &mut self,
        asset_base: &str,
        items: &[DrawItem],
        lines: &[LineItem],
        clear: [f64; 3],
        extra: Option<&dyn ExtraPass>,
    ) {
        self.frame_with_effects(
            asset_base,
            items,
            lines,
            clear,
            FrameEffects {
                extra,
                ..Default::default()
            },
        );
    }
    pub fn frame_effects(
        &mut self,
        asset_base: &str,
        items: &[DrawItem],
        lines: &[LineItem],
        clear: [f64; 3],
        camera: DVec3,
        engines: &[crate::engine_fx_gpu::EngineDraw],
    ) {
        self.frame_with_effects(
            asset_base,
            items,
            lines,
            clear,
            FrameEffects {
                camera,
                engines,
                ..Default::default()
            },
        );
    }
    pub fn frame_with_effects(
        &mut self,
        asset_base: &str,
        items: &[DrawItem],
        lines: &[LineItem],
        clear: [f64; 3],
        effects: FrameEffects<'_>,
    ) {
        let FrameEffects {
            camera,
            engines,
            extra,
        } = effects;
        self.drain_inbox(asset_base);
        self.engine_fx.prepare(
            &self.device,
            &self.queue,
            engines,
            &crate::engine_fx_gpu::FxView {
                view_proj: self.view_proj,
                view: self.view,
                near: self.near,
                width: self.width,
                height: self.height,
                camera,
                light: items.first().map(|i| i.light_dir).unwrap_or(DVec3::Y),
            },
        );
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Upload line strips not seen before.
        for l in lines {
            if !self.lines.contains_key(&l.key) && !l.points_km.is_empty() {
                let data: Vec<[f32; 3]> = l
                    .points_km
                    .iter()
                    .map(|p| [p.x as f32, p.y as f32, p.z as f32])
                    .collect();
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("line"),
                    size: (data.len() * 12) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&buf, 0, bytemuck::cast_slice(&data));
                self.lines.insert(l.key.clone(), (buf, data.len() as u32));
            }
        }

        // Object uniforms: one slot per mesh primitive draw and per line strip.
        let needed: usize = items
            .iter()
            .map(|i| {
                self.models
                    .get(&i.model)
                    .map(|m| m.primitives.len())
                    .unwrap_or(0)
            })
            .sum::<usize>()
            + lines.len();
        self.ensure_slots(needed);
        let mut slot = 0usize;
        // (model, primitive, slot, mirrored)
        let mut mesh_draws: Vec<(String, usize, usize, bool, Option<wgpu::Buffer>)> = Vec::new();
        let hidden = std::mem::take(&mut self.hidden);
        for item in items {
            let Some(model) = self.models.get(&item.model) else {
                continue;
            };
            let hide = hidden.get(&item.model);
            let base = DMat4::from_scale_rotation_translation(
                if item.mirror_z {
                    DVec3::new(item.scale, item.scale, -item.scale)
                } else {
                    DVec3::splat(item.scale)
                },
                item.rotation,
                item.rel_pos_km,
            );
            let mut pose = model
                .rig
                .pose(item.animation.or_else(|| model.rig.default_sample()));
            for (node, turn) in &item.node_turns {
                if let Some(i) = model.rig.nodes.iter().position(|n| n.name == *node) {
                    pose[i].rotation = (pose[i].rotation * *turn).normalize();
                }
            }
            for (i, turn) in &item.node_rotations {
                pose[*i].rotation = (pose[*i].rotation * *turn).normalize();
            }
            let world = model.rig.world_matrices(&pose);
            for (pi, (mesh_i, _, node)) in model.primitives.iter().enumerate() {
                if hide.is_some_and(|h| h.contains(&model.mesh_names[*mesh_i])) {
                    continue;
                }
                let m = base * world[*node].as_dmat4();
                // Each skinned draw owns its buffer. Writing into the shared model's buffer would make
                // every instance display the last instance's pose when the command buffer executes.
                let skinned = model.influences[*mesh_i].as_ref().and_then(|influences| {
                    let palette = model.rig.palette(*node, &world)?;
                    let vertices: Vec<Vertex> = model.rest_vertices[*mesh_i]
                        .iter()
                        .zip(influences)
                        .map(|(v, (joints, weights))| {
                            let (p, n) = crate::animation::skin_vertex(
                                v.pos.into(),
                                v.normal.into(),
                                *joints,
                                *weights,
                                &palette,
                            );
                            Vertex {
                                pos: p.to_array(),
                                normal: n.to_array(),
                                uv: v.uv,
                            }
                        })
                        .collect();
                    let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("skinned instance"),
                        size: std::mem::size_of_val(vertices.as_slice()) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.queue
                        .write_buffer(&buf, 0, bytemuck::cast_slice(&vertices));
                    Some(buf)
                });
                let fill = item.fill_dir.normalize_or_zero().as_vec3();
                let factor = model.meshes[*mesh_i].color;
                let u = ObjectUniform {
                    model: m.as_mat4().to_cols_array_2d(),
                    color: [
                        item.color[0] * factor[0],
                        item.color[1] * factor[1],
                        item.color[2] * factor[2],
                        item.color[3] * factor[3],
                    ],
                    light_dir: item
                        .light_dir
                        .normalize_or_zero()
                        .extend(item.sun as f64)
                        .as_vec4()
                        .to_array(),
                    light2: [fill.x, fill.y, fill.z, item.fill],
                    params: [
                        item.emissive,
                        1.0,
                        if item.atmosphere { 1.0 } else { 0.0 },
                        item.ambient,
                    ],
                    extra: [item.glow, 0.0, 0.0, 0.0],
                };
                self.queue
                    .write_buffer(&self.objects[slot].0, 0, bytemuck::bytes_of(&u));
                mesh_draws.push((item.model.clone(), pi, slot, m.determinant() < 0.0, skinned));
                slot += 1;
            }
        }
        // Forget strips that are no longer drawn (trajectory patches get new keys when they change).
        let used: std::collections::HashSet<&str> = lines.iter().map(|l| l.key.as_str()).collect();
        self.lines.retain(|k, _| used.contains(k.as_str()));
        let mut line_draws: Vec<(String, usize)> = Vec::new();
        for l in lines {
            if !self.lines.contains_key(&l.key) {
                continue;
            }
            let u = ObjectUniform {
                model: DMat4::from_translation(l.rel_origin_km)
                    .as_mat4()
                    .to_cols_array_2d(),
                color: l.color,
                light_dir: [0.0; 4],
                light2: [0.0; 4],
                params: [1.0, 0.0, 0.0, 0.0],
                extra: [0.0; 4],
            };
            self.queue
                .write_buffer(&self.objects[slot].0, 0, bytemuck::bytes_of(&u));
            line_draws.push((l.key.clone(), slot));
            slot += 1;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Callers give the background as it should look on screen (sRGB); the scene is linear.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(clear[0]),
                            g: srgb_to_linear(clear[1]),
                            b: srgb_to_linear(clear[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(x) = extra {
                x.draw_opaque(&mut pass);
            }
            pass.set_bind_group(0, &self.camera_bind, &[]);
            for (blended, mirrored) in [(false, false), (false, true), (true, false)] {
                if blended {
                    if let Some(x) = extra {
                        x.draw_transparent(&mut pass);
                        pass.set_bind_group(0, &self.camera_bind, &[]);
                    }
                }
                pass.set_pipeline(if blended {
                    &self.blend_pipeline
                } else if mirrored {
                    &self.mirror_pipeline
                } else {
                    &self.mesh_pipeline
                });
                for (model_name, pi, s, m, skinned) in &mesh_draws {
                    let model = &self.models[model_name];
                    let (mesh_i, _, _) = model.primitives[*pi];
                    let prim = &model.meshes[mesh_i];
                    if prim.blend != blended || (!blended && *m != mirrored) {
                        continue;
                    }
                    let tex = prim.bind.as_ref().unwrap_or(&self.white);
                    pass.set_bind_group(1, &self.objects[*s].1, &[]);
                    pass.set_bind_group(2, tex, &[]);
                    pass.set_vertex_buffer(0, skinned.as_ref().unwrap_or(&prim.vertices).slice(..));
                    pass.set_index_buffer(prim.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..prim.index_count, 0, 0..1);
                }
            }
            pass.set_pipeline(&self.line_pipeline);
            pass.set_bind_group(2, &self.white, &[]);
            for (key, s) in &line_draws {
                let (buf, n) = &self.lines[key];
                pass.set_bind_group(1, &self.objects[*s].1, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*n, 0..1);
            }
        }
        if self.fixture_grid {
            self.post.reference_grid(&mut encoder);
        }
        self.post.snapshot(&mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("authored engine effects"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.engine_fx.draw(&mut pass);
        }
        self.post.finish(&mut encoder, &view);
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(output);
    }

    pub fn engine_fx_layers(&self) -> usize {
        self.engine_fx.ready_layers()
    }
    pub fn engine_particle_count(&self) -> usize {
        self.engine_fx.particle_count()
    }
    pub fn effect_assets(&self) -> String {
        self.engine_fx.active_assets()
    }
    pub(crate) fn fixture_fx_ready(&self, part: &str) -> bool {
        self.engine_fx.asset_ready(part)
    }
    pub(crate) fn fixture_fx_reset(&mut self) {
        self.engine_fx.reset_simulation();
    }
    pub fn enable_fx_capture(&mut self) {
        self.engine_fx.enable_capture();
        self.fixture_grid = true;
    }

    pub(crate) fn set_fixture_grid(&mut self, enabled: bool) {
        self.fixture_grid = enabled;
    }
    pub fn engine_fx_capture(&self) -> String {
        serde_json::to_string(self.engine_fx.capture()).unwrap_or_default()
    }

    /// Linear HDR readback for comparing the WGSL port with the original compiled D3D shaders.
    pub async fn read_fx_pixels(&self) -> Result<Vec<u8>, String> {
        self.read_effect_texture(&self.post.scene, self.width, self.height, 8)
            .await
    }
    pub async fn read_reentry_pixels(&self) -> Result<Vec<u8>, String> {
        self.read_effect_texture(
            self.engine_fx
                .occlusion_texture()
                .ok_or("reentry not loaded")?,
            2048,
            2048,
            2,
        )
        .await
    }
    async fn read_effect_texture(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        pixel_bytes: u32,
    ) -> Result<Vec<u8>, String> {
        let row = (width * pixel_bytes).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FX parity readback"),
            size: (row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            buffer.map_async(wgpu::MapMode::Read, .., move |result| match result {
                Ok(()) => {
                    let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
                }
                Err(e) => {
                    let _ = reject.call1(&wasm_bindgen::JsValue::NULL, &e.to_string().into());
                }
            });
        });
        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|e| format!("{e:?}"))?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let mut output = Vec::with_capacity((width * height * pixel_bytes) as usize);
        for line in mapped.chunks(row as usize) {
            output.extend_from_slice(&line[..width as usize * pixel_bytes as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(output)
    }
}

/// Right-handed view matrix for a camera at the origin looking along `dir`.
fn look_to_rh(dir: DVec3, up: DVec3) -> DMat4 {
    let f = dir.normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    DMat4::from_cols(
        glam::DVec4::new(s.x, u.x, -f.x, 0.0),
        glam::DVec4::new(s.y, u.y, -f.y, 0.0),
        glam::DVec4::new(s.z, u.z, -f.z, 0.0),
        glam::DVec4::new(0.0, 0.0, 0.0, 1.0),
    )
}

/// Right-handed perspective projection, reversed and infinite: depth 1 at `near`, falling towards 0 at
/// infinity (WebGPU clip space, compared with `Greater`). `far` is kept in the signature for callers.
fn perspective_rh(fov_y: f64, aspect: f64, near: f64, _far: f64) -> DMat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    DMat4::from_cols(
        glam::DVec4::new(f / aspect, 0.0, 0.0, 0.0),
        glam::DVec4::new(0.0, f, 0.0, 0.0),
        glam::DVec4::new(0.0, 0.0, 0.0, -1.0),
        glam::DVec4::new(0.0, 0.0, near, 0.0),
    )
}

fn resolve_uri(base: &str, uri: &str) -> String {
    // GLB images point at "../textures/x.png" relative to assets/parts/ or assets/bodies/.
    if uri.starts_with("http") || uri.starts_with('/') {
        return uri.to_string();
    }
    let mut parts: Vec<&str> = base.trim_end_matches('/').split('/').collect();
    for seg in uri.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub(crate) async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a response")?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let buf =
        wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?)
            .await
            .map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

pub(crate) async fn fetch_bitmap(url: &str) -> Result<web_sys::ImageBitmap, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a response")?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let blob = wasm_bindgen_futures::JsFuture::from(resp.blob().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let blob: web_sys::Blob = blob.dyn_into().map_err(|_| "not a blob")?;
    let bitmap = wasm_bindgen_futures::JsFuture::from({
        let opts = web_sys::ImageBitmapOptions::new();
        opts.set_premultiply_alpha(web_sys::PremultiplyAlpha::None);
        opts.set_color_space_conversion(web_sys::ColorSpaceConversion::None);
        window
            .create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &opts)
            .map_err(|e| format!("{e:?}"))?
    })
    .await
    .map_err(|e| format!("{e:?}"))?;
    bitmap
        .dyn_into::<web_sys::ImageBitmap>()
        .map_err(|_| String::from("not a bitmap"))
}

// ---------------------------------------------------------------------------------------------------------------
// GLB parsing (the subset tools/export_web_assets.py writes)

fn parse_glb(bytes: &[u8]) -> Result<CpuModel, String> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return Err("not a GLB".into());
    }
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json_end = 20usize.checked_add(json_len).ok_or("GLB length overflow")?;
    let json: serde_json::Value =
        serde_json::from_slice(bytes.get(20..json_end).ok_or("truncated GLB JSON")?)
            .map_err(|e| e.to_string())?;
    let bin_start = json_end.checked_add(8).ok_or("GLB length overflow")?;
    let bin = bytes.get(bin_start..).unwrap_or(&[]);
    let views = json["bufferViews"].as_array().cloned().unwrap_or_default();
    let accessors = json["accessors"].as_array().cloned().unwrap_or_default();
    let access = crate::animation::Accessors { json: &json, bin };
    let read_f32 = |acc: usize, ncomp: usize| access.floats(acc, ncomp);
    let read_indices = |acc: usize| -> Result<Vec<u32>, String> {
        let a = accessors.get(acc).ok_or("accessor")?;
        let v = &views[a["bufferView"].as_u64().ok_or("view")? as usize];
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let count = a["count"].as_u64().ok_or("count")? as usize;
        match a["componentType"].as_u64() {
            Some(5125) => Ok(bin
                .get(off..off + count * 4)
                .ok_or("indices")?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect()),
            Some(5123) => Ok(bin
                .get(off..off + count * 2)
                .ok_or("indices")?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c) as u32)
                .collect()),
            _ => Err("index type".into()),
        }
    };
    // Material -> base colour image index.
    let textures = json["textures"].as_array().cloned().unwrap_or_default();
    let material_image = |mi: usize| -> Option<usize> {
        let m = json["materials"].as_array()?.get(mi)?;
        let ti = m["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64()? as usize;
        textures.get(ti)?["source"].as_u64().map(|s| s as usize)
    };
    let material_data_image = |mi: usize, which: &str| -> Option<usize> {
        let m = json["materials"].as_array()?.get(mi)?;
        let slot = if which == "normal" {
            &m["normalTexture"]
        } else if which == "emissive" {
            &m["emissiveTexture"]
        } else {
            &m["pbrMetallicRoughness"]["metallicRoughnessTexture"]
        };
        let ti = slot["index"].as_u64()? as usize;
        textures.get(ti)?["source"].as_u64().map(|s| s as usize)
    };
    let material_color = |mi: usize| -> [f32; 4] {
        json["materials"]
            .as_array()
            .and_then(|a| a.get(mi))
            .and_then(|m| m["pbrMetallicRoughness"]["baseColorFactor"].as_array())
            .map(|f| {
                let g = |i: usize| f.get(i).and_then(|v| v.as_f64()).unwrap_or(1.0);
                // The exports carry the original's colours as authored, in sRGB (Unity converts them to
                // linear in a linear-space project); the scene is lit in linear light.
                let lin = |i: usize| srgb_to_linear(g(i)) as f32;
                [lin(0), lin(1), lin(2), g(3) as f32]
            })
            .unwrap_or([1.0; 4])
    };
    let material_blend = |mi: usize| -> bool {
        json["materials"]
            .as_array()
            .and_then(|a| a.get(mi))
            .map(|m| m["alphaMode"].as_str() == Some("BLEND"))
            .unwrap_or(false)
    };
    let image_uris: Vec<String> = json["images"]
        .as_array()
        .map(|imgs| {
            imgs.iter()
                .map(|i| i["uri"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();

    let rig = crate::animation::Rig::from_gltf(&json, bin)?;
    let world = rig.world_matrices(&rig.pose(None));
    let placed: Vec<_> = rig
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.mesh.filter(|_| n.visible).map(|m| (m, world[i], i)))
        .collect();
    let access = crate::animation::Accessors { json: &json, bin };
    let meshes = json["meshes"].as_array().cloned().unwrap_or_default();
    let mut cpu_meshes: Vec<CpuMesh> = Vec::new();
    let mut by_key: HashMap<(usize, usize), usize> = HashMap::new();
    let mut primitives: Vec<(usize, Mat4, usize)> = Vec::new();
    let mut influences = Vec::new();
    let mut mesh_names: Vec<String> = Vec::new();
    for (mi, world, node) in placed {
        let Some(mesh) = meshes.get(mi) else { continue };
        for (pi, prim) in mesh["primitives"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if let Some(&idx) = by_key.get(&(mi, pi)) {
                primitives.push((idx, world, node));
                continue;
            }
            let pos = read_f32(
                prim["attributes"]["POSITION"].as_u64().ok_or("POSITION")? as usize,
                3,
            )?;
            let nrm = prim["attributes"]["NORMAL"]
                .as_u64()
                .map(|a| read_f32(a as usize, 3))
                .transpose()?;
            let uv = prim["attributes"]["TEXCOORD_0"]
                .as_u64()
                .map(|a| read_f32(a as usize, 2))
                .transpose()?;
            let indices = read_indices(prim["indices"].as_u64().ok_or("indices")? as usize)?;
            let n = pos.len() / 3;
            if nrm.as_ref().is_some_and(|a| a.len() != n * 3)
                || uv.as_ref().is_some_and(|a| a.len() != n * 2)
                || indices.iter().any(|i| *i as usize >= n)
            {
                return Err("mesh attribute or index bounds".into());
            }
            let joints = prim["attributes"]["JOINTS_0"].as_u64();
            let weights = prim["attributes"]["WEIGHTS_0"].as_u64();
            let skin = match (joints, weights) {
                (Some(j), Some(w)) => {
                    let j = access.floats(j as usize, 4)?;
                    let w = access.floats(w as usize, 4)?;
                    if j.len() != n * 4
                        || w.len() != n * 4
                        || j.iter()
                            .any(|v| *v < 0.0 || *v > 65535.0 || v.fract() != 0.0)
                    {
                        return Err("skin attributes".into());
                    }
                    Some(
                        j.as_chunks::<4>()
                            .0
                            .iter()
                            .zip(w.as_chunks::<4>().0.iter())
                            .map(|(j, w)| {
                                (
                                    [j[0] as u16, j[1] as u16, j[2] as u16, j[3] as u16],
                                    [w[0], w[1], w[2], w[3]],
                                )
                            })
                            .collect(),
                    )
                }
                (None, None) => None,
                _ => return Err("skin needs joints and weights".into()),
            };
            influences.push(skin);
            let vertices: Vec<Vertex> = (0..n)
                .map(|i| Vertex {
                    pos: [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]],
                    normal: nrm
                        .as_ref()
                        .map(|a| [a[i * 3], a[i * 3 + 1], a[i * 3 + 2]])
                        .unwrap_or([0.0, 1.0, 0.0]),
                    uv: uv
                        .as_ref()
                        .map(|a| [a[i * 2], a[i * 2 + 1]])
                        .unwrap_or([0.0, 0.0]),
                })
                .collect();
            let mat = prim["material"].as_u64().map(|m| m as usize);
            let texture = [
                mat.and_then(material_image),
                mat.and_then(|m| material_data_image(m, "normal")),
                mat.and_then(|m| material_data_image(m, "mg")),
                mat.and_then(|m| material_data_image(m, "emissive")),
            ];
            let blend = prim["material"]
                .as_u64()
                .map(|m| material_blend(m as usize))
                .unwrap_or(false);
            let color = prim["material"]
                .as_u64()
                .map(|m| material_color(m as usize))
                .unwrap_or([1.0; 4]);
            cpu_meshes.push((vertices, indices, texture, blend, color));
            mesh_names.push(mesh["name"].as_str().unwrap_or("").to_string());
            by_key.insert((mi, pi), cpu_meshes.len() - 1);
            primitives.push((cpu_meshes.len() - 1, world, node));
        }
    }
    let mesh_radius = json["extras"]["mesh_radius"].as_f64().unwrap_or(1.0);
    let mut image_linear = vec![false; image_uris.len()];
    for (_, _, tex, _, _) in &cpu_meshes {
        for k in [1, 2] {
            if let Some(i) = tex[k] {
                if i < image_linear.len() {
                    image_linear[i] = true;
                }
            }
        }
    }
    Ok(CpuModel {
        rig,
        influences,
        meshes: cpu_meshes,
        image_linear,
        primitives,
        image_uris,
        mesh_radius,
        mesh_names,
    })
}

/// Unit sphere-ish fallback is not needed: bodies without an exported GLB are drawn by the 2D map instead.
pub fn unused() -> Vec4 {
    Vec4::ZERO
}

/// One sRGB-encoded channel (0..1, as picked on screen) as linear light.
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
