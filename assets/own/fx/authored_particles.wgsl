struct Camera { vp:mat4x4<f32>, light:vec4<f32>, params:vec4<f32> };
@group(0) @binding(0) var<uniform> camera:Camera;
@group(1) @binding(0) var albedo:texture_2d<f32>;
@group(1) @binding(1) var normal_map:texture_2d<f32>;
@group(1) @binding(2) var emission_map:texture_2d<f32>;
@group(1) @binding(3) var samp:sampler;
@group(2) @binding(0) var scene:texture_2d<f32>;
@group(2) @binding(1) var depth:texture_depth_2d;
@group(2) @binding(2) var screen_samp:sampler;
struct Instance {
 @location(3) position:vec4<f32>, @location(4) axis_x:vec4<f32>, @location(5) axis_y:vec4<f32>,
 @location(6) axis_z:vec4<f32>, @location(7) color:vec4<f32>,
 @location(8) uv0:vec4<f32>, @location(9) uv1:vec4<f32>, @location(10) emission:vec4<f32>,
 @location(11) params:vec4<f32>, @location(12) lighting:vec4<f32>,
};
struct Mesh { @location(0) position:vec3<f32>, @location(1) normal:vec3<f32>, @location(2) uv:vec2<f32> };
struct Out {
 @builtin(position) clip:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) next:vec2<f32>,
 @location(2) color:vec4<f32>, @location(3) normal:vec3<f32>, @location(4) tangent:vec3<f32>,
 @location(5) bitangent:vec3<f32>, @location(6) params:vec4<f32>, @location(7) emission:vec4<f32>,
 @location(8) lighting:vec4<f32>, @location(9) linear_depth:f32,
};
fn vertex(in:Instance,position:vec3<f32>,normal:vec3<f32>,uv:vec2<f32>)->Out {
 let world=in.position.xyz+in.axis_x.xyz*position.x+in.axis_y.xyz*position.y+in.axis_z.xyz*position.z;
 var out:Out;out.clip=camera.vp*vec4(world,1.0);out.linear_depth=out.clip.w;
 out.uv=in.uv0.xy+uv*in.uv0.zw;out.next=in.uv1.xy+uv*in.uv1.zw;
 out.color=in.color;out.normal=normalize(in.axis_x.xyz*normal.x+in.axis_y.xyz*normal.y+in.axis_z.xyz*normal.z);
 out.tangent=normalize(in.axis_x.xyz);out.bitangent=normalize(in.axis_y.xyz);
 out.params=in.params;out.emission=in.emission;out.lighting=in.lighting;return out;
}
@vertex fn vs_sprite(in:Instance,@builtin(vertex_index) index:u32)->Out {
 let p=array<vec2<f32>,6>(vec2(-0.5,-0.5),vec2(0.5,-0.5),vec2(-0.5,0.5),vec2(-0.5,0.5),vec2(0.5,-0.5),vec2(0.5,0.5))[index];
 return vertex(in,vec3(p,0.0),vec3(0.0,0.0,1.0),p+0.5);
}
@vertex fn vs_mesh(mesh:Mesh,in:Instance)->Out {return vertex(in,mesh.position,mesh.normal,mesh.uv);}
@fragment fn fs(in:Out)->@location(0) vec4<f32> {
 let blend=in.params.w;
 let base=mix(textureSample(albedo,samp,in.uv),textureSample(albedo,samp,in.next),blend)*in.color;
 var n=normalize(in.normal);
 if in.params.z>0.5 {
  let tex=mix(textureSample(normal_map,samp,in.uv),textureSample(normal_map,samp,in.next),blend);
  let xy=(vec2(tex.r*tex.a,tex.g)*2.0-1.0)*in.lighting.w;
  n=normalize(in.tangent*xy.x+in.bitangent*xy.y+n*sqrt(max(1.0-dot(xy,xy),0.0)));
 }
 var lighting=vec3<f32>(1.0);
 if in.params.x>0.5 {
  // RenderKit's wrap denominator is squared; back scattering is a separate lobe.
  let nl=dot(n,normalize(camera.light.xyz));
  let wrap=1.0+in.lighting.x;
  let diffuse=clamp((nl+in.lighting.x)/(wrap*wrap),0.0,1.0);
  let scatter=clamp((-nl+0.1)*0.826446,0.0,1.0)*in.lighting.z;
  lighting=vec3(camera.params.y+clamp(diffuse+scatter,0.0,1.0)*clamp(2.0*in.lighting.y,0.0,1.0));
 }
 let emit=mix(textureSample(emission_map,samp,in.uv),textureSample(emission_map,samp,in.next),blend).rgb*in.emission.rgb;
 var alpha=base.a;
 if in.params.y>0.0 {
  let size=vec2<i32>(textureDimensions(depth));let texel=clamp(vec2<i32>(in.clip.xy),vec2<i32>(0),size-1);
  let scene_depth=camera.params.x/max(textureLoad(depth,texel,0),1e-20);
  alpha*=clamp((scene_depth-in.linear_depth)*in.params.y,0.0,1.0);
 }
 return vec4(base.rgb*lighting+emit,alpha);
}
