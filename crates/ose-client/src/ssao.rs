//! Screen-space ambient occlusion for outdoor scenes (SBS-V4): creases, the feet of buildings and the ground
//! under a craft darken a little, as the original's ambient occlusion does, so things sit on the ground instead
//! of floating on a uniformly lit field.
//!
//! Two passes after the main scene pass, from the depth buffer alone (reversed, infinite Z: depth `near / D`):
//! the occlusion at half resolution (12 samples on a spiral, normals from the depth's own derivatives, a world
//! radius that grows with distance so it stays a similar size on screen), then a depth-aware 3 x 3 blur that
//! multiplies it into the HDR scene. The sky (depth 0) and anything farther than its radius resolves are left
//! alone. `Renderer::set_ao` turns it on per frame; a view that does not ask keeps none.

/// The view the occlusion is computed for: projection and strength.
#[derive(Clone, Copy, Debug)]
pub struct AoView {
    /// Camera near distance (scene units): the reversed depth is `near / distance`.
    pub near: f32,
    /// `1 / tan(fov_y / 2)`.
    pub f: f32,
    /// Viewport width / height.
    pub aspect: f32,
    /// How dark full occlusion gets (0 off, 1 black).
    pub strength: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AoUniform {
    /// f / aspect, f, near, strength
    proj: [f32; 4],
    /// full-resolution width, height, half-resolution width, height
    size: [f32; 4],
}

const SHADER: &str = r#"
struct U { proj: vec4<f32>, size: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var ao_tex: texture_2d<f32>;

struct Out { @builtin(position) p: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0))[i];
    return Out(vec4(p, 0.0, 1.0));
}

// The view-space position at a full-resolution pixel (camera at the origin looking down -z); w 0 on the sky.
fn view_pos(px: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(u.size.xy);
    let q = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    let d = textureLoad(depth_tex, q, 0);
    if d <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let dist = u.proj.z / d;
    let ndc = (vec2<f32>(q) + 0.5) / u.size.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(ndc.x * dist / u.proj.x, ndc.y * dist / u.proj.y, -dist, 1.0);
}

@fragment fn ao(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.p.xy) * 2;
    let c = view_pos(px);
    if c.w == 0.0 {
        return vec4<f32>(1.0);
    }
    let p = c.xyz;
    let dist = -p.z;
    // Normal from the depth's neighbours, each axis from the side nearer in depth (no smearing across edges).
    let xr = view_pos(px + vec2<i32>(2, 0)).xyz - p;
    let xl = p - view_pos(px - vec2<i32>(2, 0)).xyz;
    let yd = view_pos(px + vec2<i32>(0, 2)).xyz - p;
    let yu = p - view_pos(px - vec2<i32>(0, 2)).xyz;
    let dx = select(xl, xr, abs(xr.z) < abs(xl.z));
    let dy = select(yu, yd, abs(yd.z) < abs(yu.z));
    var n = normalize(cross(dx, dy));
    if dot(n, -p) < 0.0 {
        n = -n;
    }
    // A world radius that grows with distance (about 3.5 % of it) keeps the effect a similar size on screen:
    // a craft's creases up close, a building's foot from the air.
    let radius = clamp(dist * 0.035, 0.25, 90.0);
    let r_px = radius * u.proj.y / dist * u.size.y * 0.5;
    if r_px < 2.0 {
        return vec4<f32>(1.0);
    }
    // Interleaved gradient noise turns the spiral per pixel; the blur pass smooths the pattern.
    let noise = fract(52.9829189 * fract(dot(in.p.xy, vec2<f32>(0.06711056, 0.00583715))));
    var occ = 0.0;
    let count = 12;
    for (var k = 0; k < count; k++) {
        let t = (f32(k) + 0.5) / f32(count);
        let a = (f32(k) * 2.39996 + noise * 6.2831853);
        let off = vec2<f32>(cos(a), sin(a)) * r_px * t;
        let s = view_pos(px + vec2<i32>(off));
        if s.w == 0.0 {
            continue;
        }
        let v = s.xyz - p;
        let vv = dot(v, v);
        let fall = 1.0 - clamp(vv / (radius * radius), 0.0, 1.0);
        occ += max(dot(v * inverseSqrt(max(vv, 1e-8)), n) - 0.15, 0.0) * fall;
    }
    let a = clamp(1.0 - u.proj.w * 2.2 * occ / f32(count), 0.0, 1.0);
    return vec4<f32>(a, a, a, 1.0);
}

// The occlusion, blurred 3 x 3 at half resolution with weights that drop across depth edges, multiplied into
// the scene (blend: dst * src).
@fragment fn apply(in: Out) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.p.xy);
    let c = view_pos(px);
    if c.w == 0.0 {
        return vec4<f32>(1.0);
    }
    let dist = -c.z;
    let half = vec2<i32>(u.size.zw);
    let hp = px / 2;
    var sum = 0.0;
    var wsum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(hp + vec2<i32>(x, y), vec2<i32>(0), half - vec2<i32>(1));
            let s = view_pos(q * 2);
            let w = select(exp(-abs(-s.z - dist) / (0.04 * dist)), 0.0, s.w == 0.0);
            sum += textureLoad(ao_tex, q, 0).r * w;
            wsum += w;
        }
    }
    let a = select(1.0, sum / wsum, wsum > 1e-4);
    return vec4<f32>(a, a, a, 1.0);
}
"#;

pub struct Ssao {
    ao_pipeline: wgpu::RenderPipeline,
    apply_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    ao_view: wgpu::TextureView,
    /// The occlusion pass's group (with a stand-in for the occlusion texture it renders into: a texture is never
    /// sampled and drawn to in one pass) and the apply pass's.
    bind_ao: wgpu::BindGroup,
    bind_apply: wgpu::BindGroup,
    stand_in: wgpu::TextureView,
    width: u32,
    height: u32,
}

const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Ssao {
    pub fn new(device: &wgpu::Device, depth: &wgpu::TextureView, width: u32, height: u32) -> Ssao {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ssao"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ssao"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let ao_pipeline = pipeline("ao", AO_FORMAT, None);
        // dst * src: the scene darkened by the occlusion.
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let apply_pipeline = pipeline("apply", crate::fx_post::FORMAT, Some(multiply));
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ssao"),
            size: std::mem::size_of::<AoUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ssao stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: AO_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let (ao_view, bind_ao, bind_apply) =
            Self::targets(device, &layout, &uniform, depth, &stand_in, width, height);
        Ssao {
            ao_pipeline,
            apply_pipeline,
            layout,
            uniform,
            ao_view,
            bind_ao,
            bind_apply,
            stand_in,
            width,
            height,
        }
    }

    fn targets(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        depth: &wgpu::TextureView,
        stand_in: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> (wgpu::TextureView, wgpu::BindGroup, wgpu::BindGroup) {
        let ao = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ssao half"),
            size: wgpu::Extent3d {
                width: width.div_ceil(2).max(1),
                height: height.div_ceil(2).max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: AO_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let ao_view = ao.create_view(&Default::default());
        let group = |third: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(third),
                    },
                ],
            })
        };
        let bind_ao = group(stand_in);
        let bind_apply = group(&ao_view);
        (ao_view, bind_ao, bind_apply)
    }

    /// New depth buffer and size (the window resized).
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let (ao_view, bind_ao, bind_apply) = Self::targets(
            device,
            &self.layout,
            &self.uniform,
            depth,
            &self.stand_in,
            width,
            height,
        );
        self.ao_view = ao_view;
        self.bind_ao = bind_ao;
        self.bind_apply = bind_apply;
        self.width = width;
        self.height = height;
    }

    /// Compute the occlusion and multiply it into `scene` (the HDR scene, after the main pass).
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        view: AoView,
    ) {
        let u = AoUniform {
            proj: [
                view.f / view.aspect.max(1e-6),
                view.f,
                view.near,
                view.strength,
            ],
            size: [
                self.width as f32,
                self.height as f32,
                self.width.div_ceil(2) as f32,
                self.height.div_ceil(2) as f32,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
        for (target, pipeline, bind, clear) in [
            (&self.ao_view, &self.ao_pipeline, &self.bind_ao, true),
            (scene, &self.apply_pipeline, &self.bind_apply, false),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if clear {
                            wgpu::LoadOp::Clear(wgpu::Color::WHITE)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
