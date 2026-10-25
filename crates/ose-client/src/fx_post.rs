//! HDR scene storage, refraction snapshot, and bloom for emissive exhaust.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub struct FxPost {
    pub scene: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub snapshot: wgpu::Texture,
    pub snapshot_view: wgpu::TextureView,
    width: u32,
    height: u32,
    bloom_a: wgpu::TextureView,
    bloom_b: wgpu::TextureView,
    extract: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
    scene_bind: wgpu::BindGroup,
    a_bind: wgpu::BindGroup,
    b_bind: wgpu::BindGroup,
    compose_bind: wgpu::BindGroup,
    grid: wgpu::RenderPipeline,
    grid_bind: wgpu::BindGroup,
}
const SHADER: &str = r#"
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var bloom:texture_2d<f32>;
@group(0) @binding(2) var samp:sampler;
struct Out { @builtin(position) p:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i:u32)->Out {
 let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0))[i];
 return Out(vec4(p,0.0,1.0),p*vec2(0.5,-0.5)+0.5);
}
@fragment fn extract(in:Out)->@location(0) vec4<f32> {
 let size=vec2<f32>(textureDimensions(image)); var c=vec3<f32>(0.0);
 for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){
  let s=textureSampleLevel(image,samp,in.uv+vec2<f32>(f32(x),f32(y))/size,0.0).rgb;
  c+=max(s-vec3<f32>(0.8),vec3<f32>(0.0));
 }} return vec4(c/9.0,1.0);
}
fn blur(uv:vec2<f32>,axis:vec2<f32>)->vec4<f32> {
 let step=axis/vec2<f32>(textureDimensions(image));
 var c=textureSampleLevel(image,samp,uv,0.0)*0.227027;
 c+=(textureSampleLevel(image,samp,uv+step*1.384615,0.0)+textureSampleLevel(image,samp,uv-step*1.384615,0.0))*0.316216;
 c+=(textureSampleLevel(image,samp,uv+step*3.230769,0.0)+textureSampleLevel(image,samp,uv-step*3.230769,0.0))*0.070270;
 return c;
}
@fragment fn blur_h(in:Out)->@location(0) vec4<f32>{return blur(in.uv,vec2(1.0,0.0));}
@fragment fn blur_v(in:Out)->@location(0) vec4<f32>{return blur(in.uv,vec2(0.0,1.0));}
@fragment fn compose(in:Out)->@location(0) vec4<f32> {
 let c=textureSampleLevel(image,samp,in.uv,0.0).rgb;
 let glow=textureSampleLevel(bloom,samp,in.uv,0.0).rgb*0.16;
 return vec4(c+glow,1.0);
}
@fragment fn grid(in:Out)->@location(0) vec4<f32> {
 let size=vec2<f32>(textureDimensions(image));
 let pixel=in.p.xy;
 let checker=f32((u32(pixel.x)/16u+u32(pixel.y)/16u)%2u);
 // Exactly representable in f16 at the fixture's 512px resolution.
 return vec4(0.125+0.25*checker,0.125+0.5*pixel.y/size.y,0.125+0.5*pixel.x/size.x,1.0);
}
"#;
impl FxPost {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface: wgpu::TextureFormat,
    ) -> Self {
        let make = |label, w, h, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        let scene = make(
            "HDR scene",
            width,
            height,
            sampled | wgpu::TextureUsages::COPY_SRC,
        );
        let snapshot = make(
            "refraction snapshot",
            width,
            height,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let view = scene.create_view(&Default::default());
        let snapshot_view = snapshot.create_view(&Default::default());
        let bloom_a = make("bloom A", (width / 4).max(1), (height / 4).max(1), sampled)
            .create_view(&Default::default());
        let bloom_b = make("bloom B", (width / 4).max(1), (height / 4).max(1), sampled)
            .create_view(&Default::default());
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post textures"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(b),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let scene_bind = bind(&view, &view);
        let a_bind = bind(&bloom_a, &bloom_a);
        let b_bind = bind(&bloom_b, &bloom_b);
        let compose_bind = bind(&view, &bloom_a);
        let grid_bind = bind(&snapshot_view, &bloom_a);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exhaust post"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
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
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let extract = pipeline("extract", FORMAT);
        let blur_h = pipeline("blur_h", FORMAT);
        let blur_v = pipeline("blur_v", FORMAT);
        let compose = pipeline("compose", surface);
        let grid = pipeline("grid", FORMAT);
        Self {
            scene,
            view,
            snapshot,
            snapshot_view,
            width,
            height,
            bloom_a,
            bloom_b,
            extract,
            blur_h,
            blur_v,
            compose,
            scene_bind,
            a_bind,
            b_bind,
            compose_bind,
            grid,
            grid_bind,
        }
    }
    pub fn reference_grid(&self, encoder: &mut wgpu::CommandEncoder) {
        // Bind the snapshot as a size-only input, avoiding a sample/render attachment alias.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("refraction reference grid"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
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
        pass.set_pipeline(&self.grid);
        pass.set_bind_group(0, &self.grid_bind, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn snapshot(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.scene,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.snapshot,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }
    pub fn finish(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        for (target, pipeline, bind) in [
            (&self.bloom_a, &self.extract, &self.scene_bind),
            (&self.bloom_b, &self.blur_h, &self.a_bind),
            (&self.bloom_a, &self.blur_v, &self.b_bind),
            (output, &self.compose, &self.compose_bind),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("exhaust bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
