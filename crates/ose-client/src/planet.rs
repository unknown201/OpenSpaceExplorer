//! Planet rendering up close, after the original's PQS terrain, ocean, Bruneton atmosphere and scaled cloud
//! shell (docs/reference-notes/planet-rendering.md): the GPU side. `planet_lod` picks and builds the terrain
//! quads; this module loads the body's exported assets (`tools/export_planets.py`, `assets/planets/<Body>/`),
//! uploads quads, and draws sky, terrain, ocean and clouds inside the renderer's main pass (`ExtraPass`).
//!
//! Frame: the flight view's body model frame (metres, camera-relative for geometry). Quads are kept per body and
//! built a few per frame, nearest first; until a quad's children exist the quad itself is drawn.

use crate::ocean_fft;
use crate::planet_lod::{self, QuadKey, TerrainVertex};
use crate::planet_scatter::{
    surface_distance, Exclusion, Far, FarPlacement, Grass, Instance, ScatterFile, FAR_RANGE_M,
};
use crate::render::{fetch_bitmap, fetch_bytes, DrawItem, ExtraPass, Renderer};
use glam::{DMat4, DVec3};
use ose_data::terrain::{TerrainIndex, TerrainMap, TerrainPack};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const SHADER: &str = include_str!("planet.wgsl");

/// Lifts every body's exposure towards the original's brighter frame (see `prepare`).
const EXPOSURE_GAIN: f64 = 1.35;
const BASE: &str = "assets/planets";
/// The sea's deep colour from a body's `_DiffuseColor`, scaled by one factor for every body (the one that
/// brings Kerbin's to the luminance of the shared `_DeepWaterColor` default (0, 0.34, 0.62), so darker seas
/// such as Eve's stay darker), its saturation doubled and its blue raised: Kerbin's then comes out at that default's azure,
/// as the original's seas look from 90 km in Josh's screenshots, Eve's a deep violet.
fn ocean_deep_colour(diffuse: [f32; 4]) -> [f32; 4] {
    const K: f32 = 2.2;
    const SATURATION: f32 = 2.0;
    let c = [diffuse[0] * K, diffuse[1] * K, diffuse[2] * K];
    let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let s = |v: f32| (lum + (v - lum) * SATURATION).max(0.0);
    // and shifted towards blue (green x 0.8, blue x 1.25): Kerbin's comes out near the default's azure (0.04,
    // 0.28, 0.64 against 0, 0.34, 0.62), which reads as the original's sea at 90 km once lit through the air
    [s(c[0]), s(c[1]) * 0.8, s(c[2]) * 1.25, 1.0]
}

/// Depth (m) where the sea surface stops showing the floor: a quarter of `_TransparentVisibleDepth`, 0.5 to 25.
fn ocean_opaque_depth(visible_m: Option<f64>) -> f32 {
    (visible_m.unwrap_or(100.0) * 0.25).clamp(0.5, 25.0) as f32
}

/// Fog distance under water (m): 60 % of `_TransparentVisibleDepth`, 2 to 120 (Kerbin 60 m, Laythe 120 m, Eve 2 m).
fn underwater_view_m(visible_m: Option<f64>) -> f32 {
    (visible_m.unwrap_or(100.0) * 0.6).clamp(2.0, 120.0) as f32
}

/// Segments around a ring, as `RING_SEGMENTS` in the shader.
const RING_SEGMENTS: u32 = 256;

/// Tiling period of detail textures and waves (m), as `TILE_PERIOD` in the shader.
const TILE_PERIOD: f64 = 4096.0;
/// Quad builds allowed per frame, in milliseconds of CPU time.
const BUILD_BUDGET_MS: f64 = 6.0;
/// Distant scatter must never rebuild a whole dense biome inside one frame.
const SCATTER_BUDGET_MS: f64 = 1.5;
const SLOT: u64 = 256;
/// Level of the cloud shell's quads (96 quads).
const CLOUD_LEVEL: u8 = 2;

fn log(s: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(s));
}

thread_local! {
    /// Settings > Graphics > Planet detail: 0 low, 1 medium, 2 high (lib.rs sets it from storage).
    static QUALITY: std::cell::Cell<u8> = const { std::cell::Cell::new(2) };
}

/// Planet detail. Low draws no volume clouds, terrain shadows, per-pixel relief, FFT sea, light shafts, grass or
/// far impostors; medium halves the shadow and cloud march steps and the grass range.
pub fn set_quality(q: u8) {
    QUALITY.with(|c| c.set(q.min(2)));
}

fn quality() -> u8 {
    QUALITY.with(|c| c.get())
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameU {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    cam_km: [f32; 4],
    sun: [f32; 4],
    atmo: [f32; 4],
    rayleigh: [f32; 4],
    mie: [f32; 4],
    solar: [f32; 4],
    /// x: camera height above the sea surface (m), y: 1 on ocean worlds
    water: [f32; 4],
    /// the water's colour under the surface
    water_col: [f32; 4],
    /// up to four bodies that can hide the sun: centre (km, model frame), radius (km)
    occluders: [[f32; 4]; 4],
    /// x: the star's angular radius (rad), y: number of occluders
    eclipse: [f32; 4],
    /// FFT waves: x 1 when the wave texture is live, y/z the long and short patch sizes (m), w: chop
    waves: [f32; 4],
    /// The star skybox's frame in the model frame: x axis (towards the sun, in the equatorial plane), z axis.
    star_x: [f32; 4],
    star_z: [f32; 4],
    /// the camera's model-frame position modulo TILE_PERIOD (m), for screen-space effects on the tiled maps
    cam_tile: [f32; 4],
    /// planet detail: x terrain shadow steps, y volume cloud steps, z per-pixel relief (0/1)
    quality: [f32; 4],
    /// The meshes' sun shadow map (render.rs `set_shadow`): view-projection from camera-relative metres, and
    /// x 1 when in use, y one texel in uv, z normal offset (m).
    shadow_vp: [[f32; 4]; 4],
    mesh_shadow: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ChunkU {
    offset: [f32; 4],
    center: [f32; 4],
    tile: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SurfaceU {
    slots: [[f32; 4]; 4],
    height_enable: [[f32; 4]; 4],
    slope_enable: [[f32; 4]; 4],
    height_params: [[f32; 4]; 16],
    slope_params: [[f32; 4]; 16],
    uv_scale: [[f32; 4]; 4],
    albedo_fade: [f32; 4],
    normal_fade: [f32; 4],
    deep: [f32; 4],
    shallow: [f32; 4],
    clouds: [f32; 4],
    flags: [f32; 4],
    ambient_day: [f32; 4],
    ambient_night: [f32; 4],
    /// Shoreline foam: max depth (m), tiling (m), has foam texture.
    foam: [f32; 4],
    /// Caustics on the sea floor: strength, max visible depth (m), tiling (m), animation speed.
    caustics: [f32; 4],
    /// Cloud layer top (m), wind speed (m/s), flow speed, flow strength.
    clouds2: [f32; 4],
    /// Polar caps (tools/export_poles.py): has albedo caps, has cloud caps, cap radius (rad), 0.
    poles: [f32; 4],
    /// The terrain pack's eight tiled height layers for per-pixel relief: (uv scale, height scale) pairs.
    layers: [[f32; 4]; 4],
    /// x: 1 with layer slopes, y: side/top projection contrast.
    layer_info: [f32; 4],
    /// Terrain shadows from the global height map: x 1 when uploaded, y its scale (m), z its offset (m).
    shadow: [f32; 4],
    /// x: the cloud colour map's intensity (clouds.png RGB; 1 for plain white clouds).
    cloud_col: [f32; 4],
}

fn flag_f(x: bool) -> f32 {
    if x {
        1.0
    } else {
        0.0
    }
}

/// What the flight view tells the planet each frame.
pub struct PlanetView<'a> {
    pub body_id: &'a str,
    pub radius_m: f64,
    /// Camera position in the body model frame (m).
    pub cam: DVec3,
    /// Direction to the sun in the model frame.
    pub sun: DVec3,
    /// Seconds, for animation (waves).
    pub time_s: f64,
    /// Other bodies that can hide the sun (eclipses): centre in the model frame (m) and radius (m).
    pub occluders: &'a [(DVec3, f64)],
    /// The star's angular radius seen from here (rad), for partial eclipses.
    pub sun_radius_rad: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Tex {
    Albedo,
    Normal,
    Biome,
    DetailA,
    DetailN,
    Clouds,
    Foam,
    Grass,
    Impostor,
    PoleAlbedo,
    GlobalHeight,
    Slopes,
    Ring,
    LayerMask,
}

enum Loaded {
    Info(serde_json::Value),
    NoInfo,
    Image(Tex, web_sys::ImageBitmap),
    Layer(Tex, u32, web_sys::ImageBitmap),
    Lut(usize, Vec<u8>),
    /// The body's terrain pack (`assets/terrain/<Body>`, the server's own heights), or `None` without one.
    Pack(Option<Box<(TerrainPack, Vec<u8>)>>),
    Scatter(ScatterFile),
}

/// The shared height maps (index and blob) once fetched.
type SharedTerrain = Rc<RefCell<Option<(TerrainIndex, Vec<u8>)>>>;

struct GpuQuad {
    center: DVec3,
    terrain: wgpu::Buffer,
    ocean: Option<wgpu::Buffer>,
    last_used: u64,
    /// When it was built (ms): a quad that arrives late (the camera is already well inside its range, as in a fast
    /// descent) slides in from its parent's surface instead of popping.
    built_ms: f64,
}

/// How long a newly built quad takes to morph in from its parent (ms).
const QUAD_FADE_MS: f64 = 400.0;

struct Body {
    info: Option<serde_json::Value>,
    info_done: bool,
    pack: Option<Box<(TerrainPack, Vec<u8>)>>,
    pack_done: bool,
    map: Option<TerrainMap>,
    /// No height map exists for this body: draw the plain sphere.
    no_terrain: bool,
    scatter: Option<ScatterFile>,
    /// Placed props and the camera position they were placed for.
    placed: Vec<Instance>,
    placed_at: Option<DVec3>,
    /// Grass patches (instance buffer, count) and the camera position they were placed around.
    grass: Option<(wgpu::Buffer, u32)>,
    grass_at: Option<DVec3>,
    grass_bind: Option<wgpu::BindGroup>,
    /// Far props as impostors (instance buffer, count), placed around `far_at`.
    far: Option<(wgpu::Buffer, u32)>,
    far_at: Option<DVec3>,
    far_pending: Option<FarPlacement>,
    far_bind: Option<wgpu::BindGroup>,
    min_h: f64,
    max_h: f64,
    quads: HashMap<QuadKey, GpuQuad>,
    /// Quads whose shape changed (the campus exclusions moved): still drawn, rebuilt in place as the budget
    /// allows, so the ground never opens up or blinks while they are redone.
    stale: std::collections::HashSet<QuadKey>,
    clouds: HashMap<QuadKey, GpuQuad>,
    tex: HashMap<Tex, wgpu::TextureView>,
    /// Detail arrays: texture, layers expected, layers received.
    arrays: HashMap<Tex, (wgpu::Texture, u32, u32)>,
    luts: [Option<wgpu::TextureView>; 3],
    /// The pack layers' (uv scale, height scale) pairs and projection contrast, once their slopes are uploaded.
    layer_params: Option<([[f32; 4]; 4], f32)>,
    /// The ring (Dres): its own bind group (radii uniform, texture), once the texture is in.
    ring_bind: Option<wgpu::BindGroup>,
    inbox: Rc<RefCell<Vec<Loaded>>>,
    surface_buf: wgpu::Buffer,
    surface_bind: Option<wgpu::BindGroup>,
    frame_bind: Option<wgpu::BindGroup>,
    /// The frame group without the shadow map, for grass and far impostors.
    frame_plain_bind: Option<wgpu::BindGroup>,
    dirty: bool,
}

pub struct PlanetRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sky_pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    ocean_pipeline: wgpu::RenderPipeline,
    cloud_pipeline: wgpu::RenderPipeline,
    vcloud_pipeline: wgpu::RenderPipeline,
    shaft_pipeline: wgpu::RenderPipeline,
    /// The original's skybox (`skybox_spacescape_starfield`), drawn behind the sky: its bitmap until uploaded.
    star_layout: wgpu::BindGroupLayout,
    ring_layout: wgpu::BindGroupLayout,
    ring_pipeline: wgpu::RenderPipeline,
    star_bind: wgpu::BindGroup,
    stars: Rc<RefCell<Option<web_sys::ImageBitmap>>>,
    /// the camera is under the sea this frame: light shafts, and the sea surface without its skirts (they hang
    /// below it and would read as walls from underneath)
    under_water: bool,
    /// draw the underwater light shafts (not on low detail)
    shafts: bool,
    /// sunlight scale for props and their impostors, matching the terrain's exposure
    prop_sun: f32,
    grass_pipeline: wgpu::RenderPipeline,
    far_pipeline: wgpu::RenderPipeline,
    far_chunk_buf: wgpu::Buffer,
    far_chunk_bind: wgpu::BindGroup,
    grass_layout: wgpu::BindGroupLayout,
    /// Grass placement origin minus camera (a chunk uniform of its own).
    grass_chunk_buf: wgpu::Buffer,
    grass_chunk_bind: wgpu::BindGroup,
    frame_layout: wgpu::BindGroupLayout,
    frame_plain_layout: wgpu::BindGroupLayout,
    /// The meshes' sun shadow map and its comparison sampler (render.rs), received by the terrain.
    shadow_map: (wgpu::TextureView, wgpu::Sampler),
    surface_layout: wgpu::BindGroupLayout,
    chunk_layout: wgpu::BindGroupLayout,
    frame_buf: wgpu::Buffer,
    chunk_buf: wgpu::Buffer,
    chunk_bind: wgpu::BindGroup,
    chunk_capacity: u64,
    index_buf: wgpu::Buffer,
    index_count: u32,
    lut_sampler: wgpu::Sampler,
    wrap_sampler: wgpu::Sampler,
    equi_sampler: wgpu::Sampler,
    mip: Mipmapper,
    defaults: HashMap<Tex, wgpu::TextureView>,
    default_luts: [wgpu::TextureView; 3],
    /// FFT ocean: the packed wave texture (ocean_fft.rs) and the body whose spectrum it runs.
    waves_tex: wgpu::Texture,
    waves_view: wgpu::TextureView,
    ocean: Option<(String, ocean_fft::OceanFft)>,
    terrain: SharedTerrain,
    terrain_requested: bool,
    bodies: HashMap<String, Body>,
    current: Option<String>,
    frame_no: u64,
    /// This frame's draws: quad and chunk uniform slot, for terrain/ocean and for the cloud shell.
    draws: Vec<(QuadKey, u32)>,
    cloud_draws: Vec<(QuadKey, u32)>,
    active: bool,
    /// Shader debug view (preview `debug=`): 0 off, 1 biome mask, 2 detail ratio, 3 detail inputs, 4 albedo,
    /// 5 sunlight, 6 normals, 7 layer relief slopes.
    pub debug: f32,
    /// Multiplies the exposure (preview `exp=`, for tuning).
    pub exposure_scale: f32,
    /// Ground kept free of scatter (the KSC campus), set by the flight view.
    exclusions: Vec<Exclusion>,
}

impl PlanetRenderer {
    pub fn new(gpu: &Renderer) -> PlanetRenderer {
        let device = gpu.device().clone();
        let queue = gpu.queue().clone();
        let format = gpu.format();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("planet"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let tex_entry = |binding, dim| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let samp_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        use wgpu::TextureViewDimension as D;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet frame"),
            entries: &[
                uniform_entry(0, false),
                tex_entry(1, D::D2),
                tex_entry(2, D::D3),
                tex_entry(3, D::D2),
                samp_entry(4),
                tex_entry(5, D::D2Array),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: D::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        // Grass and far impostors already sample 16 textures in the fragment stage, the limit: their frame
        // group leaves the meshes' shadow map out.
        let frame_plain_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("planet frame (no shadow)"),
                entries: &[
                    uniform_entry(0, false),
                    tex_entry(1, D::D2),
                    tex_entry(2, D::D3),
                    tex_entry(3, D::D2),
                    samp_entry(4),
                    tex_entry(5, D::D2Array),
                ],
            });
        let chunk_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet chunk"),
            entries: &[uniform_entry(0, true)],
        });
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet surface"),
            entries: &[
                uniform_entry(0, false),
                tex_entry(1, D::D2),
                tex_entry(2, D::D2),
                tex_entry(3, D::D2),
                tex_entry(4, D::D2Array),
                tex_entry(5, D::D2Array),
                tex_entry(6, D::D2),
                samp_entry(7),
                samp_entry(8),
                tex_entry(9, D::D2),
                tex_entry(10, D::D2),
                tex_entry(11, D::D2),
                tex_entry(12, D::D2Array),
                tex_entry(13, D::D2),
            ],
        });
        // The sky's own group 1: the star skybox (bindings 1 and 2, clear of the quads' chunk uniform at 0).
        let star_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet stars"),
            entries: &[tex_entry(1, D::D2), samp_entry(2)],
        });
        // Rings (Dres): group 1 of their own (radii uniform, radial texture, sampler at 3..5, clear of the chunk's 0
        // and the stars' 1..2).
        let ring_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet ring"),
            entries: &[uniform_entry(3, false), tex_entry(4, D::D2), samp_entry(5)],
        });
        let ring_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("planet ring"),
            bind_group_layouts: &[Some(&frame_layout), Some(&ring_layout)],
            immediate_size: 0,
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("planet sky"),
            bind_group_layouts: &[Some(&frame_layout), Some(&star_layout)],
            immediate_size: 0,
        });
        let quad_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("planet quads"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&chunk_layout),
                Some(&surface_layout),
            ],
            immediate_size: 0,
        });
        let depth = |write: bool, compare| wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let vertex_attrs = [
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
                format: wgpu::VertexFormat::Float32,
                offset: 24,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 28,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 32,
                shader_location: 4,
            },
        ];
        let vbl = [Some(wgpu::VertexBufferLayout {
            array_stride: 44,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vertex_attrs,
        })];
        let quad_pipeline = |label, vs, fs, blend: bool, write: bool, cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&quad_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &vbl,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(depth(write, wgpu::CompareFunction::Greater)),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let terrain_pipeline = quad_pipeline(
            "planet terrain",
            "vs_quad",
            "fs_terrain",
            false,
            true,
            Some(wgpu::Face::Back),
        );
        // no culling: the sea is seen from below when the camera is under water
        let ocean_pipeline =
            quad_pipeline("planet ocean", "vs_ocean", "fs_ocean", true, true, None);
        let cloud_pipeline =
            quad_pipeline("planet clouds", "vs_quad", "fs_clouds", true, false, None);
        // Volumetric clouds: a full-screen ray march through the cloud layer, writing the depth of where it
        // enters the cloud so nearer craft and terrain stay in front; premultiplied colour.
        // Underwater light shafts: a full-screen march through the water in front of the camera, added on top.
        let shaft_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet light shafts"),
            layout: Some(&quad_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_shafts"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth(false, wgpu::CompareFunction::Greater)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let vcloud_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet volume clouds"),
            layout: Some(&quad_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_vclouds"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth(false, wgpu::CompareFunction::Greater)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let grass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("planet grass"),
            entries: &[tex_entry(0, D::D2Array), samp_entry(1)],
        });
        let grass_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("planet grass"),
                bind_group_layouts: &[
                    Some(&frame_plain_layout),
                    Some(&chunk_layout),
                    Some(&surface_layout),
                    Some(&grass_layout),
                ],
                immediate_size: 0,
            });
        let grass_attrs = [
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
                format: wgpu::VertexFormat::Float32x4,
                offset: 24,
                shader_location: 2,
            },
        ];
        let grass_vbl = [Some(wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &grass_attrs,
        })];
        let grass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet grass"),
            layout: Some(&grass_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_grass"),
                compilation_options: Default::default(),
                buffers: &grass_vbl,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_grass"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth(true, wgpu::CompareFunction::Greater)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let (grass_chunk_buf, grass_chunk_bind) = Self::make_chunks(&device, &chunk_layout, 1);
        let far_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 28,
                shader_location: 2,
            },
        ];
        let far_vbl = [Some(wgpu::VertexBufferLayout {
            array_stride: 44,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &far_attrs,
        })];
        let far_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet impostors"),
            layout: Some(&grass_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_far"),
                compilation_options: Default::default(),
                buffers: &far_vbl,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_far"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth(true, wgpu::CompareFunction::Greater)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let (far_chunk_buf, far_chunk_bind) = Self::make_chunks(&device, &chunk_layout, 1);
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet sky"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth(false, wgpu::CompareFunction::Always)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planet frame"),
            size: std::mem::size_of::<FrameU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let chunk_capacity = 1024;
        let (chunk_buf, chunk_bind) = Self::make_chunks(&device, &chunk_layout, chunk_capacity);
        let indices = planet_lod::indices();
        let index_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planet indices"),
            size: (indices.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buf, 0, bytemuck::cast_slice(&indices));
        let sampler = |u, v, mip| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("planet"),
                address_mode_u: u,
                address_mode_v: v,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: mip,
                anisotropy_clamp: if mip == wgpu::MipmapFilterMode::Linear {
                    8
                } else {
                    1
                },
                ..Default::default()
            })
        };
        use wgpu::AddressMode as A;
        let lut_sampler = sampler(
            A::ClampToEdge,
            A::ClampToEdge,
            wgpu::MipmapFilterMode::Nearest,
        );
        let wrap_sampler = sampler(A::Repeat, A::Repeat, wgpu::MipmapFilterMode::Linear);
        let equi_sampler = sampler(A::Repeat, A::ClampToEdge, wgpu::MipmapFilterMode::Linear);
        let mip = Mipmapper::new(&device);
        let solid = |rgba: [u8; 4], srgb: bool, array: bool| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("planet default"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if srgb {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                t.as_image_copy(),
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(if array { D::D2Array } else { D::D2 }),
                ..Default::default()
            })
        };
        let mut defaults = HashMap::new();
        defaults.insert(Tex::Albedo, solid([128, 128, 128, 255], true, false));
        defaults.insert(Tex::Normal, solid([128, 128, 255, 128], false, false));
        defaults.insert(Tex::Biome, solid([0, 0, 0, 0], false, false));
        defaults.insert(Tex::DetailA, solid([128, 128, 128, 128], true, true));
        defaults.insert(Tex::DetailN, solid([128, 128, 255, 128], false, true));
        defaults.insert(Tex::Clouds, solid([255, 255, 255, 0], true, false));
        defaults.insert(Tex::Foam, solid([0, 0, 0, 255], false, false));
        defaults.insert(Tex::PoleAlbedo, solid([128, 128, 128, 255], true, false));
        defaults.insert(Tex::GlobalHeight, solid([0, 0, 0, 0], false, false));
        defaults.insert(Tex::Slopes, solid([0, 0, 0, 0], false, true));
        defaults.insert(Tex::LayerMask, solid([0, 0, 0, 0], false, false));
        let lut = |w, h, d| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("planet lut default"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: d,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: if d > 1 {
                    wgpu::TextureDimension::D3
                } else {
                    wgpu::TextureDimension::D2
                },
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(if d > 1 { D::D3 } else { D::D2 }),
                ..Default::default()
            })
        };
        let default_luts = [lut(1, 1, 1), lut(1, 1, 2), lut(1, 1, 1)];
        let waves_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean waves"),
            size: wgpu::Extent3d {
                width: ocean_fft::N as u32,
                height: ocean_fft::N as u32,
                depth_or_array_layers: 2,
            },
            mip_level_count: ocean_fft::MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let waves_view = waves_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(D::D2Array),
            ..Default::default()
        });
        let ring_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planet ring"),
            layout: Some(&ring_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_ring"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_ring"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth(false, wgpu::CompareFunction::Greater)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let star_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet stars (none yet)"),
            layout: &star_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&defaults[&Tex::Foam]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&equi_sampler),
                },
            ],
        });
        let stars: Rc<RefCell<Option<web_sys::ImageBitmap>>> = Rc::new(RefCell::new(None));
        {
            let slot = stars.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_bitmap("assets/textures/skybox_spacescape_starfield.png").await {
                    Ok(b) => *slot.borrow_mut() = Some(b),
                    Err(e) => log(&format!("planet stars: {e}")),
                }
            });
        }
        PlanetRenderer {
            ring_layout,
            ring_pipeline,
            star_layout,
            star_bind,
            stars,
            shaft_pipeline,
            under_water: false,
            shadow_map: (gpu.shadow_map().0.clone(), gpu.shadow_map().1.clone()),
            shafts: false,
            prop_sun: 1.0,
            waves_tex,
            waves_view,
            ocean: None,
            device,
            queue,
            sky_pipeline,
            terrain_pipeline,
            ocean_pipeline,
            cloud_pipeline,
            vcloud_pipeline,
            grass_pipeline,
            far_pipeline,
            far_chunk_buf,
            far_chunk_bind,
            grass_layout,
            grass_chunk_buf,
            grass_chunk_bind,
            frame_layout,
            frame_plain_layout,
            surface_layout,
            chunk_layout,
            frame_buf,
            chunk_buf,
            chunk_bind,
            chunk_capacity,
            index_buf,
            index_count: indices.len() as u32,
            lut_sampler,
            wrap_sampler,
            equi_sampler,
            mip,
            defaults,
            default_luts,
            terrain: Rc::new(RefCell::new(None)),
            terrain_requested: false,
            bodies: HashMap::new(),
            current: None,
            frame_no: 0,
            draws: Vec::new(),
            cloud_draws: Vec::new(),
            active: false,
            debug: 0.0,
            exposure_scale: 1.0,
            exclusions: Vec::new(),
        }
    }

    fn make_chunks(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planet chunks"),
            size: capacity * SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet chunks"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ChunkU>() as u64),
                }),
            }],
        });
        (buf, bind)
    }

    /// Start loading a body's assets (and the shared terrain maps) once.
    fn ensure_body(&mut self, id: &str) {
        if !self.terrain_requested {
            self.terrain_requested = true;
            let slot = self.terrain.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let idx = fetch_bytes(&format!("{BASE}/terrain.json")).await;
                let bin = fetch_bytes(&format!("{BASE}/terrain.bin")).await;
                match (idx, bin) {
                    (Ok(i), Ok(b)) => match serde_json::from_slice::<TerrainIndex>(&i) {
                        Ok(index) => *slot.borrow_mut() = Some((index, b)),
                        Err(e) => log(&format!("planet terrain index: {e}")),
                    },
                    (Err(e), _) | (_, Err(e)) => log(&format!("planet terrain: {e}")),
                }
            });
        }
        if self.bodies.contains_key(id) {
            return;
        }
        let surface_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planet surface"),
            size: std::mem::size_of::<SurfaceU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let inbox: Rc<RefCell<Vec<Loaded>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let inbox = inbox.clone();
            let id = id.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                // The terrain pack the server lands on, when the container has one.
                let description = fetch_bytes(&format!("assets/terrain/{id}.json"))
                    .await
                    .ok()
                    .and_then(|j| serde_json::from_slice::<TerrainPack>(&j).ok());
                let pack = match description {
                    Some(pack) => fetch_bytes(&format!("assets/terrain/{id}.bin"))
                        .await
                        .ok()
                        .map(|blob| Box::new((pack, blob))),
                    None => None,
                };
                inbox.borrow_mut().push(Loaded::Pack(pack));
            });
        }
        {
            // Small metadata must not sit behind a multi-megabyte terrain pack. Start textures, sky
            // and atmosphere loading while the terrain is in flight, including on slow remote links.
            let inbox = inbox.clone();
            let base = format!("{BASE}/{id}");
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_bytes(&format!("{base}/planet.json")).await {
                    Ok(b) => match serde_json::from_slice(&b) {
                        Ok(v) => inbox.borrow_mut().push(Loaded::Info(v)),
                        Err(_) => inbox.borrow_mut().push(Loaded::NoInfo),
                    },
                    Err(_) => inbox.borrow_mut().push(Loaded::NoInfo),
                }
            });
        }
        self.bodies.insert(
            id.to_string(),
            Body {
                info: None,
                info_done: false,
                pack: None,
                pack_done: false,
                map: None,
                no_terrain: false,
                scatter: None,
                placed: Vec::new(),
                placed_at: None,
                grass: None,
                grass_at: None,
                grass_bind: None,
                far: None,
                far_at: None,
                far_pending: None,
                far_bind: None,
                min_h: 0.0,
                max_h: 0.0,
                quads: HashMap::new(),
                stale: std::collections::HashSet::new(),
                clouds: HashMap::new(),
                tex: HashMap::new(),
                arrays: HashMap::new(),
                luts: [None, None, None],
                layer_params: None,
                ring_bind: None,
                inbox,
                surface_buf,
                surface_bind: None,
                frame_bind: None,
                frame_plain_bind: None,
                dirty: true,
            },
        );
    }

    /// Fetch the textures `planet.json` says exist.
    fn request_assets(id: &str, info: &serde_json::Value, inbox: &Rc<RefCell<Vec<Loaded>>>) {
        let base = format!("{BASE}/{id}");
        let mut images: Vec<(Tex, String)> = Vec::new();
        if let Some(s) = info.get("surface") {
            if s.get("albedo_source").is_some() {
                images.push((Tex::Albedo, format!("{base}/albedo.jpg")));
            }
            if s.get("normal_source").is_some() {
                images.push((Tex::Normal, format!("{base}/normal.png")));
            }
            images.push((Tex::Biome, format!("{base}/biome.png")));
            for (tex, prefix, key) in [
                (Tex::DetailA, "detail_a", "detail_a_count"),
                (Tex::DetailN, "detail_n", "detail_n_count"),
            ] {
                let n = s.get(key).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                for i in 0..n {
                    let url = format!("{base}/{prefix}{i}.png");
                    let inbox = inbox.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match fetch_bitmap(&url).await {
                            Ok(b) => inbox.borrow_mut().push(Loaded::Layer(tex, i, b)),
                            Err(e) => log(&format!("planet {url}: {e}")),
                        }
                    });
                }
            }
        }
        if info.get("clouds").is_some() {
            images.push((Tex::Clouds, format!("{base}/clouds.png")));
        }
        if info["ocean"].get("foam").is_some() {
            images.push((Tex::Foam, format!("{base}/foam.png")));
        }
        if let Some(t) = info["ring"]["texture"].as_str() {
            images.push((Tex::Ring, format!("{base}/{t}")));
        }
        if info["poles"]["albedo"].as_bool() == Some(true) {
            images.push((Tex::PoleAlbedo, format!("{base}/poles.png")));
        }
        for (tex, url) in images {
            let inbox = inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_bitmap(&url).await {
                    Ok(b) => inbox.borrow_mut().push(Loaded::Image(tex, b)),
                    Err(e) => log(&format!("planet {url}: {e}")),
                }
            });
        }
        {
            let url = format!("{base}/scatter.json");
            let inbox = inbox.clone();
            let base = base.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(b) = fetch_bytes(&url).await {
                    match serde_json::from_slice::<ScatterFile>(&b) {
                        Ok(mut f) => {
                            // the vegetation system's own biome mask, before placing anything
                            if let Some(m) = &f.mask {
                                if let Ok(raw) = fetch_bytes(&format!("{base}/{}", m.file)).await {
                                    f.mask_data = raw.as_chunks::<4>().0.to_vec();
                                }
                            }
                            inbox.borrow_mut().push(Loaded::Scatter(f))
                        }
                        Err(e) => log(&format!("planet {url}: {e}")),
                    }
                }
            });
        }
        if info.get("atmosphere").is_some() {
            for (i, name) in ["atmo_transmittance", "atmo_scattering", "atmo_irradiance"]
                .iter()
                .enumerate()
            {
                let url = format!("{base}/{name}.f16");
                let inbox = inbox.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_bytes(&url).await {
                        Ok(b) => inbox.borrow_mut().push(Loaded::Lut(i, b)),
                        Err(e) => log(&format!("planet {url}: {e}")),
                    }
                });
            }
        }
    }

    fn texture_for(&self, w: u32, h: u32, layers: u32, srgb: bool) -> wgpu::Texture {
        let mips = 32 - w.max(h).leading_zeros();
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("planet image"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    fn copy_bitmap(&self, bitmap: &web_sys::ImageBitmap, t: &wgpu::Texture, layer: u32) {
        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture: t,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: false,
            },
            wgpu::Extent3d {
                width: bitmap.width(),
                height: bitmap.height(),
                depth_or_array_layers: 1,
            },
        );
    }

    fn upload_lut(&self, i: usize, bytes: &[u8]) -> Option<wgpu::TextureView> {
        let (w, h, d) = [(1024u32, 128u32, 1u32), (256, 128, 32), (64, 16, 1)][i];
        if bytes.len() != (w * h * d * 8) as usize {
            log(&format!("planet lut {i}: {} bytes", bytes.len()));
            return None;
        }
        let t = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("planet lut"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: d,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: if d > 1 {
                wgpu::TextureDimension::D3
            } else {
                wgpu::TextureDimension::D2
            },
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            t.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: d,
            },
        );
        Some(t.create_view(&wgpu::TextureViewDescriptor::default()))
    }

    fn drain(&mut self, id: &str) {
        let Some(body) = self.bodies.get(id) else {
            return;
        };
        let items: Vec<Loaded> = body.inbox.borrow_mut().drain(..).collect();
        let inbox = body.inbox.clone();
        for it in items {
            match it {
                Loaded::Info(v) => {
                    Self::request_assets(id, &v, &inbox);
                    let b = self.bodies.get_mut(id).unwrap();
                    b.info = Some(v);
                    b.info_done = true;
                    b.dirty = true;
                }
                Loaded::NoInfo => {
                    let b = self.bodies.get_mut(id).unwrap();
                    b.info_done = true;
                    b.dirty = true;
                }
                Loaded::Image(tex, bitmap) => {
                    let srgb =
                        matches!(tex, Tex::Albedo | Tex::Clouds | Tex::PoleAlbedo | Tex::Ring);
                    let t = self.texture_for(bitmap.width(), bitmap.height(), 1, srgb);
                    self.copy_bitmap(&bitmap, &t, 0);
                    self.mip
                        .generate(&self.device, &self.queue, &t, t.mip_level_count(), 1);
                    let view = t.create_view(&wgpu::TextureViewDescriptor::default());
                    let b = self.bodies.get_mut(id).unwrap();
                    b.tex.insert(tex, view);
                    b.dirty = true;
                }
                Loaded::Layer(tex, i, bitmap) => {
                    let count = if tex == Tex::Grass {
                        self.bodies[id]
                            .scatter
                            .as_ref()
                            .map(|f| f.grass_textures().len())
                            .unwrap_or(1) as u32
                    } else if tex == Tex::Impostor {
                        self.bodies[id]
                            .scatter
                            .as_ref()
                            .map(|f| f.impostor_textures().len())
                            .unwrap_or(1) as u32
                    } else {
                        let key = if tex == Tex::DetailA {
                            "detail_a_count"
                        } else {
                            "detail_n_count"
                        };
                        self.bodies[id]
                            .info
                            .as_ref()
                            .and_then(|v| v["surface"][key].as_u64())
                            .unwrap_or(1) as u32
                    };
                    if !self.bodies[id].arrays.contains_key(&tex) {
                        let t = self.texture_for(
                            bitmap.width(),
                            bitmap.height(),
                            count.max(1),
                            tex == Tex::DetailA || tex == Tex::Grass || tex == Tex::Impostor,
                        );
                        self.bodies
                            .get_mut(id)
                            .unwrap()
                            .arrays
                            .insert(tex, (t, count.max(1), 0));
                    }
                    let (t, layers) = {
                        let (t, layers, _) = &self.bodies[id].arrays[&tex];
                        (t.clone(), *layers)
                    };
                    if bitmap.width() != t.width() || bitmap.height() != t.height() {
                        // the slice stays empty, but it still counts so the array completes
                        log(&format!("planet {tex:?} layer {i}: size differs"));
                    } else {
                        self.copy_bitmap(&bitmap, &t, i);
                    }
                    let done = {
                        let b = self.bodies.get_mut(id).unwrap();
                        let e = b.arrays.get_mut(&tex).unwrap();
                        e.2 += 1;
                        e.2 == layers
                    };
                    if done {
                        self.mip.generate(
                            &self.device,
                            &self.queue,
                            &t,
                            t.mip_level_count(),
                            layers,
                        );
                        let view = t.create_view(&wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            ..Default::default()
                        });
                        let b = self.bodies.get_mut(id).unwrap();
                        b.tex.insert(tex, view);
                        b.dirty = true;
                    }
                }
                Loaded::Scatter(f) => {
                    for (i, t) in f.impostor_textures().iter().enumerate() {
                        let url = format!("assets/{t}");
                        let inbox = inbox.clone();
                        let i = i as u32;
                        wasm_bindgen_futures::spawn_local(async move {
                            match fetch_bitmap(&url).await {
                                Ok(b) => {
                                    inbox.borrow_mut().push(Loaded::Layer(Tex::Impostor, i, b))
                                }
                                Err(e) => log(&format!("planet {url}: {e}")),
                            }
                        });
                    }
                    for (i, t) in f.grass_textures().iter().enumerate() {
                        let url = format!("assets/{t}");
                        let inbox = inbox.clone();
                        let i = i as u32;
                        wasm_bindgen_futures::spawn_local(async move {
                            match fetch_bitmap(&url).await {
                                Ok(b) => inbox.borrow_mut().push(Loaded::Layer(Tex::Grass, i, b)),
                                Err(e) => log(&format!("planet {url}: {e}")),
                            }
                        });
                    }
                    self.bodies.get_mut(id).unwrap().scatter = Some(f);
                }
                Loaded::Pack(p) => {
                    let b = self.bodies.get_mut(id).unwrap();
                    b.pack = p;
                    b.pack_done = true;
                }
                Loaded::Lut(i, bytes) => {
                    let v = self.upload_lut(i, &bytes);
                    let b = self.bodies.get_mut(id).unwrap();
                    b.luts[i] = v;
                    b.dirty = true;
                }
            }
        }
        // The terrain map, once the shared maps and the body's description are in.
        let terrain = self.terrain.borrow();
        let b = self.bodies.get_mut(id).unwrap();
        if b.map.is_some() || b.no_terrain || !b.info_done || !b.pack_done {
            return;
        }
        // A gas giant (the atmosphere model's IsGasGiant: Jool) has no ground to shape: the sphere under its
        // cloud deck, without the height map's bumps or any scatter.
        if gas_giant(b) {
            b.no_terrain = true;
            b.dirty = true;
            return;
        }
        if let Some(pack) = b.pack.take() {
            let (pack, blob) = *pack;
            b.map = TerrainMap::from_pack(&pack, &blob);
            if let Some(d) = b.map.as_ref().and_then(|m| m.detail.as_ref()) {
                if let Some((view, params)) =
                    slope_texture(&self.device, &self.queue, &d.layer_slopes())
                {
                    b.tex.insert(Tex::Slopes, view);
                    if let Some((w, h, texels)) = d.mask() {
                        let t = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("terrain layer mask"),
                            size: wgpu::Extent3d {
                                width: w as u32,
                                height: h as u32,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 32 - (w.max(h) as u32).leading_zeros(),
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST
                                | wgpu::TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        });
                        self.queue.write_texture(
                            t.as_image_copy(),
                            bytemuck::cast_slice(texels),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(w as u32 * 4),
                                rows_per_image: Some(h as u32),
                            },
                            wgpu::Extent3d {
                                width: w as u32,
                                height: h as u32,
                                depth_or_array_layers: 1,
                            },
                        );
                        self.mip
                            .generate(&self.device, &self.queue, &t, t.mip_level_count(), 1);
                        b.tex.insert(
                            Tex::LayerMask,
                            t.create_view(&wgpu::TextureViewDescriptor::default()),
                        );
                    }
                    b.layer_params = Some((params, d.contrast() as f32));
                    b.dirty = true;
                }
            }
        }
        if b.map.is_none() {
            let Some((index, blob)) = terrain.as_ref() else {
                return;
            };
            if let Some(entry) = index.bodies.iter().find(|e| e.id == id) {
                let radius = b
                    .info
                    .as_ref()
                    .and_then(|v| v["surface"]["Radius"].as_f64())
                    .unwrap_or(1.0);
                b.map = TerrainMap::from_entry(entry, blob, radius);
            }
            // A body without any height map (Jool) is drawn as the sphere.
            b.no_terrain = b.map.is_none();
        }
        if let Some(m) = &b.map {
            if let std::collections::hash_map::Entry::Vacant(e) = b.tex.entry(Tex::GlobalHeight) {
                e.insert(height_texture(
                    &self.device,
                    &self.queue,
                    m.width,
                    m.height,
                    &m.data,
                ));
                b.dirty = true;
            }
        }
        if let Some(m) = &b.map {
            // Lowest point from the global map less the tiled layers' and decals' reach; highest from the map.
            let lo =
                m.data.iter().copied().min().unwrap_or(0) as f64 / 65535.0 * m.scale_m + m.offset_m;
            b.min_h = lo - if m.detail.is_some() { 1_500.0 } else { 0.0 };
            b.max_h = m.max_m;
        }
        b.dirty = true;
    }

    fn atmosphere_ready(b: &Body) -> bool {
        b.luts.iter().all(|l| l.is_some())
    }

    fn rebind(&mut self, id: &str) {
        let b = &self.bodies[id];
        let get = |t: Tex| b.tex.get(&t).unwrap_or(&self.defaults[&t]);
        let tv = |binding, t: Tex| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(get(t)),
        };
        let surface_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet surface"),
            layout: &self.surface_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: b.surface_buf.as_entire_binding(),
                },
                tv(1, Tex::Albedo),
                tv(2, Tex::Normal),
                tv(3, Tex::Biome),
                tv(4, Tex::DetailA),
                tv(5, Tex::DetailN),
                tv(6, Tex::Clouds),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.wrap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.equi_sampler),
                },
                tv(9, Tex::Foam),
                tv(10, Tex::PoleAlbedo),
                tv(11, Tex::GlobalHeight),
                tv(12, Tex::Slopes),
                tv(13, Tex::LayerMask),
            ],
        });
        let ring_bind = match (b.tex.get(&Tex::Ring), b.info.as_ref().map(|v| &v["ring"])) {
            (Some(view), Some(r)) if r.is_object() => {
                let f = |k: &str| r[k].as_f64().unwrap_or(0.0) as f32;
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("planet ring"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue.write_buffer(
                    &buf,
                    0,
                    bytemuck::cast_slice(&[
                        f("inner_m"),
                        f("outer_m"),
                        f("alpha_edge").max(0.001),
                        0.0,
                    ]),
                );
                Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("planet ring"),
                    layout: &self.ring_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::Sampler(&self.equi_sampler),
                        },
                    ],
                }))
            }
            _ => None,
        };
        let atmo = Self::atmosphere_ready(b);
        let lut = |i: usize| match (&b.luts[i], atmo) {
            (Some(v), true) => v,
            _ => &self.default_luts[i],
        };
        let frame_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.frame_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(lut(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lut(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(lut(2)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.lut_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.waves_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.shadow_map.0),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_map.1),
                },
            ],
        });
        let frame_plain_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet frame (no shadow)"),
            layout: &self.frame_plain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.frame_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(lut(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lut(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(lut(2)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.lut_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.waves_view),
                },
            ],
        });
        let surf = surface_uniform(b, self.debug);
        self.queue
            .write_buffer(&b.surface_buf, 0, bytemuck::bytes_of(&surf));
        let b = self.bodies.get_mut(id).unwrap();
        b.surface_bind = Some(surface_bind);
        b.ring_bind = ring_bind;
        b.frame_bind = Some(frame_bind);
        b.frame_plain_bind = Some(frame_plain_bind);
        b.dirty = false;
    }

    fn upload_quad(&self, mesh: &planet_lod::QuadMesh) -> GpuQuad {
        let mk = |v: &[TerrainVertex]| {
            let floats: Vec<f32> = v
                .iter()
                .flat_map(|t| {
                    [
                        t.pos[0],
                        t.pos[1],
                        t.pos[2],
                        t.normal[0],
                        t.normal[1],
                        t.normal[2],
                        t.height,
                        t.extra,
                        t.morph[0],
                        t.morph[1],
                        t.morph[2],
                    ]
                })
                .collect();
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("planet quad"),
                size: (floats.len() * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&buf, 0, bytemuck::cast_slice(&floats));
            buf
        };
        GpuQuad {
            center: DVec3::from_array(mesh.center),
            terrain: mk(&mesh.terrain),
            ocean: mesh.ocean.as_deref().map(mk),
            last_used: self.frame_no,
            built_ms: now_ms(),
        }
    }

    /// Prepare a frame for `view` after the camera is set; `false` while the body's terrain or surface texture
    /// is not loaded yet (the caller then draws its own stand-in).
    pub fn prepare(&mut self, gpu: &Renderer, view: &PlanetView) -> bool {
        self.frame_no += 1;
        self.upload_stars();
        self.active = false;
        self.draws.clear();
        self.cloud_draws.clear();
        let id = view.body_id.to_string();
        self.ensure_body(&id);
        self.drain(&id);
        if self.current.as_deref() != Some(id.as_str()) {
            // Another body: forget the previous one's quads (textures stay cached).
            if let Some(prev) = self.current.take() {
                if let Some(b) = self.bodies.get_mut(&prev) {
                    b.quads.clear();
                    b.clouds.clear();
                }
            }
            self.current = Some(id.clone());
        }
        {
            let b = &self.bodies[&id];
            if !b.info_done
                || (b.map.is_none() && !b.no_terrain)
                || !b.tex.contains_key(&Tex::Albedo)
            {
                return false;
            }
        }
        if self.bodies[&id].dirty {
            self.rebind(&id);
        }
        let radius = view.radius_m;
        let cam = [view.cam.x, view.cam.y, view.cam.z];
        let has_ocean = self.bodies[&id]
            .map
            .as_ref()
            .map(|m| m.has_ocean)
            .unwrap_or(false);
        let (min_h, max_h) = (self.bodies[&id].min_h, self.bodies[&id].max_h);
        let (draw, wanted) = {
            let quads = &self.bodies[&id].quads;
            planet_lod::select(cam, radius, min_h, max_h, &|k| quads.contains_key(k))
        };
        // Build the nearest missing quads within the time budget, then redo stale ones in view, nearest first.
        let t0 = now_ms();
        let mut redo: Vec<(f64, QuadKey)> = {
            let b = &self.bodies[&id];
            draw.iter()
                .filter(|k| b.stale.contains(k))
                .map(|k| (planet_lod::distance_to_quad(k, cam, radius), *k))
                .collect()
        };
        redo.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let n_wanted = wanted.len();
        for (i, (_, key)) in wanted.into_iter().chain(redo).enumerate() {
            if now_ms() - t0 > BUILD_BUDGET_MS {
                break;
            }
            let mesh = {
                let map = self.bodies[&id].map.as_ref();
                let ex = &self.exclusions;
                planet_lod::build_quad(&key, radius, has_ocean, &|d| {
                    let h = planet_lod::map_height(map, d);
                    // A site model's own ground (the KSC campus: a 19 km plateau whose skirts slope below the
                    // sea) and the terrain are drawn together and the higher shows, as in the original. Where
                    // the two are at nearly the same height they fought for the pixels and their outlines
                    // flickered as the camera moved, so out of the facilities the terrain is draped 4 m over
                    // that ground (one textured surface, the ground craft stand on). Under the facilities it
                    // drops 20 m below it so the campus model's own lawns and roads show.
                    let campus = map.and_then(|m| {
                        let (lat, lon) = planet_lod::lat_lon(d);
                        m.overlay.as_ref().and_then(|o| o.sample(lat, lon))
                    });
                    let p = planet_lod::scale(d, radius + h);
                    let sink = ex
                        .iter()
                        .filter(|e| !e.props)
                        .map(|e| e.inner_weight(p, 300.0, 150.0))
                        .fold(0.0, f64::max);
                    match campus {
                        Some(c) => h.max(c + 4.0) * (1.0 - sink) + (h.min(c) - 20.0) * sink,
                        None => h - 20.0 * sink,
                    }
                })
            };
            let mut q = self.upload_quad(&mesh);
            let b = self.bodies.get_mut(&id).unwrap();
            if i >= n_wanted {
                // a replacement: no fade-in from the parent
                q.built_ms = 0.0;
            }
            b.stale.remove(&key);
            b.quads.insert(key, q);
        }
        // The cloud shell, once its texture is in.
        // None on a gas giant: its layer starts at 0 m, on the sphere, and the banded albedo is the deck.
        let cloud_base = self.bodies[&id].info.as_ref().and_then(|v| {
            v.get("clouds")
                .map(|c| c["height_range_m"][0].as_f64().unwrap_or(7000.0))
        });
        let cloud_base = cloud_base.filter(|_| !gas_giant(&self.bodies[&id]));
        if let Some(h) = cloud_base {
            let b = &self.bodies[&id];
            if b.clouds.is_empty() && b.tex.contains_key(&Tex::Clouds) {
                let mut shell = HashMap::new();
                let mut stack: Vec<QuadKey> = QuadKey::roots().collect();
                while let Some(k) = stack.pop() {
                    if k.level < CLOUD_LEVEL {
                        stack.extend(k.children());
                        continue;
                    }
                    let mesh = planet_lod::build_quad(&k, radius, false, &|_| h);
                    shell.insert(k, self.upload_quad(&mesh));
                }
                self.bodies.get_mut(&id).unwrap().clouds = shell;
            }
        }
        // Evict quads not drawn for a while.
        {
            let frame_no = self.frame_no;
            let b = self.bodies.get_mut(&id).unwrap();
            for k in &draw {
                if let Some(q) = b.quads.get_mut(k) {
                    q.last_used = frame_no;
                }
            }
            if b.quads.len() > 2500 {
                b.quads.retain(|k, q| {
                    k.level <= planet_lod::MIN_LEVEL || frame_no - q.last_used < 120
                });
            }
        }
        // Chunk uniforms.
        let cloud_keys: Vec<QuadKey> = self.bodies[&id].clouds.keys().copied().collect();
        let needed = (draw.len() + cloud_keys.len()).max(1) as u64;
        if needed > self.chunk_capacity {
            self.chunk_capacity = needed.next_power_of_two();
            let (buf, bind) =
                Self::make_chunks(&self.device, &self.chunk_layout, self.chunk_capacity);
            self.chunk_buf = buf;
            self.chunk_bind = bind;
        }
        let mut data = vec![0u8; (needed * SLOT) as usize];
        let mut slot = 0u32;
        let now = now_ms();
        let mut put = |center: DVec3, level: u8, cloud: bool, fade: f32| -> u32 {
            let off = center - view.cam;
            let tile = DVec3::new(
                center.x.rem_euclid(TILE_PERIOD),
                center.y.rem_euclid(TILE_PERIOD),
                center.z.rem_euclid(TILE_PERIOD),
            );
            let u = ChunkU {
                offset: [off.x as f32, off.y as f32, off.z as f32, fade],
                center: [
                    center.x as f32,
                    center.y as f32,
                    center.z as f32,
                    level as f32,
                ],
                tile: [
                    tile.x as f32,
                    tile.y as f32,
                    tile.z as f32,
                    if cloud { 1.0 } else { 0.0 },
                ],
            };
            let at = (slot as u64 * SLOT) as usize;
            data[at..at + std::mem::size_of::<ChunkU>()].copy_from_slice(bytemuck::bytes_of(&u));
            slot += 1;
            slot - 1
        };
        let b = &self.bodies[&id];
        let mut draws = Vec::with_capacity(draw.len());
        for k in &draw {
            let q = &b.quads[k];
            // the coarsest levels never had a parent to come from
            let fade = if k.level > planet_lod::MIN_LEVEL {
                (1.0 - (now - q.built_ms) / QUAD_FADE_MS).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            draws.push((*k, put(q.center, k.level, false, fade)));
        }
        let mut cloud_draws = Vec::new();
        for k in &cloud_keys {
            let q = &b.clouds[k];
            cloud_draws.push((*k, put(q.center, k.level, true, 0.0)));
        }
        self.queue.write_buffer(&self.chunk_buf, 0, &data);
        // Frame uniform.
        let vp: DMat4 = gpu.view_proj();
        let inv = vp.inverse();
        let a = b.info.as_ref().and_then(|v| v.get("atmosphere"));
        let f = |k: &str, d: f64| a.and_then(|a| a[k].as_f64()).unwrap_or(d);
        let v3 = |k: &str, d: [f64; 3]| -> [f64; 3] {
            a.and_then(|a| a[k].as_array())
                .map(|x| {
                    [
                        x.first().and_then(|v| v.as_f64()).unwrap_or(d[0]),
                        x.get(1).and_then(|v| v.as_f64()).unwrap_or(d[1]),
                        x.get(2).and_then(|v| v.as_f64()).unwrap_or(d[2]),
                    ]
                })
                .unwrap_or(d)
        };
        let has_atmo = a.is_some() && Self::atmosphere_ready(b);
        let bottom = f("BottomRadius", radius / 1000.0);
        let top = bottom + f("AtmosphereHeight", 0.0);
        let ray = v3("RayleighScattering", [0.0; 3]);
        let rs = f("RayleighScatteringScale", 1.0) * 0.1;
        let mie = v3("MieScattering", [0.0; 3]);
        let ms = f("MieScatteringScale", 1.0) * 0.01;
        let solar = v3("SolarIrradiance", [1.0; 3]);
        // Exposure stands in for the original's post-process auto exposure. Bodies with an atmosphere use their
        // model's own exposure, as Bruneton's tone map intends now that the frame is sRGB-encoded (Kerbin 4.12,
        // Eve 3, Duna 4.04), with a floor of 2 (Laythe's own 0.9 is near black without the original's auto
        // exposure on top). Airless bodies get a fixed value that puts their mean albedo in sunlight where
        // Kerbin's sits (albedo 0.1, solar irradiance 2).
        let mean_albedo = b
            .info
            .as_ref()
            .and_then(|v| v["surface"]["mean_albedo"].as_f64())
            .unwrap_or(0.1);
        let solar_mean = solar.iter().sum::<f64>() / 3.0 * 2.0;
        let auto = 4.12 * (0.1 / mean_albedo.max(0.01)) * (2.0 / solar_mean.max(0.1));
        // EXPOSURE_GAIN: against Josh's KSP2 screenshots our frame at the pad was 25-30 % too dark in sRGB (the
        // sky's top band 108,135,164 against 147,176,214), the original's auto exposure lifting it further.
        let exposure = EXPOSURE_GAIN
            * match a.and_then(|a| a["Exposure"][0].as_f64()) {
                Some(e) => (e * 0.1).max(2.0),
                None => auto.clamp(4.0, 20.0),
            };
        // Props (rocks, trees: the renderer's own lit shader, no exposure) get their sunlight scaled to what the
        // terrain receives through this exposure, so they are not darker than the ground they stand on.
        // Eased on very dark bodies (Moho's exposure is ~10): their rocks reuse bright stock textures that would
        // blow out to white.
        self.prop_sun = ((exposure * solar_mean / std::f64::consts::PI) as f32).clamp(1.0, 2.5)
            * (6.0 / exposure as f32).min(1.0);
        // FFT waves while the camera is low over an ocean world with a spectrum
        let waves_live = has_ocean
            && quality() > 0
            && view.cam.length() - radius < 60_000.0
            && update_waves(
                &mut self.ocean,
                &self.queue,
                &self.waves_tex,
                &id,
                b.info.as_ref(),
                view.time_s,
            );
        let cam_km = view.cam / 1000.0;
        let sun = view.sun.normalize_or_zero();
        // The stars stay put in space while the body turns: their x axis follows the sun's direction in the
        // equatorial plane (the sun moves slowly against the stars over a year, the body turns daily).
        let star_x = {
            let e = DVec3::new(sun.x, 0.0, sun.z);
            if e.length() > 1e-6 {
                e.normalize()
            } else {
                DVec3::X
            }
        };
        let star_z = star_x.cross(DVec3::Y);
        let fu = FrameU {
            view_proj: vp.as_mat4().to_cols_array_2d(),
            inv_view_proj: inv.as_mat4().to_cols_array_2d(),
            cam_km: [
                cam_km.x as f32,
                cam_km.y as f32,
                cam_km.z as f32,
                (view.time_s % 10_000.0) as f32,
            ],
            sun: [
                sun.x as f32,
                sun.y as f32,
                sun.z as f32,
                exposure as f32 * self.exposure_scale,
            ],
            atmo: [
                bottom as f32,
                top as f32,
                f("SunZenithAngle", 102.0).to_radians().cos() as f32,
                f("MieAnisotropy", 0.8) as f32,
            ],
            rayleigh: [
                (ray[0] * rs) as f32,
                (ray[1] * rs) as f32,
                (ray[2] * rs) as f32,
                if has_atmo { 1.0 } else { 0.0 },
            ],
            mie: [
                (mie[0] * ms) as f32,
                (mie[1] * ms) as f32,
                (mie[2] * ms) as f32,
                (f("SunAngleRadius", 0.04675) * 0.1) as f32,
            ],
            solar: [
                (solar[0] * 2.0) as f32,
                (solar[1] * 2.0) as f32,
                (solar[2] * 2.0) as f32,
                radius as f32,
            ],
            water: [
                (view.cam.length() - radius) as f32,
                if has_ocean { 1.0 } else { 0.0 },
                // z: 1 draws the clouds as a volume (camera below 40 km over a cloudy body)
                // (not on a gas giant, whose cloud deck is its surface)
                if b.tex.contains_key(&Tex::Clouds)
                    && !gas_giant(b)
                    && quality() > 0
                    && view.cam.length() - radius < 40_000.0
                {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ],
            occluders: {
                let mut o = [[0.0f32; 4]; 4];
                for (k, (c, r)) in view.occluders.iter().take(4).enumerate() {
                    let ck = *c / 1000.0;
                    o[k] = [ck.x as f32, ck.y as f32, ck.z as f32, (*r / 1000.0) as f32];
                }
                o
            },
            eclipse: [
                view.sun_radius_rad.max(1e-5) as f32,
                view.occluders.len().min(4) as f32,
                0.0,
                0.0,
            ],
            star_x: [star_x.x as f32, star_x.y as f32, star_x.z as f32, 0.0],
            star_z: [star_z.x as f32, star_z.y as f32, star_z.z as f32, 0.0],
            quality: {
                let q = quality();
                [
                    [0.0, 10.0, 20.0][q as usize],
                    [0.0, 24.0, 48.0][q as usize],
                    f32::from(u8::from(q > 0)),
                    self.prop_sun,
                ]
            },
            shadow_vp: gpu.shadow_uniform().0,
            mesh_shadow: gpu.shadow_uniform().1,
            cam_tile: [
                view.cam.x.rem_euclid(TILE_PERIOD) as f32,
                view.cam.y.rem_euclid(TILE_PERIOD) as f32,
                view.cam.z.rem_euclid(TILE_PERIOD) as f32,
                0.0,
            ],
            waves: [
                f32::from(u8::from(waves_live)),
                ocean_fft::PATCH_LONG as f32,
                ocean_fft::PATCH_SHORT as f32,
                ocean_fft::CHOP as f32,
            ],
            water_col: {
                let s = surface_uniform(b, 0.0);
                let o = b.info.as_ref().and_then(|v| v.get("ocean"));
                let visible = o.and_then(|o| o["TransparentVisibleDepth"].as_f64());
                // w: how far one sees under water (m)
                [
                    s.shallow[0] * 0.6 + s.deep[0] * 0.4,
                    s.shallow[1] * 0.6 + s.deep[1] * 0.4,
                    s.shallow[2] * 0.6 + s.deep[2] * 0.4,
                    underwater_view_m(visible),
                ]
            },
        };
        self.queue
            .write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&fu));
        self.draws = draws;
        self.cloud_draws = cloud_draws;
        self.under_water = has_ocean && fu.water[0] < 0.0;
        // The sun's lens flare (lens_flare.rs, drawn by the renderer over the finished scene): faded and reddened
        // as the sun sinks into the air, none under the sea; the depth buffer hides it behind terrain and craft.
        {
            let el = view.cam.normalize_or_zero().dot(sun) as f32;
            let (strength, tint) = if self.under_water {
                (0.0, [1.0, 1.0, 1.0])
            } else if has_atmo {
                let day = ((el + 0.03) / 0.13).clamp(0.0, 1.0);
                let warm = (el / 0.2).clamp(0.0, 1.0);
                (day, [1.0, 0.55 + 0.45 * warm, 0.3 + 0.7 * warm])
            } else {
                (1.0, [1.0, 1.0, 1.0])
            };
            if strength > 0.0 {
                gpu.set_flare(sun, strength, tint, view.sun_radius_rad);
            }
        }
        self.shafts = self.under_water && quality() > 0;
        self.active = true;
        true
    }

    /// The skybox bitmap, once fetched: an sRGB texture with mips, bound for the sky pass.
    fn upload_stars(&mut self) {
        let Some(bitmap) = self.stars.borrow_mut().take() else {
            return;
        };
        let t = self.texture_for(bitmap.width(), bitmap.height(), 1, true);
        self.copy_bitmap(&bitmap, &t, 0);
        self.mip
            .generate(&self.device, &self.queue, &t, t.mip_level_count(), 1);
        let view = t.create_view(&wgpu::TextureViewDescriptor::default());
        self.star_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planet stars"),
            layout: &self.star_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.equi_sampler),
                },
            ],
        });
    }

    /// Ground under a model-frame direction (m above the radius; the sea surface on ocean worlds), from the same
    /// map as the server. `None` until the body's map is loaded.
    pub fn ground_m(&self, body_id: &str, dir: DVec3) -> Option<f64> {
        let m = self.bodies.get(body_id)?.map.as_ref()?;
        let (lat, lon) = planet_lod::lat_lon([dir.x, dir.y, dir.z]);
        Some(m.ground_m(lat, lon))
    }

    /// Props (rocks, plants, trees) around the camera as draw items for the renderer, after the original's
    /// vegetation packages (planet_scatter.rs). Re-placed when the camera has moved a few metres.
    pub fn scatter_items(&mut self, gpu: &mut Renderer, view: &PlanetView) -> Vec<DrawItem> {
        let Some(b) = self.bodies.get_mut(view.body_id) else {
            return Vec::new();
        };
        let (Some(f), Some(map)) = (&b.scatter, &b.map) else {
            return Vec::new();
        };
        let moved = b
            .placed_at
            .map(|p| (p - view.cam).length())
            .unwrap_or(f64::INFINITY);
        if moved > 4.0 {
            b.placed = f.place(
                map,
                view.radius_m,
                [view.cam.x, view.cam.y, view.cam.z],
                &self.exclusions,
            );

            b.placed_at = Some(view.cam);
        }
        // Grass: re-placed around the camera when it has moved 8 m (it fades out 12 m inside its range).
        let gmoved = b
            .grass_at
            .map(|p| (p - view.cam).length())
            .unwrap_or(f64::INFINITY);
        if gmoved > 8.0 {
            let mut g = f.place_grass(
                map,
                view.radius_m,
                [view.cam.x, view.cam.y, view.cam.z],
                &self.exclusions,
            );
            // medium keeps the nearer half of the range, low draws none
            match quality() {
                0 => g.clear(),
                1 => g.retain(|p| {
                    p.pos[0] * p.pos[0] + p.pos[1] * p.pos[1] + p.pos[2] * p.pos[2] < 55.0 * 55.0
                }),
                _ => {}
            }
            b.grass_at = Some(view.cam);
            b.grass = (!g.is_empty()).then(|| {
                let floats: Vec<f32> = g
                    .iter()
                    .flat_map(|p: &Grass| {
                        [
                            p.pos[0], p.pos[1], p.pos[2], p.up[0], p.up[1], p.up[2], p.scale,
                            p.yaw, p.layer, p.tint,
                        ]
                    })
                    .collect();
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("planet grass"),
                    size: (floats.len() * 4) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&buf, 0, bytemuck::cast_slice(&floats));
                (buf, g.len() as u32)
            });
        }
        if let Some(origin) = b.grass_at {
            let off = origin - view.cam;
            let tint = |k: usize| -> [f32; 4] {
                // the first grass item's dry and healthy tints (Kerbin's grasses share a look)
                let t = f
                    .packages
                    .iter()
                    .flat_map(|p| &p.items)
                    .find_map(|i| i.tint)
                    .map(|t| t[k])
                    .unwrap_or([1.0; 3]);
                [t[0] as f32, t[1] as f32, t[2] as f32, 1.0]
            };
            let u = ChunkU {
                offset: [off.x as f32, off.y as f32, off.z as f32, 0.0],
                center: tint(0),
                tile: tint(1),
            };
            self.queue
                .write_buffer(&self.grass_chunk_buf, 0, bytemuck::bytes_of(&u));
        }
        // Far props are anchored to the ground, so vertical ascent reuses their placement. The shader
        // already fades each prop by its current camera distance. Rebuild only after lateral travel.
        let cam = view.cam.to_array();
        let high = view.cam.length() - view.radius_m - planet_lod::map_height(Some(map), cam)
            > FAR_RANGE_M;
        let fmoved = b
            .far_at
            .map(|p| surface_distance(p.to_array(), cam, view.radius_m))
            .unwrap_or(f64::INFINITY);
        if high || quality() == 0 {
            b.far = None;
            b.far_at = None;
            b.far_pending = None;
        } else if fmoved > 60.0 && b.far_pending.is_none() {
            b.far_pending = Some(FarPlacement::new(cam, view.radius_m));
        }
        let start = now_ms();
        let mut finished = false;
        if let Some(job) = &mut b.far_pending {
            while now_ms() - start < SCATTER_BUDGET_MS {
                if f.advance_far(job, map, &self.exclusions, 256) {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            let job = b.far_pending.take().unwrap();
            b.far_at = Some(DVec3::from_array(job.origin));
            let g = job.finish();
            b.far = (!g.is_empty()).then(|| {
                let floats: Vec<f32> = g
                    .iter()
                    .flat_map(|p: &Far| {
                        [
                            p.pos[0],
                            p.pos[1],
                            p.pos[2],
                            p.size,
                            p.lift,
                            p.layer,
                            p.frames,
                            p.colour[0],
                            p.colour[1],
                            p.colour[2],
                            p.near,
                        ]
                    })
                    .collect();
                let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("planet impostors"),
                    size: (floats.len() * 4) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue
                    .write_buffer(&buf, 0, bytemuck::cast_slice(&floats));
                (buf, g.len() as u32)
            });
        }
        if let Some(origin) = b.far_at {
            let off = origin - view.cam;
            let u = ChunkU {
                offset: [off.x as f32, off.y as f32, off.z as f32, 0.0],
                center: [0.0; 4],
                tile: [0.0; 4],
            };
            self.queue
                .write_buffer(&self.far_chunk_buf, 0, bytemuck::bytes_of(&u));
        }
        if b.far_bind.is_none() {
            if let Some(view_tex) = b.tex.get(&Tex::Impostor) {
                b.far_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("planet impostors"),
                    layout: &self.grass_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view_tex),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.wrap_sampler),
                        },
                    ],
                }));
            }
        }
        if b.grass_bind.is_none() {
            if let Some(view_tex) = b.tex.get(&Tex::Grass) {
                b.grass_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("planet grass"),
                    layout: &self.grass_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view_tex),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.wrap_sampler),
                        },
                    ],
                }));
            }
        }
        let models = f.models();
        // props are lit by the renderer's own shader: dim them by hand during an eclipse
        let vis = eclipse_visible(view.cam, view.sun, view.occluders, view.sun_radius_rad) as f32;
        let dim = 0.15 + 0.85 * vis;
        let mut out = Vec::with_capacity(b.placed.len());
        for inst in &b.placed {
            let path = models[inst.model];
            let key = format!("scatter:{path}");
            gpu.request_model(&key, &format!("assets/{path}"));
            if !gpu.has_model(&key) {
                continue;
            }
            let up = DVec3::from_array(inst.up);
            let rot = glam::DQuat::from_rotation_arc(DVec3::Y, up)
                * glam::DQuat::from_rotation_y(inst.yaw);
            // over the last part of its range the mesh shrinks away while its impostor dithers in
            let d = (DVec3::from_array(inst.pos) - view.cam).length();
            let fade = 1.0
                - ((d - inst.range * crate::planet_scatter::XFADE)
                    / (inst.range * (1.0 - crate::planet_scatter::XFADE)).max(1e-3))
                .clamp(0.0, 1.0);
            let fade = fade * fade * (3.0 - 2.0 * fade);
            if fade < 0.02 {
                continue;
            }
            let mut it = DrawItem::lit(
                key,
                DVec3::from_array(inst.pos) - view.cam,
                rot,
                inst.scale * fade,
                view.sun,
            );
            it.ambient = 0.3;
            it.sun = self.prop_sun;
            it.color = [dim, dim, dim, 1.0];
            out.push(it);
        }
        out
    }

    /// Keep scatter (rocks, plants, trees, grass) off these rectangles; props are placed again when they change.
    pub fn set_exclusions(&mut self, ex: Vec<Exclusion>) {
        if ex != self.exclusions {
            // the quads under both the old and the new exclusions change
            let old = std::mem::replace(&mut self.exclusions, ex);
            let ex: Vec<Exclusion> = old
                .into_iter()
                .chain(self.exclusions.iter().cloned())
                .collect();
            for b in self.bodies.values_mut() {
                b.placed_at = None;
                b.grass_at = None;
                b.far_at = None;
                b.far_pending = None;
                b.far = None;
                // terrain under the campus is lowered: redo the quads near it, in place
                let radius = b.map.as_ref().map(|m| m.body_radius_m).unwrap_or(1.0);
                let near: Vec<QuadKey> = b
                    .quads
                    .keys()
                    .filter(|k| {
                        let c = planet_lod::scale(k.center_dir(), radius);
                        let reach = k.angular_radius() * radius + 15_000.0;
                        ex.iter()
                            .any(|e| planet_lod::length(planet_lod::sub(c, e.origin)) < reach)
                    })
                    .copied()
                    .collect();
                b.stale.extend(near);
            }
        }
    }

    fn body(&self) -> Option<&Body> {
        self.bodies.get(self.current.as_deref()?)
    }
}

/// Fraction of the star's disc visible from `p` (model frame, m), as the shader's `eclipse`.
pub fn eclipse_visible(p: DVec3, sun: DVec3, occluders: &[(DVec3, f64)], sun_radius: f64) -> f64 {
    let a_s = sun_radius.max(1e-5);
    let sun = sun.normalize_or_zero();
    let mut vis = 1.0;
    for (c, r) in occluders.iter().take(4) {
        let d = *c - p;
        let dist = d.length();
        if dist <= *r || d.dot(sun) <= 0.0 {
            continue;
        }
        let a_o = (r / dist).clamp(0.0, 1.0).asin();
        let theta = (d / dist).dot(sun).clamp(-1.0, 1.0).acos();
        let full = (a_o - a_s).abs();
        let deepest = if a_o < a_s {
            (a_o * a_o) / (a_s * a_s)
        } else {
            1.0
        };
        let x = ((theta - full) / (a_s + a_o - full).max(1e-9)).clamp(0.0, 1.0);
        let t = x * x * (3.0 - 2.0 * x);
        vis *= 1.0 - deepest * (1.0 - t);
    }
    vis
}

/// Run the body's ocean spectrum to time `t` and upload the packed waves with their mips; false when the body
/// has none. Gravity is Kerbin's for every sea (the spectrum's frequencies are tuned for it).
fn update_waves(
    ocean: &mut Option<(String, ocean_fft::OceanFft)>,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    id: &str,
    info: Option<&serde_json::Value>,
    t: f64,
) -> bool {
    if ocean.as_ref().map(|o| o.0.as_str()) != Some(id) {
        let spec = info
            .and_then(|v| v.get("ocean"))
            .and_then(|o| o.get("spectrum"))
            .and_then(|s| serde_json::from_value::<ocean_fft::Spectrum>(s.clone()).ok());
        let Some(spec) = spec else {
            *ocean = None;
            return false;
        };
        *ocean = Some((id.to_string(), ocean_fft::OceanFft::new(&spec, 9.81)));
    }
    let Some((_, fft)) = ocean.as_mut() else {
        return false;
    };
    fft.update(t);
    for (level, texels) in fft.texels().iter().enumerate() {
        let n = (ocean_fft::N >> level) as u32;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 8),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 2,
            },
        );
    }
    true
}

/// The terrain pack's layer slopes as an eight-layer RG16F array with box-filtered mips (all stock layers are
/// 1024 x 1024; others are skipped), and their (uv scale, height scale) pairs; `None` without any layer.
fn slope_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layers: &[Option<ose_data::terrain_detail::LayerSlopes>],
) -> Option<(wgpu::TextureView, [[f32; 4]; 4])> {
    const N: usize = 1024;
    const MIPS: u32 = 8;
    let mut params = [[0.0f32; 4]; 4];
    let mut any = false;
    for (k, l) in layers.iter().enumerate().take(8) {
        if let Some(l) = l.as_ref().filter(|l| l.width == N && l.height == N) {
            params[k / 2][(k % 2) * 2] = l.uv_scale as f32;
            params[k / 2][(k % 2) * 2 + 1] = l.height_scale as f32;
            any = true;
        }
    }
    if !any {
        return None;
    }
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain layer slopes"),
        size: wgpu::Extent3d {
            width: N as u32,
            height: N as u32,
            depth_or_array_layers: 8,
        },
        mip_level_count: MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for k in 0..8 {
        let mut cur: Vec<[f32; 2]> = match layers.get(k).and_then(|l| l.as_ref()) {
            Some(l) if l.width == N && l.height == N => l.slopes.clone(),
            _ => vec![[0.0; 2]; N * N],
        };
        let mut n = N;
        for level in 0..MIPS {
            let bytes: Vec<u16> = cur
                .iter()
                .flat_map(|s| [ocean_fft::f16_bits(s[0]), ocean_fft::f16_bits(s[1])])
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: k as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&bytes),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(n as u32 * 4),
                    rows_per_image: Some(n as u32),
                },
                wgpu::Extent3d {
                    width: n as u32,
                    height: n as u32,
                    depth_or_array_layers: 1,
                },
            );
            let m = n / 2;
            let mut next = vec![[0.0f32; 2]; m * m];
            for y in 0..m {
                for x in 0..m {
                    let mut s = [0.0f32; 2];
                    for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let t = cur[(2 * y + b) * n + 2 * x + a];
                        s[0] += t[0] * 0.25;
                        s[1] += t[1] * 0.25;
                    }
                    next[y * m + x] = s;
                }
            }
            cur = next;
            n = m;
        }
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    Some((view, params))
}

/// The global height map (normalised u16, row 0 at the south pole) as R16F with box-filtered mips, for the
/// terrain shadow rays.
fn height_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    w: usize,
    h: usize,
    data: &[u16],
) -> wgpu::TextureView {
    let mips = 32 - (w.max(h) as u32).leading_zeros();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain global heights"),
        size: wgpu::Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut cur: Vec<f32> = data.iter().map(|&v| v as f32 / 65535.0).collect();
    let (mut cw, mut ch) = (w, h);
    for level in 0..mips {
        let bytes: Vec<u16> = cur.iter().map(|&v| ocean_fft::f16_bits(v)).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&bytes),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cw as u32 * 2),
                rows_per_image: Some(ch as u32),
            },
            wgpu::Extent3d {
                width: cw as u32,
                height: ch as u32,
                depth_or_array_layers: 1,
            },
        );
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let mut next = vec![0.0f32; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let at = |a: usize, b: usize| {
                    cur[(2 * y + b).min(ch - 1) * cw + (2 * x + a).min(cw - 1)]
                };
                next[y * nw + x] = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) * 0.25;
            }
        }
        cur = next;
        cw = nw;
        ch = nh;
    }
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The atmosphere model's IsGasGiant (Jool): no ground under the cloud deck.
fn gas_giant(b: &Body) -> bool {
    b.info
        .as_ref()
        .and_then(|v| v["atmosphere"]["IsGasGiant"].as_i64())
        .unwrap_or(0)
        == 1
}

fn surface_uniform(b: &Body, debug: f32) -> SurfaceU {
    let mut u: SurfaceU = bytemuck::Zeroable::zeroed();
    let info = b.info.as_ref();
    let v4 = |v: Option<&serde_json::Value>, d: [f32; 4]| -> [f32; 4] {
        match v.and_then(|v| v.as_array()) {
            Some(a) => {
                let mut o = d;
                for (i, x) in a.iter().take(4).enumerate() {
                    o[i] = x.as_f64().unwrap_or(d[i] as f64) as f32;
                }
                o
            }
            None => d,
        }
    };
    u.slots = [[-1.0; 4]; 4];
    if let Some(s) = info.and_then(|v| v.get("surface")) {
        for (c, ch) in ["R", "G", "B", "A"].iter().enumerate() {
            let bi = &s["biomes"][ch];
            if bi.is_null() {
                continue;
            }
            u.slots[c] = v4(bi.get("slots"), [-1.0; 4]);
            u.height_enable[c] = v4(bi.get("HeightEnable"), [0.0; 4]);
            u.slope_enable[c] = v4(bi.get("SlopeEnable"), [0.0; 4]);
            u.uv_scale[c] = v4(bi.get("UVScale"), [1.0; 4]);
            for l in 0..4 {
                let layer = &bi["layers"][l];
                u.height_params[c * 4 + l] = v4(layer.get("HeightParams"), [0.0; 4]);
                u.slope_params[c * 4 + l] = v4(layer.get("SlopeParams"), [0.0; 4]);
            }
        }
        u.albedo_fade = v4(s.get("AlbedoScaledFadeParams"), [0.0, 160_000.0, 0.3, 1.0]);
        u.normal_fade = v4(
            s.get("NormalScaledFadeParams"),
            [80_000.0, 80_500.0, 0.0, 0.6],
        );
    }
    let o = info.and_then(|v| v.get("ocean"));
    // The deep colour: the body's own _DiffuseColor (Kerbin teal, Laythe blue, Eve purple). _DeepWaterColor is
    // the same default on every ocean material, so it says nothing about the body; the diffuse colour is
    // scaled to about its brightness, which is what the shading here was tuned with.
    let deep = match o.and_then(|o| o.get("DiffuseColor")) {
        Some(d) => ocean_deep_colour(v4(Some(d), [0.0; 4])),
        None => v4(
            o.and_then(|o| o.get("DeepWaterColor")),
            [0.0, 0.34, 0.62, 1.0],
        ),
    };
    let shallow = v4(
        o.and_then(|o| o.get("ShallowColor")),
        [0.08, 0.36, 0.38, 1.0],
    );
    // the depth over which the shallow colour gives way to the deep one: _ShallowMaxDepth scaled by
    // _ShallowColorDistribution (Kerbin 343 m x 0.01: only right at the shore, so its open seas are the deep
    // azure the original shows; Eve 25 m x 0.49, Laythe 500 m x 0.08)
    let max_depth = (o
        .and_then(|o| o["ShallowMaxDepth"].as_f64())
        .unwrap_or(343.0)
        * o.and_then(|o| o["ShallowColorDistribution"].as_f64())
            .unwrap_or(0.01)
            .clamp(0.01, 1.0)) as f32;
    let smooth = o.and_then(|o| o["Smoothness"].as_f64()).unwrap_or(0.86) as f32;
    u.deep = [deep[0], deep[1], deep[2], max_depth];
    u.foam = [
        o.and_then(|o| o["ShorelineFoamMaxDepth"].as_f64())
            .unwrap_or(4.0) as f32,
        o.and_then(|o| o["FoamShapeTilingSize"].as_f64())
            .unwrap_or(10.0) as f32,
        flag_f(b.tex.contains_key(&Tex::Foam)),
        // depth (m) at which the sea is opaque: a quarter of _TransparentVisibleDepth (Kerbin 100 m -> 25 m,
        // Eve's 0.5 m soup is opaque at once)
        ocean_opaque_depth(o.and_then(|o| o["TransparentVisibleDepth"].as_f64())),
    ];
    let has_caustics =
        o.and_then(|o| o.get("caustics")).is_some() && b.tex.contains_key(&Tex::Foam);
    let of = |k: &str, d: f64| o.and_then(|o| o[k].as_f64()).unwrap_or(d) as f32;
    u.caustics = [
        if has_caustics {
            of("CausticsStrength", 0.15)
        } else {
            0.0
        },
        of("CausticsMaxVisibleDepth", 50.0),
        of("CausticsTilingSize", 8.0),
        of("CausticsAnimationSpeed", 1.0),
    ];
    u.shallow = [shallow[0], shallow[1], shallow[2], smooth];
    let c = info.and_then(|v| v.get("clouds"));
    let slices = b
        .arrays
        .get(&Tex::DetailA)
        .filter(|a| a.1 == a.2)
        .map(|a| a.1)
        .unwrap_or(0);
    u.clouds = [
        c.and_then(|c| c["height_range_m"][0].as_f64())
            .unwrap_or(0.0) as f32,
        c.map(|c| c["density"].as_f64().unwrap_or(1.0))
            .unwrap_or(0.0) as f32,
        // no shell, shadow or volume on a gas giant (see gas_giant)
        if c.is_some() && !gas_giant(b) {
            1.0
        } else {
            0.0
        },
        slices as f32,
    ];
    // z/w: the scaled material's flow speed and strength (Jool's drifting bands), 0 without
    let flow = info.and_then(|v| v["surface"].get("flow"));
    let ff = |k: &str| flow.and_then(|f| f[k].as_f64()).unwrap_or(0.0) as f32;
    u.clouds2 = [
        c.and_then(|c| c["height_range_m"][1].as_f64())
            .unwrap_or(9500.0) as f32,
        8.0,
        ff("speed"),
        ff("strength"),
    ];
    if let Some((p, contrast)) = b.layer_params.filter(|_| !gas_giant(b)) {
        u.layers = p;
        u.layer_info = [1.0, contrast, 0.0, 0.0];
    }
    if let Some(m) = b
        .map
        .as_ref()
        .filter(|_| b.tex.contains_key(&Tex::GlobalHeight))
    {
        u.shadow = [1.0, m.scale_m as f32, m.offset_m as f32, 0.0];
    }
    u.cloud_col = [
        c.and_then(|c| c["color_map_intensity"].as_f64())
            .unwrap_or(1.0) as f32,
        0.0,
        0.0,
        0.0,
    ];
    let poles = info.and_then(|v| v.get("poles"));
    u.poles = [
        flag_f(b.tex.contains_key(&Tex::PoleAlbedo)),
        flag_f(
            b.tex.contains_key(&Tex::PoleAlbedo)
                && poles.and_then(|p| p["clouds"].as_bool()) == Some(true),
        ),
        poles
            .and_then(|p| p["cap_deg"].as_f64())
            .unwrap_or(30.0)
            .to_radians() as f32,
        0.0,
    ];
    let flag = |x: bool| if x { 1.0 } else { 0.0 };
    // The original's CelestialBodyLighting ambient colours; w: 1 when the body has no atmosphere (the ambient
    // then lights the day side too, the sky does it otherwise).
    let l = info.and_then(|v| v.get("lighting"));
    let airless = info.map(|v| v.get("atmosphere").is_none()).unwrap_or(true);
    let a3 = |k: &str, d: [f32; 4]| v4(l.and_then(|l| l.get(k)), d);
    // Unity serialises colours in gamma space; the original renders in linear space, so they are decoded.
    let lin = |c: [f32; 4]| {
        let f = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        [f(c[0]), f(c[1]), f(c[2]), c[3]]
    };
    let day = lin(a3("ambient_day", [0.25, 0.25, 0.27, 0.0]));
    let night = lin(a3("ambient_night", [0.15, 0.15, 0.17, 0.0]));
    u.ambient_day = [day[0], day[1], day[2], flag(airless)];
    u.ambient_night = [night[0], night[1], night[2], 0.0];
    u.flags = [
        // a gas giant's deck takes no ground detail or normal map
        flag(slices > 0 && b.tex.contains_key(&Tex::Biome) && !gas_giant(b)),
        flag(b.tex.contains_key(&Tex::Normal) && !gas_giant(b)),
        debug,
        // the original's biome height rules count from the PQS radius, below the sea by its ocean altitude
        b.map.as_ref().map(|m| -m.offset_m).unwrap_or(0.0) as f32,
    ];
    u
}

impl ExtraPass for PlanetRenderer {
    fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.active {
            return;
        }
        let Some(b) = self.body() else { return };
        let (Some(fb), Some(sb)) = (&b.frame_bind, &b.surface_bind) else {
            return;
        };
        pass.set_pipeline(&self.sky_pipeline);
        pass.set_bind_group(0, fb, &[]);
        pass.set_bind_group(1, &self.star_bind, &[]);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.terrain_pipeline);
        pass.set_bind_group(0, fb, &[]);
        pass.set_bind_group(2, sb, &[]);
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        for (k, slot) in &self.draws {
            let Some(q) = b.quads.get(k) else { continue };
            pass.set_bind_group(1, &self.chunk_bind, &[(*slot as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, q.terrain.slice(..));
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
        if let (Some((buf, n)), Some(fb2)) = (&b.far, &b.far_bind) {
            pass.set_pipeline(&self.far_pipeline);
            pass.set_bind_group(0, b.frame_plain_bind.as_ref().unwrap_or(fb), &[]);
            pass.set_bind_group(1, &self.far_chunk_bind, &[0]);
            pass.set_bind_group(2, sb, &[]);
            pass.set_bind_group(3, fb2, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..6, 0..*n);
        }
        if let (Some((buf, n)), Some(gb)) = (&b.grass, &b.grass_bind) {
            pass.set_pipeline(&self.grass_pipeline);
            pass.set_bind_group(0, b.frame_plain_bind.as_ref().unwrap_or(fb), &[]);
            pass.set_bind_group(1, &self.grass_chunk_bind, &[0]);
            pass.set_bind_group(2, sb, &[]);
            pass.set_bind_group(3, gb, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..12, 0..*n);
        }
    }

    fn draw_transparent(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.active {
            return;
        }
        let Some(b) = self.body() else { return };
        let (Some(fb), Some(sb)) = (&b.frame_bind, &b.surface_bind) else {
            return;
        };
        pass.set_pipeline(&self.ocean_pipeline);
        pass.set_bind_group(0, fb, &[]);
        pass.set_bind_group(2, sb, &[]);
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        for (k, slot) in &self.draws {
            let Some(q) = b.quads.get(k) else { continue };
            let Some(ocean) = &q.ocean else { continue };
            pass.set_bind_group(1, &self.chunk_bind, &[(*slot as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, ocean.slice(..));
            let n = if self.under_water {
                planet_lod::GRID_INDICES
            } else {
                self.index_count
            };
            pass.draw_indexed(0..n, 0, 0..1);
        }
        if let Some(rb) = &b.ring_bind {
            pass.set_pipeline(&self.ring_pipeline);
            pass.set_bind_group(0, fb, &[]);
            pass.set_bind_group(1, rb, &[]);
            pass.draw(0..(RING_SEGMENTS + 1) * 2, 0..1);
        }
        if self.shafts {
            pass.set_pipeline(&self.shaft_pipeline);
            pass.set_bind_group(0, fb, &[]);
            pass.set_bind_group(1, &self.chunk_bind, &[0]);
            pass.set_bind_group(2, sb, &[]);
            pass.draw(0..3, 0..1);
        }
        if !self.cloud_draws.is_empty() {
            pass.set_pipeline(&self.vcloud_pipeline);
            pass.set_bind_group(0, fb, &[]);
            pass.set_bind_group(1, &self.chunk_bind, &[0]);
            pass.set_bind_group(2, sb, &[]);
            pass.draw(0..3, 0..1);
        }
        pass.set_pipeline(&self.cloud_pipeline);
        pass.set_bind_group(0, fb, &[]);
        pass.set_bind_group(2, sb, &[]);
        for (k, slot) in &self.cloud_draws {
            let Some(q) = b.clouds.get(k) else { continue };
            pass.set_bind_group(1, &self.chunk_bind, &[(*slot as u64 * SLOT) as u32]);
            pass.set_vertex_buffer(0, q.terrain.slice(..));
            pass.draw_indexed(0..planet_lod::GRID_INDICES, 0, 0..1);
        }
    }
}

/// Mipmap generation by repeated half-size blits (WebGPU has no built-in).
struct Mipmapper {
    shader: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipelines: RefCell<HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>>,
}

const MIP_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct O { @builtin(position) p: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> O {
    var o: O;
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    o.p = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}
@fragment fn fs(in: O) -> @location(0) vec4<f32> { return textureSampleLevel(src, samp, in.uv, 0.0); }
"#;

impl Mipmapper {
    fn new(device: &wgpu::Device) -> Mipmapper {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mip"),
            source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
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
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mip"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Mipmapper {
            shader,
            layout,
            sampler,
            pipelines: RefCell::new(HashMap::new()),
        }
    }

    fn generate(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        mips: u32,
        layers: u32,
    ) {
        let format = texture.format();
        let mut pipes = self.pipelines.borrow_mut();
        let pipeline = pipes.entry(format).or_insert_with(|| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mip"),
                bind_group_layouts: &[Some(&self.layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mip"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mip") });
        for layer in 0..layers {
            for level in 1..mips {
                let view = |l: u32| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_mip_level: l,
                        mip_level_count: Some(1),
                        base_array_layer: layer,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                };
                let src = view(level - 1);
                let dst = view(level);
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mip"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("mip"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit(Some(enc.finish()));
    }
}
