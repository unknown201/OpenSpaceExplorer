//! Lens flare: the sun's glare and ghosts over the finished scene, drawn into the linear HDR scene buffer
//! (fx_post::FORMAT) before the post pass, so it blooms and is sRGB-encoded with everything else.
//!
//! The sun's visibility comes from the depth buffer (reversed Z: 0 where nothing was drawn): a 5 x 5 grid of
//! texels across the sun's disc, so craft, terrain and the planet's limb cover it gradually. The planet renderer
//! sets the sun's direction (camera-relative world) and strength (dimmed near the horizon by the air) each frame
//! through `Renderer::set_flare`; without one, nothing is drawn.

use glam::{DMat4, DVec3, DVec4};

/// A frame's flare: sun direction (camera-relative world), strength, tint, angular radius (rad).
pub type FlareRequest = (DVec3, f32, [f32; 3], f64);

const SHADER: &str = r#"
struct U {
    // sun position (NDC xy), x pixel size of the disc in NDC, on (1/0)
    sun: vec4<f32>,
    // tint rgb, strength
    tint: vec4<f32>,
    // viewport size (px), aspect (w / h), 0
    view: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var depth_tex: texture_depth_2d;

struct Out { @builtin(position) p: vec4<f32>, @location(0) ndc: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0))[i];
    return Out(vec4(p, 0.0, 1.0), p);
}

fn visibility() -> f32 {
    let size = vec2<i32>(textureDimensions(depth_tex));
    let c = (u.sun.xy * vec2(0.5, -0.5) + 0.5) * vec2<f32>(size);
    let r = max(u.sun.z * 0.5 * f32(size.x), 1.5);
    var seen = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let q = vec2<i32>(c + vec2(f32(x), f32(y)) * r * 0.5);
            if q.x < 0 || q.y < 0 || q.x >= size.x || q.y >= size.y {
                continue;
            }
            if textureLoad(depth_tex, q, 0) <= 0.0 {
                seen += 1.0;
            }
        }
    }
    return seen / 25.0;
}

fn ghost(p: vec2<f32>, at: vec2<f32>, radius: f32, soft: f32) -> f32 {
    let d = length((p - at) * vec2(u.view.z, 1.0));
    return 1.0 - smoothstep(radius * (1.0 - soft), radius, d);
}

@fragment fn fs(in: Out) -> @location(0) vec4<f32> {
    if u.sun.w < 0.5 {
        discard;
    }
    let vis = visibility();
    if vis <= 0.0 {
        discard;
    }
    let s = u.sun.xy;
    let p = in.ndc;
    // the sun's distance from the centre of the frame: ghosts sit on the line through it, mirrored
    let edge = 1.0 - smoothstep(0.9, 1.4, max(abs(s.x), abs(s.y)));
    var c = vec3<f32>(0.0);
    // ghosts: tinted discs and rings along the axis
    c += vec3(0.9, 0.6, 0.3) * ghost(p, s * -0.25, 0.05, 0.6) * 0.10;
    c += vec3(0.4, 0.7, 1.0) * ghost(p, s * -0.55, 0.11, 0.3) * 0.06;
    c += vec3(0.5, 1.0, 0.6) * ghost(p, s * -0.85, 0.04, 0.8) * 0.12;
    c += vec3(1.0, 0.5, 0.8) * (ghost(p, s * -1.25, 0.20, 0.08) - ghost(p, s * -1.25, 0.18, 0.1)) * 0.10;
    c += vec3(0.7, 0.8, 1.0) * ghost(p, s * 0.45, 0.03, 0.8) * 0.08;
    // a thin horizontal streak through the sun
    let d = (p - s) * vec2(u.view.z, 1.0);
    let streak = exp(-abs(d.y) * 260.0) * exp(-abs(d.x) * 1.8);
    c += vec3(0.8, 0.85, 1.0) * streak * 0.3;
    // a faint wide glare around the sun
    c += vec3(1.0, 0.95, 0.85) * exp(-length(d) * 6.0) * 0.12;
    return vec4(c * u.tint.rgb * u.tint.a * vis * edge, 0.0);
}
"#;

pub struct LensFlare {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    buf: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl LensFlare {
    pub fn new(device: &wgpu::Device, depth: &wgpu::TextureView) -> LensFlare {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lens flare"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lens flare"),
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lens flare"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lens flare"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::fx_post::FORMAT,
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens flare"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = Self::bind(device, &layout, &buf, depth);
        LensFlare {
            pipeline,
            layout,
            buf,
            bind,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buf: &wgpu::Buffer,
        depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lens flare"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
            ],
        })
    }

    /// The depth buffer was recreated (resize).
    pub fn resize(&mut self, device: &wgpu::Device, depth: &wgpu::TextureView) {
        self.bind = Self::bind(device, &self.layout, &self.buf, depth);
    }

    /// Draw onto the HDR scene `target` (load, additive) when the sun is in front of the camera.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        view_proj: DMat4,
        sun: (DVec3, f32, [f32; 3]),
        sun_radius_rad: f64,
        size: (u32, u32),
    ) {
        let (dir, strength, tint) = sun;
        let Some(ndc) = sun_ndc(view_proj, dir) else {
            return;
        };
        if strength <= 0.0 || ndc.x.abs() > 1.6 || ndc.y.abs() > 1.6 {
            return;
        }
        // the disc's size on screen (NDC x): project a point a disc radius off the sun
        let side = dir.any_orthonormal_vector();
        let disc = sun_ndc(view_proj, dir + side * sun_radius_rad.max(0.002))
            .map(|e| ((e - ndc).length() as f32).max(0.004))
            .unwrap_or(0.01);
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let data: [f32; 12] = [
            ndc.x as f32,
            ndc.y as f32,
            disc,
            1.0,
            tint[0],
            tint[1],
            tint[2],
            strength,
            size.0 as f32,
            size.1 as f32,
            aspect,
            0.0,
        ];
        queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(&data));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lens flare"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Where a direction (camera-relative world, at infinity) lands in NDC; `None` behind the camera.
pub fn sun_ndc(view_proj: DMat4, dir: DVec3) -> Option<glam::DVec2> {
    let c = view_proj * DVec4::new(dir.x, dir.y, dir.z, 0.0);
    (c.w > 1e-9).then(|| glam::DVec2::new(c.x / c.w, c.y / c.w))
}
