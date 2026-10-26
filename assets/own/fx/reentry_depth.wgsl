// Hidden/VFX/ReentryPartDepth: radial camera distance, normalized between near and far.
struct Uniform { vp:mat4x4<f32>, model:mat4x4<f32>, camera:vec4<f32>, range:vec4<f32> };
@group(0) @binding(0) var<uniform> u:Uniform;
struct Out { @builtin(position) clip:vec4<f32>, @location(0) world:vec3<f32> };
@vertex fn vs(@location(0) position:vec3<f32>)->Out {
 let world=u.model*vec4(position,1.0);var out:Out;
 out.clip=u.vp*world;out.world=world.xyz;return out;
}
@fragment fn fs(in:Out)->@location(0) f32 {
 return clamp((distance(in.world,u.camera.xyz)-u.range.x)/(u.range.y-u.range.x),0.0,1.0);
}
