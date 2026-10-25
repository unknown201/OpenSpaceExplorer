//! Re-entry plasma around a craft: the glowing shock sheath on its leading side and the hot wake trailing back
//! into the airflow. Its strength follows the hottest part's heat flux from the server, which already carries
//! the air: dense Eve air lights it high up and slowly, thin Duna air only at speed, airless bodies never.
//!
//! The colour runs from dull orange at the onset through hot orange-pink to a violet-white core at full heat,
//! the original's look, and goes above 1 (the scene buffer is HDR) so the hottest sheath blooms. It is drawn
//! as soft additive cards (a bright cap on the craft's leading side, a streaked wake widening and cooling
//! behind it) after the scene, through `Renderer::add_plasma`.

/// Plasma strength 0..1 from the hottest part's heat flux (kW/m²): it starts at 20 and is full at 420.
pub fn strength(flux_kw_m2: f64) -> f64 {
    ((flux_kw_m2 - 20.0) / 400.0).clamp(0.0, 1.0)
}

/// The sheath's colour (linear, HDR) at strength `p`.
pub fn colour(p: f64) -> [f32; 3] {
    let p = p.clamp(0.0, 1.0);
    let lerp = |a: [f64; 3], b: [f64; 3], t: f64| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let c = if p < 0.5 {
        lerp([1.0, 0.45, 0.18], [1.0, 0.55, 0.45], p * 2.0)
    } else {
        lerp([1.0, 0.55, 0.45], [0.95, 0.8, 1.0], (p - 0.5) * 2.0)
    };
    let gain = 1.0 + 1.8 * p * p;
    [
        (c[0] * gain) as f32,
        (c[1] * gain) as f32,
        (c[2] * gain) as f32,
    ]
}

/// One craft's plasma this frame: centre (camera-relative, m), unit airflow direction past it (behind the
/// craft), the craft's radius (m), strength 0..1 and the clock (s) for the flicker.
#[derive(Clone, Copy, Debug)]
pub struct Sheath {
    pub centre: glam::DVec3,
    pub trail: glam::DVec3,
    pub radius: f64,
    pub strength: f64,
    pub time_s: f64,
}

/// How many sheaths one frame draws (the active craft and a few near it).
pub const MAX_SHEATHS: usize = 4;

#[cfg(target_arch = "wasm32")]
const SHADER: &str = r#"
struct S {
    // centre (camera-relative, m), radius (m)
    c: vec4<f32>,
    // trail direction, wake length (m)
    t: vec4<f32>,
    // colour (HDR), strength
    col: vec4<f32>,
};
struct U { view_proj: mat4x4<f32>, time: vec4<f32>, s: array<S, 4> };
@group(0) @binding(0) var<uniform> u: U;

struct Out {
    @builtin(position) p: vec4<f32>,
    // metres along the projected trail (0 at the centre) and across it
    @location(0) xy: vec2<f32>,
    @location(1) @interpolate(flat) i: u32,
    @location(2) @interpolate(flat) wake: f32,
};

@vertex fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) i: u32) -> Out {
    let s = u.s[i];
    let r = s.c.w;
    let v = normalize(s.c.xyz);
    // the trail as seen on screen: projected onto the plane facing the camera; head-on it shrinks to a glow
    var a = s.t.xyz - v * dot(s.t.xyz, v);
    let side_on = length(a);
    if side_on < 1e-3 {
        a = normalize(cross(v, vec3<f32>(0.0, 1.0, 0.3)));
    } else {
        a = a / side_on;
    }
    let b = cross(v, a);
    let wake = s.t.w * max(side_on, 0.15);
    let w = r * 1.6 + wake * 0.3;
    let corner = array<vec2<f32>, 4>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0), vec2(1.0, 1.0))[vi];
    let x = select(-r * 1.8, wake, corner.x > 0.0);
    let y = corner.y * w;
    // the card sits on the craft's near side so the glow lies over it, not behind it
    let world = s.c.xyz - v * r + a * x + b * y;
    var out: Out;
    out.p = u.view_proj * vec4<f32>(world, 1.0);
    out.xy = vec2(x, y);
    out.i = i;
    out.wake = wake;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let k = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), k.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), k.x), k.y);
}

@fragment fn fs(in: Out) -> @location(0) vec4<f32> {
    let s = u.s[in.i];
    let r = s.c.w;
    let p = s.col.w;
    let x = in.xy.x;
    let y = in.xy.y;
    // the shock in front of the craft: a shell wrapping its leading side, the craft showing through inside it
    let d = length(vec2(x, y * 0.85));
    let front = smoothstep(0.6 * r, -0.4 * r, x);
    let shell = exp(-pow((d - r) / (0.35 * r), 2.0)) * front;
    let haze = exp(-pow(d / (1.3 * r), 2.0)) * 0.12;
    // the wake: widening and cooling behind it, streaked by the flow
    let len = max(in.wake, r);
    let xn = max(x, 0.0) / len;
    let width = r * (0.9 + 2.5 * xn);
    let streak = 0.6 + 0.4 * noise(vec2(x * 0.35 - u.time.x * 40.0, y * 2.5 / r));
    let wake = exp(-pow(y / width, 2.0) * 2.2) * exp(-xn * 3.0) * smoothstep(-0.5 * r, 0.5 * r, x) * streak;
    let flicker = 0.9 + 0.1 * noise(vec2(u.time.x * 25.0, f32(in.i) * 7.0));
    let hot = s.col.rgb;
    let cool = hot * vec3(1.0, 0.45, 0.4);
    // soft card edges (the card is a little larger than the glow)
    let w = r * 1.6 + in.wake * 0.3;
    let edges = smoothstep(-1.8 * r, -1.2 * r, x) * (1.0 - smoothstep(0.75 * len, len, x))
        * (1.0 - smoothstep(0.8 * w, w, abs(y)));
    let c = hot * (shell * 0.9 + haze) + mix(hot, cool, smoothstep(0.0, 0.6, xn)) * wake * 0.7;
    return vec4(c * flicker * edges * (0.35 + 0.65 * p), 0.0);
}
"#;

/// The re-entry plasma pass: soft additive cards along each craft's airflow, drawn into the HDR scene over
/// everything but tested against its depth (terrain in front hides them).
#[cfg(target_arch = "wasm32")]
pub struct PlasmaPass {
    pipeline: wgpu::RenderPipeline,
    buf: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

#[cfg(target_arch = "wasm32")]
const UNIFORM_FLOATS: usize = 16 + 4 + MAX_SHEATHS * 12;

#[cfg(target_arch = "wasm32")]
impl PlasmaPass {
    pub fn new(device: &wgpu::Device, depth_format: wgpu::TextureFormat) -> PlasmaPass {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("re-entry plasma"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("re-entry plasma"),
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("re-entry plasma"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("re-entry plasma"),
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
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("re-entry plasma"),
            size: (UNIFORM_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("re-entry plasma"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            }],
        });
        PlasmaPass {
            pipeline,
            buf,
            bind,
        }
    }

    /// Draw `sheaths` (at most `MAX_SHEATHS`) in `pass`, whose depth attachment is the scene's (read only).
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view_proj: glam::DMat4,
        sheaths: &[Sheath],
    ) {
        let n = sheaths.len().min(MAX_SHEATHS);
        if n == 0 {
            return;
        }
        let mut data = [0f32; UNIFORM_FLOATS];
        data[..16].copy_from_slice(&view_proj.as_mat4().to_cols_array());
        data[16] = (sheaths[0].time_s % 1000.0) as f32;
        for (k, s) in sheaths.iter().take(n).enumerate() {
            let o = 20 + k * 12;
            let c = colour(s.strength);
            let t = s.trail.normalize_or_zero();
            data[o..o + 12].copy_from_slice(&[
                s.centre.x as f32,
                s.centre.y as f32,
                s.centre.z as f32,
                s.radius.max(0.5) as f32,
                t.x as f32,
                t.y as f32,
                t.z as f32,
                wake_m(s.radius, s.strength) as f32,
                c[0],
                c[1],
                c[2],
                s.strength as f32,
            ]);
        }
        queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(&data));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..4, 0..n as u32);
    }
}

/// The wake's length behind a craft of `radius` m at strength `p`: 3 radii at the onset, 12 at full heat.
pub fn wake_m(radius: f64, p: f64) -> f64 {
    radius.max(0.5) * (3.0 + 9.0 * p.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_starts_at_20_and_is_full_at_420() {
        assert_eq!(strength(0.0), 0.0);
        assert_eq!(strength(20.0), 0.0);
        assert!((strength(220.0) - 0.5).abs() < 1e-9);
        assert_eq!(strength(1000.0), 1.0);
    }

    #[test]
    fn hotter_plasma_is_brighter_and_bluer() {
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let (cool, hot) = (colour(0.1), colour(1.0));
        assert!(lum(hot) > lum(cool) * 2.0);
        assert!(hot[2] / hot[0] > cool[2] / cool[0]);
        // full heat goes above 1 so it blooms
        assert!(hot.iter().any(|&v| v > 1.0));
    }

    #[test]
    fn the_wake_grows_with_heat() {
        assert_eq!(wake_m(2.0, 0.0), 6.0);
        assert_eq!(wake_m(2.0, 1.0), 24.0);
        assert_eq!(wake_m(0.1, 0.0), 1.5);
    }
}
