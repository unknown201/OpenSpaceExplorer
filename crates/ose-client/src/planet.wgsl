// Planet rendering: sky, terrain, ocean and cloud shell (planet.rs, docs/reference-notes/planet-rendering.md).
//
// Frame: the body's model frame, camera-relative in metres for geometry; the atmosphere functions work in
// kilometres around the body's centre, as the original's Bruneton LUTs do. The atmosphere is a WGSL port of
// Eric Bruneton's "Precomputed Atmospheric Scattering" (2017, BSD) reading the LUTs the original baked.

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // camera position in the model frame (km), w: time (s)
    cam_km: vec4<f32>,
    // direction to the sun (model frame), w: exposure
    sun: vec4<f32>,
    // bottom radius (km), top radius (km), mu_s_min, Mie g
    atmo: vec4<f32>,
    // Rayleigh scattering per km, w: 1 when the body has an atmosphere
    rayleigh: vec4<f32>,
    // Mie scattering per km, w: sun angular radius (rad)
    mie: vec4<f32>,
    // solar irradiance, w: body radius (m)
    solar: vec4<f32>,
    // x: camera height above the sea surface (m), y: 1 on ocean worlds
    water: vec4<f32>,
    // the water's colour under the surface (shallow and deep mixed)
    water_col: vec4<f32>,
    // bodies that can hide the sun: centre (km, model frame), radius (km)
    occluders: array<vec4<f32>, 4>,
    // x: the star's angular radius (rad), y: number of occluders
    eclipse: vec4<f32>,
    // FFT waves: x 1 when live, y/z long and short patch sizes (m), w: chop
    waves: vec4<f32>,
    // the star skybox's x and z axes in the model frame (y is the body's north)
    star_x: vec4<f32>,
    star_z: vec4<f32>,
    // the camera's model-frame position modulo TILE_PERIOD (m)
    cam_tile: vec4<f32>,
    // planet detail: x terrain shadow steps, y volume cloud steps, z per-pixel relief (0/1); w: the props'
    // sunlight scale (matching the terrain's exposure)
    quality: vec4<f32>,
};

struct Chunk {
    // quad centre minus camera (m)
    offset: vec4<f32>,
    // quad centre in the model frame (m), w: level
    center: vec4<f32>,
    // quad centre modulo the tiling period (m), w: 1 for the cloud shell
    tile: vec4<f32>,
};

struct Surface {
    slots: array<vec4<f32>, 4>,
    height_enable: array<vec4<f32>, 4>,
    slope_enable: array<vec4<f32>, 4>,
    height_params: array<vec4<f32>, 16>,
    slope_params: array<vec4<f32>, 16>,
    uv_scale: array<vec4<f32>, 4>,
    // scaled albedo fade (start m, end m, min, max)
    albedo_fade: vec4<f32>,
    // scaled normal fade (start m, end m, min, max)
    normal_fade: vec4<f32>,
    // deep water colour, w: shallow max depth (m)
    deep: vec4<f32>,
    // shallow water colour, w: smoothness
    shallow: vec4<f32>,
    // cloud base height (m), cloud density, has clouds, detail slices
    clouds: vec4<f32>,
    // has detail (biome) textures, has scaled normal, debug view, ocean altitude (m)
    flags: vec4<f32>,
    // the original's ambient colour by day, w: 1 for airless bodies
    ambient_day: vec4<f32>,
    ambient_night: vec4<f32>,
    // shoreline foam: max depth (m), tiling (m), has foam texture, 0
    foam: vec4<f32>,
    // caustics: strength, max visible depth (m), tiling (m), animation speed
    caustics: vec4<f32>,
    // cloud layer top (m), wind speed (m/s), flow speed, flow strength
    clouds2: vec4<f32>,
    // polar caps: has albedo caps, has cloud caps, cap radius (rad)
    poles: vec4<f32>,
    // the terrain pack's eight tiled height layers: (uv scale, height scale) pairs
    layers: array<vec4<f32>, 4>,
    // x: 1 with layer slopes, y: side/top projection contrast
    layer_info: vec4<f32>,
    // terrain shadows: x 1 with the global height map, y its scale (m), z its offset (m)
    shadow: vec4<f32>,
    // x: the cloud colour map's intensity (clouds RGB; 1 for white clouds)
    cloud_col: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var transmittance_tex: texture_2d<f32>;
@group(0) @binding(2) var scattering_tex: texture_3d<f32>;
@group(0) @binding(3) var irradiance_tex: texture_2d<f32>;
@group(0) @binding(4) var lut_samp: sampler;
// The sky pass's own group 1 (quads use binding 0 of group 1 for their chunk): the original's starfield skybox.
@group(1) @binding(1) var star_tex: texture_2d<f32>;
@group(1) @binding(2) var star_samp: sampler;
// A ring pass's own group 1 (Dres): inner and outer radius (m), alpha edge; the radial texture.
struct RingU { radii: vec4<f32> };
@group(1) @binding(3) var<uniform> ring: RingU;
@group(1) @binding(4) var ring_tex: texture_2d<f32>;
@group(1) @binding(5) var ring_samp: sampler;
const RING_SEGMENTS: u32 = 256u;
// FFT waves (ocean_fft.rs): layer 0 x/z displacement, height, Jacobian of the long patch; layer 1 its slopes
// and the short patch's slopes.
@group(0) @binding(5) var waves_tex: texture_2d_array<f32>;

@group(1) @binding(0) var<uniform> chunk: Chunk;

@group(2) @binding(0) var<uniform> surface: Surface;
@group(2) @binding(1) var albedo_tex: texture_2d<f32>;
@group(2) @binding(2) var normal_tex: texture_2d<f32>;
@group(2) @binding(3) var biome_tex: texture_2d<f32>;
@group(2) @binding(4) var detail_a: texture_2d_array<f32>;
@group(2) @binding(5) var detail_n: texture_2d_array<f32>;
@group(2) @binding(6) var clouds_tex: texture_2d<f32>;
@group(2) @binding(7) var wrap_samp: sampler;
@group(2) @binding(8) var equi_samp: sampler;
@group(2) @binding(9) var foam_tex: texture_2d<f32>;
// Polar caps of the albedo (RGB) and cloud (A) maps (tools/export_poles.py): north left, south right.
@group(2) @binding(10) var pole_albedo_tex: texture_2d<f32>;
// The global height map (normalised, row 0 at the south pole), for terrain shadows.
@group(2) @binding(11) var global_height_tex: texture_2d<f32>;
// The terrain pack's layer slopes (d height / d tile u, v per layer, ose_data LayerSlopes), with mips.
@group(2) @binding(12) var slopes_tex: texture_2d_array<f32>;
// The pack's own layer mask (not the scaled biome mask): row 0 at the south pole.
@group(2) @binding(13) var layer_mask_tex: texture_2d<f32>;

const PI: f32 = 3.14159265;
const TRANSMITTANCE_W: f32 = 1024.0;
const TRANSMITTANCE_H: f32 = 128.0;
const SCATTERING_R: f32 = 32.0;
const SCATTERING_MU: f32 = 128.0;
const SCATTERING_MU_S: f32 = 32.0;
const SCATTERING_NU: f32 = 8.0;
const IRRADIANCE_W: f32 = 64.0;
const IRRADIANCE_H: f32 = 16.0;
// Tiling period of the detail textures and waves (m); the CPU passes each quad's centre modulo this.
const TILE_PERIOD: f32 = 4096.0;

// ---------------------------------------------------------------------------------------------------------
// Bruneton atmosphere

fn bottom_r() -> f32 { return frame.atmo.x; }
fn top_r() -> f32 { return frame.atmo.y; }

fn safe_sqrt(a: f32) -> f32 { return sqrt(max(a, 0.0)); }

fn dist_to_top(r: f32, mu: f32) -> f32 {
    let disc = r * r * (mu * mu - 1.0) + top_r() * top_r();
    return max(-r * mu + safe_sqrt(disc), 0.0);
}

fn ray_hits_ground(r: f32, mu: f32) -> bool {
    return mu < 0.0 && r * r * (mu * mu - 1.0) + bottom_r() * bottom_r() >= 0.0;
}

fn unit_to_coord(x: f32, size: f32) -> f32 { return 0.5 / size + x * (1.0 - 1.0 / size); }

fn transmittance_uv(r: f32, mu: f32) -> vec2<f32> {
    let h = safe_sqrt(top_r() * top_r() - bottom_r() * bottom_r());
    let rho = safe_sqrt(r * r - bottom_r() * bottom_r());
    let d = dist_to_top(r, mu);
    let d_min = top_r() - r;
    let d_max = rho + h;
    let x_mu = (d - d_min) / max(d_max - d_min, 1e-6);
    let x_r = rho / h;
    return vec2<f32>(unit_to_coord(x_mu, TRANSMITTANCE_W), unit_to_coord(x_r, TRANSMITTANCE_H));
}

fn transmittance_to_top(r: f32, mu: f32) -> vec3<f32> {
    return textureSampleLevel(transmittance_tex, lut_samp, transmittance_uv(r, mu), 0.0).rgb;
}

fn transmittance(r: f32, mu: f32, d: f32, hits_ground: bool) -> vec3<f32> {
    let r_d = clamp(sqrt(max(d * d + 2.0 * r * mu * d + r * r, 0.0)), bottom_r(), top_r());
    let mu_d = clamp((r * mu + d) / r_d, -1.0, 1.0);
    if hits_ground {
        return min(transmittance_to_top(r_d, -mu_d) / max(transmittance_to_top(r, -mu), vec3<f32>(1e-6)), vec3<f32>(1.0));
    }
    return min(transmittance_to_top(r, mu) / max(transmittance_to_top(r_d, mu_d), vec3<f32>(1e-6)), vec3<f32>(1.0));
}

fn transmittance_to_sun(r: f32, mu_s: f32) -> vec3<f32> {
    let sin_h = bottom_r() / r;
    let cos_h = -safe_sqrt(1.0 - sin_h * sin_h);
    let a = frame.mie.w;
    return transmittance_to_top(r, mu_s) * smoothstep(-sin_h * a, sin_h * a, mu_s - cos_h);
}

fn scattering_uvwz(r: f32, mu: f32, mu_s: f32, nu: f32, hits_ground: bool) -> vec4<f32> {
    let h = safe_sqrt(top_r() * top_r() - bottom_r() * bottom_r());
    let rho = safe_sqrt(r * r - bottom_r() * bottom_r());
    let u_r = unit_to_coord(rho / h, SCATTERING_R);
    let r_mu = r * mu;
    let disc = r_mu * r_mu - r * r + bottom_r() * bottom_r();
    var u_mu: f32;
    if hits_ground {
        let d = -r_mu - safe_sqrt(disc);
        let d_min = r - bottom_r();
        let d_max = rho;
        var x = 0.0;
        if d_max != d_min {
            x = (d - d_min) / (d_max - d_min);
        }
        u_mu = 0.5 - 0.5 * unit_to_coord(x, SCATTERING_MU / 2.0);
    } else {
        let d = -r_mu + safe_sqrt(disc + h * h);
        let d_min = top_r() - r;
        let d_max = rho + h;
        u_mu = 0.5 + 0.5 * unit_to_coord((d - d_min) / max(d_max - d_min, 1e-6), SCATTERING_MU / 2.0);
    }
    let d = dist_to_top(bottom_r(), mu_s);
    let d_min = top_r() - bottom_r();
    let d_max = h;
    let a = (d - d_min) / (d_max - d_min);
    let big_d = dist_to_top(bottom_r(), frame.atmo.z);
    let big_a = (big_d - d_min) / (d_max - d_min);
    let u_mu_s = unit_to_coord(max(1.0 - a / big_a, 0.0) / (1.0 + a), SCATTERING_MU_S);
    let u_nu = (nu + 1.0) / 2.0;
    return vec4<f32>(u_nu, u_mu_s, u_mu, u_r);
}

struct Scatter { rayleigh: vec3<f32>, mie: vec3<f32> };

fn combined_scattering(r: f32, mu: f32, mu_s: f32, nu: f32, hits_ground: bool) -> Scatter {
    let uvwz = scattering_uvwz(r, mu, mu_s, nu, hits_ground);
    let tex_x = uvwz.x * (SCATTERING_NU - 1.0);
    let x0 = floor(tex_x);
    let l = tex_x - x0;
    let uvw0 = vec3<f32>((x0 + uvwz.y) / SCATTERING_NU, uvwz.z, uvwz.w);
    let uvw1 = vec3<f32>((x0 + 1.0 + uvwz.y) / SCATTERING_NU, uvwz.z, uvwz.w);
    let c = mix(textureSampleLevel(scattering_tex, lut_samp, uvw0, 0.0),
                textureSampleLevel(scattering_tex, lut_samp, uvw1, 0.0), l);
    var s: Scatter;
    s.rayleigh = c.rgb;
    if c.r <= 0.0 {
        s.mie = vec3<f32>(0.0);
    } else {
        s.mie = c.rgb * c.a / c.r * (frame.rayleigh.x / max(frame.mie.x, 1e-9)) * (frame.mie.xyz / max(frame.rayleigh.xyz, vec3<f32>(1e-9)));
    }
    return s;
}

fn rayleigh_phase(nu: f32) -> f32 { return 3.0 / (16.0 * PI) * (1.0 + nu * nu); }

fn mie_phase(g: f32, nu: f32) -> f32 {
    let k = 3.0 / (8.0 * PI) * (1.0 - g * g) / (2.0 + g * g);
    return k * (1.0 + nu * nu) / pow(max(1.0 + g * g - 2.0 * g * nu, 1e-4), 1.5);
}

struct Sky { radiance: vec3<f32>, transmittance: vec3<f32> };

// Sky radiance along a ray from `camera` (km) that leaves the atmosphere or hits the ground.
fn sky_radiance(camera_in: vec3<f32>, view: vec3<f32>, sun: vec3<f32>) -> Sky {
    var out: Sky;
    var camera = camera_in;
    var r = length(camera);
    var rmu = dot(camera, view);
    let dtt = -rmu - safe_sqrt(rmu * rmu - r * r + top_r() * top_r());
    if dtt > 0.0 {
        camera = camera + view * dtt;
        r = top_r();
        rmu = rmu + dtt;
    } else if r > top_r() {
        out.radiance = vec3<f32>(0.0);
        out.transmittance = vec3<f32>(1.0);
        return out;
    }
    r = max(r, bottom_r() + 0.001);
    let mu = rmu / r;
    let mu_s = dot(camera, sun) / r;
    let nu = dot(view, sun);
    let hits = ray_hits_ground(r, mu);
    if hits {
        out.transmittance = vec3<f32>(0.0);
    } else {
        out.transmittance = transmittance_to_top(r, mu);
    }
    let s = combined_scattering(r, mu, mu_s, nu, hits);
    out.radiance = s.rayleigh * rayleigh_phase(nu) + s.mie * mie_phase(frame.atmo.w, nu);
    return out;
}

// In-scattered radiance and transmittance between `camera` and `point` (km).
fn sky_to_point(camera_in: vec3<f32>, point: vec3<f32>, sun: vec3<f32>) -> Sky {
    var out: Sky;
    var camera = camera_in;
    let view = normalize(point - camera);
    var r = length(camera);
    var rmu = dot(camera, view);
    let dtt = -rmu - safe_sqrt(rmu * rmu - r * r + top_r() * top_r());
    if dtt > 0.0 {
        camera = camera + view * dtt;
        r = top_r();
        rmu = rmu + dtt;
    } else if r > top_r() {
        out.radiance = vec3<f32>(0.0);
        out.transmittance = vec3<f32>(1.0);
        return out;
    }
    r = max(r, bottom_r() + 0.001);
    let mu = clamp(rmu / r, -1.0, 1.0);
    let mu_s = dot(camera, sun) / r;
    let nu = dot(view, sun);
    let d = length(point - camera);
    let hits = ray_hits_ground(r, mu);
    out.transmittance = transmittance(r, mu, d, hits);
    let s0 = combined_scattering(r, mu, mu_s, nu, hits);
    let r_p = clamp(sqrt(max(d * d + 2.0 * r * mu * d + r * r, 0.0)), bottom_r(), top_r());
    let mu_p = clamp((r * mu + d) / r_p, -1.0, 1.0);
    let mu_s_p = clamp((r * mu_s + d * nu) / r_p, -1.0, 1.0);
    let s1 = combined_scattering(r_p, mu_p, mu_s_p, nu, hits);
    let ray = max(s0.rayleigh - out.transmittance * s1.rayleigh, vec3<f32>(0.0));
    var mie = max(s0.mie - out.transmittance * s1.mie, vec3<f32>(0.0));
    mie = mie * smoothstep(0.0, 0.01, mu_s);
    out.radiance = ray * rayleigh_phase(nu) + mie * mie_phase(frame.atmo.w, nu);
    return out;
}

fn irradiance(r: f32, mu_s: f32) -> vec3<f32> {
    let x_r = (r - bottom_r()) / (top_r() - bottom_r());
    let x_mu_s = mu_s * 0.5 + 0.5;
    let uv = vec2<f32>(unit_to_coord(x_mu_s, IRRADIANCE_W), unit_to_coord(x_r, IRRADIANCE_H));
    return textureSampleLevel(irradiance_tex, lut_samp, uv, 0.0).rgb;
}

struct Light { sun: vec3<f32>, sky: vec3<f32> };

// Fraction of the star's disc not hidden by other bodies, seen from `p` (km): moons and planets passing in front
// of the sun (the disc overlap of the two, smoothed between first and full contact).
fn eclipse(p: vec3<f32>, sun: vec3<f32>) -> f32 {
    var vis = 1.0;
    let a_s = frame.eclipse.x;
    for (var i = 0; i < 4; i = i + 1) {
        if f32(i) >= frame.eclipse.y {
            break;
        }
        let o = frame.occluders[i];
        let d = o.xyz - p;
        let dist = length(d);
        if dist <= o.w || dot(d, sun) <= 0.0 {
            continue;
        }
        let a_o = asin(clamp(o.w / dist, 0.0, 1.0));
        let theta = acos(clamp(dot(d / dist, sun), -1.0, 1.0));
        let full = abs(a_o - a_s);
        // covered fraction at the deepest point: all of it, or the ratio of the disc areas
        let deepest = select(1.0, (a_o * a_o) / (a_s * a_s), a_o < a_s);
        let t = smoothstep(full, a_s + a_o, theta);
        vis = vis * (1.0 - deepest * (1.0 - t));
    }
    return vis;
}

// Direct sun and sky irradiance on a surface at `p` (km) with normal `n`.
fn sun_and_sky(p: vec3<f32>, n: vec3<f32>, sun: vec3<f32>) -> Light {
    var out: Light;
    let has_atmo = frame.rayleigh.w > 0.5;
    let ndl = max(dot(n, sun), 0.0) * eclipse(p, sun);
    if !has_atmo {
        out.sun = frame.solar.xyz * ndl;
        out.sky = vec3<f32>(0.0);
        return out;
    }
    let r = clamp(length(p), bottom_r() + 0.001, top_r());
    let mu_s = dot(p, sun) / length(p);
    out.sky = irradiance(r, mu_s) * (1.0 + dot(n, p) / length(p)) * 0.5;
    out.sun = frame.solar.xyz * transmittance_to_sun(r, mu_s) * ndl;
    return out;
}

// Shade of the cloud layer on a point (model frame, m): where the ray towards the sun crosses the cloud base,
// the cloud cover there dims the direct sunlight (the original's cloud shadows, without the volume).
fn cloud_shadow(model: vec3<f32>, sun: vec3<f32>) -> f32 {
    if surface.clouds.z < 0.5 {
        return 1.0;
    }
    let up = normalize(model);
    let r_cloud = frame.solar.w + surface.clouds.x;
    let r = length(model);
    if r > r_cloud {
        return 1.0;
    }
    let cos_s = dot(up, sun);
    if cos_s <= 0.02 {
        return 1.0;
    }
    // distance along the sun ray to the cloud sphere
    let b = dot(model, sun);
    let t = -b + sqrt(max(b * b - r * r + r_cloud * r_cloud, 0.0));
    let hit = normalize(model + sun * t);
    let c = textureSampleLevel(clouds_tex, equi_samp, equi_uv(hit), 4.0);
    let cover = clamp(c.a * surface.clouds.y, 0.0, 1.0);
    return 1.0 - 0.75 * cover;
}

// Ambient light (the original's CelestialBodyLighting): by day on airless bodies (the sky lights the others),
// at night everywhere, blended over the terminator like its dayBlendRange.
fn ambient(up: vec3<f32>, sun: vec3<f32>) -> vec3<f32> {
    let day = smoothstep(-0.01, 0.2, dot(up, sun));
    let d = surface.ambient_day.rgb * surface.ambient_day.w;
    return mix(surface.ambient_night.rgb, d, day) * (2.0 / PI);
}

fn under_water() -> bool {
    return frame.water.y > 0.5 && frame.water.x < 0.0;
}

// Light under the sea (the original's _UnderWaterDiffuseColor, darker with depth) and the fog it puts between
// the camera and what it sees (frame.water_col.w m: 60 % of the body's _TransparentVisibleDepth).
fn water_light(depth_m: f32) -> vec3<f32> {
    let sun_up = max(dot(normalize(frame.cam_km.xyz), frame.sun.xyz), 0.0);
    let col = frame.water_col.rgb;
    return col * (0.08 + 0.6 * sun_up) * exp(-max(depth_m, 0.0) / max(frame.water_col.w, 8.0));
}

fn water_fog(radiance: vec3<f32>, dist: f32) -> vec3<f32> {
    let f = 1.0 - exp(-dist / max(frame.water_col.w, 1.0));
    return mix(radiance, water_light(-frame.water.x) / PI * frame.solar.xyz, f);
}

fn tonemap(radiance: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - exp(-radiance * frame.sun.w);
}

// Aerial perspective: the radiance reflected at `p` (km) as seen from the camera.
fn through_air(radiance: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    if frame.rayleigh.w < 0.5 {
        return radiance;
    }
    let s = sky_to_point(frame.cam_km.xyz, p, frame.sun.xyz);
    // From high up the original's ground stays vivid under its haze (its aerial perspective is a camera volume
    // of limited reach): the in-scatter over the ground is eased to 60 % as the camera climbs from 10 to 40 km.
    let alt_km = length(frame.cam_km.xyz) - bottom_r();
    let haze = 1.0 - 0.4 * smoothstep(10.0, 40.0, alt_km);
    // an eclipse dims the sunlit air too
    return radiance * s.transmittance + s.radiance * haze * eclipse(frame.cam_km.xyz, frame.sun.xyz);
}

// ---------------------------------------------------------------------------------------------------------
// Sky: a full-screen triangle behind everything

struct SkyOut { @builtin(position) clip: vec4<f32>, @location(0) ndc: vec2<f32> };

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    var out: SkyOut;
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    out.clip = vec4<f32>(xy, 0.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let p = frame.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let view = normalize(p.xyz / p.w);
    let sun = frame.sun.xyz;
    if under_water() {
        let up = normalize(frame.cam_km.xyz);
        let looking_up = clamp(dot(view, up), 0.0, 1.0);
        // the fog colour far away (so the sea floor fades into it without an edge), brighter towards the surface
        let c = water_light(-frame.water.x) / PI * frame.solar.xyz * (1.0 + 1.5 * looking_up * looking_up);
        return vec4<f32>(tonemap(c), 1.0);
    }
    var radiance = vec3<f32>(0.0);
    var t = vec3<f32>(1.0);
    if frame.rayleigh.w > 0.5 {
        let s = sky_radiance(frame.cam_km.xyz, view, sun);
        radiance = s.radiance * eclipse(frame.cam_km.xyz, sun);
        t = s.transmittance;
    }
    // The stars behind the air: the skybox at about its own brightness after the tone map in space, seen through
    // the atmosphere's transmittance and washed out by a daytime sky.
    let sv = vec3<f32>(dot(view, frame.star_x.xyz), view.y, dot(view, frame.star_z.xyz));
    let suv = vec2<f32>(atan2(sv.z, sv.x) / (2.0 * PI) + 0.5, 0.5 - asin(clamp(sv.y, -1.0, 1.0)) / PI);
    let star = textureSampleLevel(star_tex, star_samp, suv, 0.0).rgb;
    radiance = radiance + t * star / max(frame.sun.w, 0.1);
    // The sun: its disc written far above 1 into the HDR target, so the post bloom glows around it as the
    // original's sun does, and a soft corona (glare in the eye or lens) tinted by the air it shines through.
    let a = frame.mie.w;
    let ang = acos(clamp(dot(view, sun), -1.0, 1.0));
    let tint = t / max(max(t.r, max(t.g, t.b)), 1e-4) * smoothstep(0.0, 0.2, max(max(t.r, t.g), t.b));
    var hdr = vec3<f32>(0.0);
    if ang < a {
        hdr = tint * 24.0;
    }
    let corona = tint * (0.55 * exp(-max(ang - a, 0.0) / 0.012) + 0.12 * exp(-max(ang - a, 0.0) / 0.08));
    return vec4<f32>(tonemap(radiance) + corona + hdr, 1.0);
}

// ---------------------------------------------------------------------------------------------------------
// Rings (the original's planet ring: Dres, 414 to 440 km, a radial colour and alpha texture): a flat strip in the
// equatorial plane, lit by the sun from either side, in the body's shadow where the sun ray crosses it.

struct RingOut { @builtin(position) clip: vec4<f32>, @location(0) model: vec3<f32> };

@vertex
fn vs_ring(@builtin(vertex_index) i: u32) -> RingOut {
    var out: RingOut;
    let a = f32(i / 2u) / f32(RING_SEGMENTS) * 2.0 * PI;
    let r = select(ring.radii.x, ring.radii.y, (i % 2u) == 1u);
    let model = vec3<f32>(cos(a) * r, 0.0, sin(a) * r);
    out.model = model;
    out.clip = frame.view_proj * vec4<f32>(model - frame.cam_km.xyz * 1000.0, 1.0);
    return out;
}

@fragment
fn fs_ring(in: RingOut) -> @location(0) vec4<f32> {
    let r = length(in.model.xz);
    let t = clamp((r - ring.radii.x) / max(ring.radii.y - ring.radii.x, 1.0), 0.0, 1.0);
    let c = textureSampleLevel(ring_tex, ring_samp, vec2<f32>(t, 0.5), 0.0);
    let e = ring.radii.z;
    let alpha = c.a * smoothstep(0.0, e, t) * smoothstep(0.0, e, 1.0 - t);
    if alpha < 0.002 {
        discard;
    }
    let sun = frame.sun.xyz;
    // the body's shadow: the sun ray from here passes within its radius, in front of the ring point
    let b = dot(in.model, sun);
    let closest = length(in.model - sun * b);
    let shadow = select(1.0, smoothstep(frame.solar.w * 0.97, frame.solar.w * 1.03, closest), b < 0.0);
    let lit = frame.solar.xyz * (0.25 + 0.75 * abs(sun.y)) * shadow * eclipse(in.model * 0.001, sun);
    let radiance = c.rgb / PI * lit + c.rgb * 0.01;
    return vec4<f32>(tonemap(radiance) * alpha, alpha);
}

// ---------------------------------------------------------------------------------------------------------
// Terrain and ocean quads

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) height: f32,
    @location(3) extra: f32,
    // offset to the parent quad's surface (geomorphing)
    @location(4) morph: vec3<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    // camera-relative position (m)
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // position within the quad (m) plus the quad centre modulo the tiling period
    @location(2) tile: vec3<f32>,
    @location(3) height: f32,
    @location(4) extra: f32,
    // model-frame position (m), for directions and latitude/longitude
    @location(5) model: vec3<f32>,
};

@vertex
fn vs_quad(in: VIn) -> VOut {
    return quad_out(in);
}

fn quad_out(in: VIn) -> VOut {
    var out: VOut;
    // Geomorphing: a quad of level L is drawn while the camera is within 2 T of it (its parent split there) and
    // splits itself inside T = radius x 2.5 / 2^L (planet_lod::select); its vertices slide onto the parent's
    // surface as the camera nears 2 T, so neither the switch to the parent nor to the children pops.
    let lvl = chunk.center.w;
    let t_split = frame.solar.w * 2.5 / exp2(lvl);
    let d0 = length(chunk.offset.xyz + in.pos);
    // (offset.w: a newly built quad's fade-in, morphing it in from its parent over QUAD_FADE_MS)
    let m = max(select(smoothstep(1.4 * t_split, 1.9 * t_split, d0), 0.0, lvl < 2.5), chunk.offset.w);
    let rel = chunk.offset.xyz + in.pos + in.morph * m;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.normal = in.normal;
    out.tile = chunk.tile.xyz + in.pos;
    out.height = in.height;
    out.extra = in.extra;
    out.model = chunk.center.xyz + in.pos;
    return out;
}

// The sea's vertices move with the FFT waves near the camera (up and sideways, the original's displacement
// textures); the fragment still samples the waves at the undisplaced tile position.
@vertex
fn vs_ocean(in: VIn) -> VOut {
    var out = quad_out(in);
    if frame.waves.x > 0.5 {
        let dist = length(out.rel);
        let fade = 1.0 - smoothstep(600.0, 1500.0, dist);
        if fade > 0.0 {
            // the mip whose texels match the vertex spacing (33 vertices across a quad)
            let spacing = frame.solar.w * 1.5708 / exp2(chunk.center.w) / 32.0;
            let lod_l = log2(max(spacing / (frame.waves.y / 128.0), 1.0));
            let w = ocean_waves(out.tile, normalize(out.model), lod_l, 8.0);
            let rel = out.rel + w.disp * fade;
            out.rel = rel;
            out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
        }
    }
    return out;
}

struct Wave {
    // surface gradient (model frame), displacement (m), Jacobian, weight of the long and short patches
    grad: vec3<f32>,
    disp: vec3<f32>,
    jac: f32,
    w_long: f32,
    w_short: f32,
};

// The FFT patches on the sphere: projected along the axis closest to `up` (blended where two are close), at
// mip levels chosen by the caller; each patch fades out as its last mip is reached.
fn ocean_waves(p: vec3<f32>, up: vec3<f32>, lod_l: f32, lod_s: f32) -> Wave {
    var out: Wave;
    out.grad = vec3<f32>(0.0);
    out.disp = vec3<f32>(0.0);
    out.jac = 1.0;
    let last = f32(textureNumLevels(waves_tex) - 1u);
    out.w_long = 1.0 - smoothstep(last - 1.0, last, lod_l);
    out.w_short = 1.0 - smoothstep(last - 1.0, last, lod_s);
    var bw = pow(abs(up), vec3<f32>(8.0));
    bw = bw / (bw.x + bw.y + bw.z);
    var jac = 0.0;
    for (var a = 0; a < 3; a = a + 1) {
        let wgt = bw[a];
        if wgt < 0.01 {
            continue;
        }
        var e1 = vec3<f32>(0.0, 1.0, 0.0);
        var e2 = vec3<f32>(0.0, 0.0, 1.0);
        if a == 1 {
            e1 = vec3<f32>(0.0, 0.0, 1.0);
            e2 = vec3<f32>(1.0, 0.0, 0.0);
        } else if a == 2 {
            e1 = vec3<f32>(1.0, 0.0, 0.0);
            e2 = vec3<f32>(0.0, 1.0, 0.0);
        }
        let xy = vec2<f32>(dot(p, e1), dot(p, e2));
        let uv_l = xy / frame.waves.y;
        let uv_s = xy / frame.waves.z;
        let d = textureSampleLevel(waves_tex, wrap_samp, uv_l, 0, min(lod_l, last));
        let sl = textureSampleLevel(waves_tex, wrap_samp, uv_l, 1, min(lod_l, last));
        let ss = textureSampleLevel(waves_tex, wrap_samp, uv_s, 1, min(lod_s, last));
        let g2 = sl.xy * out.w_long + ss.zw * out.w_short;
        out.grad = out.grad + (e1 * g2.x + e2 * g2.y) * wgt;
        out.disp = out.disp + (e1 * d.x + e2 * d.z + up * d.y) * (wgt * out.w_long);
        jac = jac + mix(1.0, d.w, out.w_long) * wgt;
    }
    out.jac = jac;
    return out;
}

// Equirectangular texture coordinates of a model-frame direction: u = azimuth / 2 pi (longitude -180 at u = 0),
// v from the north pole down.
fn equi_uv(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(-d.z, d.x) / (2.0 * PI);
    let v = 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

// Derivatives for equirectangular sampling without a seam where u wraps (computed in uniform control flow).
struct EquiGrad { dx: vec2<f32>, dy: vec2<f32> };

fn equi_grad(uv: vec2<f32>) -> EquiGrad {
    // u runs -0.5..0.5 and wraps where atan2 does; u2 wraps half a turn away (at u = 0), so one of them is
    // continuous across any pixel quad
    let u2 = fract(uv.x);
    var g: EquiGrad;
    g.dx = dpdx(uv);
    g.dy = dpdy(uv);
    let dx2 = dpdx(u2);
    let dy2 = dpdy(u2);
    if abs(dx2) + abs(dy2) < abs(g.dx.x) + abs(g.dy.x) {
        g.dx.x = dx2;
        g.dy.x = dy2;
    }
    return g;
}

// Where a direction falls on the polar caps (azimuthal equidistant around each pole, the same azimuth as
// `equi_uv`), and how much the cap replaces the equirect map there (fully inside 55 % of the cap radius).
struct PoleUv { uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, w: f32 };

fn pole_uv(up: vec3<f32>) -> PoleUv {
    var o: PoleUv;
    let cap = max(surface.poles.z, 0.01);
    let colat = acos(clamp(abs(up.y), 0.0, 1.0));
    let h = vec2<f32>(up.x, -up.z);
    let r = colat / cap;
    let c = 0.5 + 0.5 * h / max(length(h), 1e-6) * min(r, 1.0);
    o.uv = vec2<f32>(c.x * 0.5 + select(0.5, 0.0, up.y >= 0.0), c.y);
    o.dx = dpdx(o.uv);
    o.dy = dpdy(o.uv);
    o.w = 1.0 - smoothstep(0.55, 0.8, r);
    return o;
}

// Cubic B-spline filtering from four bilinear taps (the baked cloud coverage is noisy at texel scale; plain
// bilinear magnification of it shows soft squares from orbit).
fn sample_bicubic(t: texture_2d<f32>, uv: vec2<f32>, g: EquiGrad) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(t, 0));
    let st = uv * size - 0.5;
    let i = floor(st);
    let f = st - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let s0 = w0 + w1;
    let s1 = w2 + w3;
    let o0 = (i - 1.0 + w1 / s0 + 0.5) / size;
    let o1 = (i + 1.0 + w3 / s1 + 0.5) / size;
    let a = textureSampleGrad(t, equi_samp, vec2<f32>(o0.x, o0.y), g.dx, g.dy);
    let b = textureSampleGrad(t, equi_samp, vec2<f32>(o1.x, o0.y), g.dx, g.dy);
    let c = textureSampleGrad(t, equi_samp, vec2<f32>(o0.x, o1.y), g.dx, g.dy);
    let d = textureSampleGrad(t, equi_samp, vec2<f32>(o1.x, o1.y), g.dx, g.dy);
    return mix(mix(a, b, s1.x / (s0.x + s1.x)), mix(c, d, s1.x / (s0.x + s1.x)), s1.y / (s0.y + s1.y));
}

// Per-pixel relief from the terrain pack's tiled height layers (ose_data terrain_detail::Detail::sample): the
// same side (equirect, uv x 2) and top projections blended by latitude, the same biome weights, differentiated
// into a slope (east, north; m per m). Only what the mesh cannot show is returned: each layer's slope at the
// pixel's mip minus its slope at the mip matching twice the mesh's vertex spacing.
fn relief_slope(up: vec3<f32>, eg: EquiGrad, biome: vec4<f32>, dtx: vec2<f32>, dty: vec2<f32>,
                spacing: f32) -> vec2<f32> {
    let r = frame.solar.w;
    let lat = asin(clamp(up.y, -1.0, 1.0));
    let u = fract(atan2(-up.z, up.x) / (2.0 * PI));
    let lon = u * 2.0 * PI - PI;
    let v = lat / PI + 0.5;
    let cl = max(cos(lat), 0.05);
    let sl = sin(lat);
    let co = cos(lon);
    let so = sin(lon);
    let top = vec2<f32>(cos(lat) * co, cos(lat) * so) * 0.5;
    let blend = 1.0 - pow(abs(sl), surface.layer_info.y);
    let m = biome / max(biome.r + biome.g + biome.b + biome.a, 0.001);
    let last = f32(textureNumLevels(slopes_tex) - 1u);
    var se = 0.0;
    var sn = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let pr = surface.layers[k / 2];
        let s = select(pr.x, pr.z, k % 2 == 1);
        let hs = select(pr.y, pr.w, k % 2 == 1);
        let w = m[k % 4] * hs;
        if s <= 0.0 || w < 0.01 {
            continue;
        }
        if blend > 0.01 {
            let c = vec2<f32>(u * 2.0, v) * s;
            let dx = vec2<f32>(eg.dx.x * 2.0, -eg.dx.y) * s;
            let dy = vec2<f32>(eg.dy.x * 2.0, -eg.dy.y) * s;
            let lod = clamp(log2(max(max(length(dx), length(dy)) * 1024.0, 1e-6)), 0.0, last);
            let texel = PI * r / (s * 1024.0);
            let lod_mesh = clamp(log2(max(2.0 * spacing / texel, 1.0)), lod, last);
            let g = textureSampleLevel(slopes_tex, wrap_samp, c, k, lod).xy
                - textureSampleLevel(slopes_tex, wrap_samp, c, k, lod_mesh).xy;
            se = se + w * blend * g.x * s / (PI * r * cl);
            sn = sn + w * blend * g.y * s / (PI * r);
        }
        if blend < 0.99 {
            let c = top * s;
            let lod = clamp(log2(max(max(length(dtx), length(dty)) * s * 1024.0, 1e-6)), 0.0, last);
            let texel = 2.0 * r / (s * 1024.0);
            let lod_mesh = clamp(log2(max(2.0 * spacing / texel, 1.0)), lod, last);
            let g = textureSampleLevel(slopes_tex, wrap_samp, c, k, lod).xy
                - textureSampleLevel(slopes_tex, wrap_samp, c, k, lod_mesh).xy;
            let xe = (g.x * -so + g.y * co) * 0.5 * s / r;
            let xn = (g.x * -sl * co + g.y * -sl * so) * 0.5 * s / r;
            se = se + w * (1.0 - blend) * xe;
            sn = sn + w * (1.0 - blend) * xn;
        }
    }
    return vec2<f32>(se, sn);
}

// Terrain shadows: march from the ground towards the sun over the global height map (the large-scale
// relief: crater rims, ridges, mountains) and darken where it rises above the ray, softened over a small angle.
// Start and samples use the same map, so the ground never shadows itself.
fn global_height(d: vec3<f32>, lod: f32) -> f32 {
    let u = fract(atan2(-d.z, d.x) / (2.0 * PI));
    let v = asin(clamp(d.y, -1.0, 1.0)) / PI + 0.5;
    let h = textureSampleLevel(global_height_tex, equi_samp, vec2<f32>(u, v), lod).r;
    return h * surface.shadow.y + surface.shadow.z;
}

fn terrain_shadow(model: vec3<f32>, sun: vec3<f32>) -> f32 {
    if surface.shadow.x < 0.5 {
        return 1.0;
    }
    let up = normalize(model);
    if dot(up, sun) <= 0.0 {
        return 1.0;
    }
    let r = frame.solar.w;
    let texel = PI * r / f32(textureDimensions(global_height_tex, 0).y);
    let last = f32(textureNumLevels(global_height_tex) - 1u);
    let p = up * (r + global_height(up, 0.0) + 2.0);
    var lit = 1.0;
    var t = texel * 0.5;
    let shadow_steps = i32(frame.quality.x);
    if shadow_steps <= 0 {
        return 1.0;
    }
    for (var i = 0; i < shadow_steps; i = i + 1) {
        let q = p + sun * t;
        let lq = length(q);
        let lod = clamp(log2(max(t * 0.03 / texel, 1.0)), 0.0, last);
        let h = global_height(q / lq, lod);
        // height of the ray above the terrain, as an angle seen from the start
        lit = min(lit, clamp((lq - r - h) / (t * 0.03) + 0.5, 0.0, 1.0));
        if lit <= 0.0 {
            break;
        }
        // the same ~90 km reach with fewer steps on medium detail
        t = t * pow(1.45, 20.0 / f32(shadow_steps));
    }
    return lit;
}

fn sample_equi(t: texture_2d<f32>, uv: vec2<f32>, g: EquiGrad) -> vec4<f32> {
    return textureSampleGrad(t, equi_samp, uv, g.dx, g.dy);
}

// Drifting bands on a gas giant (the original's scaled-material flow, `_FlowmapSpeed1` / `_FlowmapStrength1`,
// whose flow texture is unset): alternating zonal jets by latitude advect the albedo along longitude, two
// phases half a cycle apart cross-faded so the shear never builds up.
fn flowed_albedo(uv: vec2<f32>, g: EquiGrad, y: f32) -> vec3<f32> {
    let lat = asin(clamp(y, -1.0, 1.0));
    // jets: a dozen bands, strongest at the equator, nothing at the poles
    let jet = sin(lat * 12.0 + 0.6) * cos(lat) + 0.35 * cos(lat) * cos(lat);
    // one cycle every 1 / (speed * 0.05) s (about 90 s for Jool), moving a texel band of strength * 0.01 in u
    let t = frame.cam_km.w * surface.clouds2.z * 0.05;
    let shift = jet * surface.clouds2.w * 0.01;
    let p0 = fract(t);
    let p1 = fract(t + 0.5);
    let a = sample_equi(albedo_tex, uv + vec2<f32>(shift * (p0 - 0.5), 0.0), g).rgb;
    let b = sample_equi(albedo_tex, uv + vec2<f32>(shift * (p1 - 0.5), 0.0), g).rgb;
    let w = abs(p0 - 0.5) * 2.0;
    return mix(a, b, w);
}

fn window(x: f32, lo: f32, hi: f32, blend: f32) -> f32 {
    let b = max(blend, 1e-3);
    return smoothstep(lo - b, lo, x) * (1.0 - smoothstep(hi, hi + b, x));
}

// Triplanar weights for a direction (sharpened like the original's _TriplanarContrast).
fn tri_weights(n: vec3<f32>) -> vec3<f32> {
    let w = pow(abs(n), vec3<f32>(4.0));
    return w / (w.x + w.y + w.z);
}

fn hash2(i: vec2<f32>) -> f32 {
    return fract(sin(dot(i, vec2<f32>(127.1, 311.7))) * 43758.547);
}

fn vnoise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash2(i), hash2(i + vec2<f32>(1.0, 0.0)), u.x),
               mix(hash2(i + vec2<f32>(0.0, 1.0)), hash2(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// Anti-tiling (the original's stochastic sampling, `_StochasticScale`): a second copy of the slice, turned and
// shifted, blended in by low-frequency noise so the 8 m and 64 m repeats do not show as rows across the ground.
fn detail_2d(slot: i32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, anti: bool) -> vec4<f32> {
    let a = textureSampleGrad(detail_a, wrap_samp, uv, slot, dx, dy);
    if !anti {
        return a;
    }
    let r = mat2x2<f32>(0.8, -0.6, 0.6, 0.8);
    let b = textureSampleGrad(detail_a, wrap_samp, r * uv + vec2<f32>(0.37, 0.61), slot, r * dx, r * dy);
    let k = smoothstep(0.3, 0.7, vnoise2(uv * 0.23));
    return mix(a, b, k);
}

// The detail normal (x in alpha, y in green) with the same anti-tiling copy as `detail_2d`; the turned copy's
// tangent-space vector is turned back.
fn detail_n2(slot: i32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec2<f32> {
    let a = textureSampleGrad(detail_n, wrap_samp, uv, slot, dx, dy);
    if frame.quality.z < 0.5 {
        return vec2<f32>(a.a, a.g) * 2.0 - 1.0;
    }
    let r = mat2x2<f32>(0.8, -0.6, 0.6, 0.8);
    let b = textureSampleGrad(detail_n, wrap_samp, r * uv + vec2<f32>(0.37, 0.61), slot, r * dx, r * dy);
    let na = vec2<f32>(a.a, a.g) * 2.0 - 1.0;
    let nb = transpose(r) * (vec2<f32>(b.a, b.g) * 2.0 - 1.0);
    return mix(na, nb, smoothstep(0.3, 0.7, vnoise2(uv * 0.23)));
}

fn tri_sample(slot: i32, p: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, w: vec3<f32>, period: f32) -> vec4<f32> {
    let q = p / period;
    let qx = dpx / period;
    let qy = dpy / period;
    // (not on low detail, which keeps the plain repeat)
    let anti = period < 100.0 && frame.quality.z > 0.5;
    let a = detail_2d(slot, q.yz, qx.yz, qy.yz, anti);
    let b = detail_2d(slot, q.zx, qx.zx, qy.zx, anti);
    let c = detail_2d(slot, q.xy, qx.xy, qy.xy, anti);
    return a * w.x + b * w.y + c * w.z;
}

// Detail normal (the normSAO slices: x in alpha, y in green) projected like `tri_sample`, as a perturbation
// of the surface normal in the model frame.
fn tri_normal(slot: i32, p: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, w: vec3<f32>, period: f32) -> vec3<f32> {
    let q = p / period;
    let qx = dpx / period;
    let qy = dpy / period;
    let na = detail_n2(slot, q.yz, qx.yz, qy.yz);
    let nb = detail_n2(slot, q.zx, qx.zx, qy.zx);
    let nc = detail_n2(slot, q.xy, qx.xy, qy.xy);
    return vec3<f32>(0.0, na.x, na.y) * w.x + vec3<f32>(nb.y, 0.0, nb.x) * w.y + vec3<f32>(nc.x, nc.y, 0.0) * w.z;
}

fn slot_mean(slot: i32) -> vec3<f32> {
    let levels = f32(textureNumLevels(detail_a));
    return textureSampleLevel(detail_a, wrap_samp, vec2<f32>(0.5), slot, levels - 1.0).rgb;
}

@fragment
fn fs_terrain(in: VOut) -> @location(0) vec4<f32> {
    let up = normalize(in.model);
    var n = normalize(in.normal);
    let uv = equi_uv(up);
    let eg = equi_grad(uv);
    let dpx = dpdx(in.tile);
    let dpy = dpdy(in.tile);
    let dist = length(in.rel);
    let pu = pole_uv(up);
    // projections of the terrain layers (relief below), differentiated here in uniform control flow
    let top_c = vec2<f32>(-up.x, up.z) * 0.5;
    let dtx = dpdx(top_c);
    let dty = dpdy(top_c);
    var albedo = sample_equi(albedo_tex, uv, eg).rgb;
    if surface.clouds2.z > 0.0 {
        albedo = flowed_albedo(uv, eg, up.y);
    }
    if surface.poles.x > 0.5 && pu.w > 0.0 {
        albedo = mix(albedo, textureSampleGrad(pole_albedo_tex, equi_samp, pu.uv, pu.dx, pu.dy).rgb, pu.w);
    }

    // The scaled normal map (far): perturb in the east/north/up frame.
    if surface.flags.y > 0.5 {
        let f = surface.normal_fade;
        let k = mix(f.z, f.w, smoothstep(f.x, f.y, dist));
        if k > 0.001 {
            let nm = sample_equi(normal_tex, uv, eg);
            let tx = nm.a * 2.0 - 1.0;
            let ty = nm.g * 2.0 - 1.0;
            let east = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), up) + vec3<f32>(1e-6, 0.0, 0.0));
            let north = cross(up, east);
            n = normalize(n + (east * tx + north * ty) * k);
        }
    }

    // Biome detail (near): the biome mask's four channels each name four slices; their height and slope rules
    // weight them, and the result modulates the scaled albedo so the large-scale colour stays the original's.
    var dbg = vec3<f32>(0.0);
    if surface.flags.z > 0.5 && surface.flags.z < 1.5 {
        return vec4<f32>(sample_equi(biome_tex, uv, eg).rgb, 1.0);
    }
    if surface.flags.x > 0.5 {
        let f = surface.albedo_fade;
        let scaled = mix(f.z, f.w, smoothstep(f.x, f.y, dist));
        let k = 1.0 - scaled;
        if k > 0.01 {
            var biome = sample_equi(biome_tex, uv, eg);
            // Much of the land has no biome weight at all (Kerbin around the KSC); there the first biome is the
            // base, as the mask's R channel is on most bodies.
            let bsum = biome.r + biome.g + biome.b + biome.a;
            if bsum < 0.05 {
                biome = biome + vec4<f32>(1.0 - bsum / 0.05, 0.0, 0.0, 0.0);
            }
            let slope = degrees(acos(clamp(dot(normalize(in.normal), up), -1.0, 1.0)));
            var best = array<f32, 16>();
            for (var c = 0; c < 4; c = c + 1) {
                let bw = biome[c];
                if bw < 0.02 {
                    continue;
                }
                for (var l = 0; l < 4; l = l + 1) {
                    var w = 1.0;
                    if surface.height_enable[c][l] > 0.5 {
                        let hp = surface.height_params[c * 4 + l];
                        w = w * window(in.height + surface.flags.w, hp.x, hp.y, hp.z);
                    }
                    if surface.slope_enable[c][l] > 0.5 {
                        let sp = surface.slope_params[c * 4 + l];
                        w = w * window(slope, sp.x, sp.y, sp.z);
                    }
                    let s = i32(surface.slots[c][l]);
                    if s >= 0 && s < 16 {
                        best[s] = best[s] + w * bw;
                    }
                }
            }
            // The two strongest slices.
            var s0 = -1;
            var s1 = -1;
            var w0 = 0.0;
            var w1 = 0.0;
            for (var s = 0; s < 16; s = s + 1) {
                let w = best[s];
                if w > w0 {
                    s1 = s0;
                    w1 = w0;
                    s0 = s;
                    w0 = w;
                } else if w > w1 {
                    s1 = s;
                    w1 = w;
                }
            }
            let slices = i32(surface.clouds.w);
            if s0 >= 0 && s0 < slices {
                let tw = tri_weights(up);
                var ratio = vec3<f32>(0.0);
                var bump = vec3<f32>(0.0);
                var total = 0.0;
                for (var i = 0; i < 2; i = i + 1) {
                    var s = s0;
                    var w = w0;
                    if i == 1 {
                        s = s1;
                        w = w1;
                    }
                    if s < 0 || s >= slices || w <= 0.0 {
                        continue;
                    }
                    // Three tilings, like the original's distance resampling (8 m, 64 m, 512 m), so the ground
                    // keeps texture from the pad out to tens of kilometres.
                    let near = tri_sample(s, in.tile, dpx, dpy, tw, 8.0);
                    let mid = tri_sample(s, in.tile, dpx, dpy, tw, 64.0);
                    let far = tri_sample(s, in.tile, dpx, dpy, tw, 512.0);
                    let mean = max(slot_mean(s), vec3<f32>(0.02));
                    let c0 = mix(near.rgb, mid.rgb, smoothstep(30.0, 400.0, dist));
                    let c1 = mix(c0, far.rgb, smoothstep(400.0, 4000.0, dist));
                    ratio = ratio + w * c1 / mean;
                    // normals resampled with distance like the albedo (8 m near, 64 m beyond ~30 m), so the
                    // 8 m pattern does not repeat in rows across the ground
                    if dist < 2000.0 {
                        let nmix = smoothstep(30.0, 250.0, dist);
                        var nb = vec3<f32>(0.0);
                        if nmix < 0.99 {
                            nb = tri_normal(s, in.tile, dpx, dpy, tw, 8.0) * (1.0 - nmix);
                        }
                        if nmix > 0.01 {
                            nb = nb + tri_normal(s, in.tile, dpx, dpy, tw, 64.0) * nmix;
                        }
                        bump = bump + w * nb;
                    }
                    total = total + w;
                }
                if total > 0.0 {
                    let r = clamp(ratio / total, vec3<f32>(0.0), vec3<f32>(3.0));
                    dbg = r * 0.5;
                    albedo = albedo * mix(vec3<f32>(1.0), r, k);
                    let nk = k * (1.0 - smoothstep(600.0, 2000.0, dist));
                    n = normalize(n + bump / total * nk);
                }
            }
        }
    }

    if surface.flags.z > 1.5 && surface.flags.z < 2.5 {
        return vec4<f32>(dbg, 1.0);
    }
    if surface.flags.z > 2.5 && surface.flags.z < 3.5 {
        return vec4<f32>(surface.flags.x, surface.clouds.w / 16.0, f32(surface.slots[0].x) / 16.0, 1.0);
    }
    let p_km = frame.cam_km.xyz + in.rel * 0.001;
    // the tiled layers' relief finer than the mesh shows
    if surface.layer_info.x > 0.5 && frame.quality.z > 0.5 {
        let spacing = frame.solar.w * 1.5708 / exp2(chunk.center.w) / 32.0;
        let mask_uv = vec2<f32>(fract(uv.x), 1.0 - uv.y);
        let mask = textureSampleGrad(layer_mask_tex, equi_samp, mask_uv, eg.dx * vec2<f32>(1.0, -1.0),
            eg.dy * vec2<f32>(1.0, -1.0));
        let sl = relief_slope(up, eg, mask, dtx, dty, spacing);
        let east = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), up) + vec3<f32>(1e-6, 0.0, 0.0));
        let north = cross(up, east);
        n = normalize(n - east * sl.x - north * sl.y);
        if surface.flags.z > 6.5 && surface.flags.z < 7.5 {
            return vec4<f32>(abs(sl.x) * 4.0, abs(sl.y) * 4.0, mask.r * 0.5, 1.0);
        }
    }
    if surface.flags.z > 6.5 && surface.flags.z < 7.5 {
        return vec4<f32>(0.0, 0.0, 1.0, 1.0);
    }
    var light = sun_and_sky(p_km, n, frame.sun.xyz);
    light.sun = light.sun * terrain_shadow(in.model, frame.sun.xyz);
    let shade = cloud_shadow(in.model, frame.sun.xyz);
    // Caustics: the sea floor under shallow water catches the waves' focused sunlight (the original's
    // _CausticsTexture, two drifting copies, the brighter where they meet), fading with depth.
    if surface.caustics.x > 0.0 && in.height < 0.0 && dist < 3000.0 {
        let depth = -in.height;
        let tiling = max(surface.caustics.z, 0.5);
        let t = frame.cam_km.w * 0.05 * surface.caustics.w;
        let q = vec2<f32>(in.tile.x + in.tile.z * 0.31, in.tile.y + in.tile.z * 0.73) / tiling;
        let c1 = textureSampleLevel(foam_tex, wrap_samp, q + vec2<f32>(t, t * 0.6), 0.0).g;
        let c2 = textureSampleLevel(foam_tex, wrap_samp, q * 1.13 - vec2<f32>(t * 0.7, t * 0.2), 0.0).g;
        let fade = (1.0 - smoothstep(0.0, max(surface.caustics.y, 1.0), depth))
            * (1.0 - smoothstep(1500.0, 3000.0, dist));
        albedo = albedo * (1.0 + min(c1, c2) * surface.caustics.x * 12.0 * fade * shade);
    }
    if surface.flags.z > 3.5 && surface.flags.z < 4.5 {
        return vec4<f32>(albedo, 1.0);
    }
    if surface.flags.z > 4.5 && surface.flags.z < 5.5 {
        return vec4<f32>(light.sun * 0.25, 1.0);
    }
    if surface.flags.z > 5.5 && surface.flags.z < 6.5 {
        return vec4<f32>(n * 0.5 + 0.5, 1.0);
    }
    var radiance = albedo / PI * (light.sun * shade + light.sky) + albedo * ambient(up, frame.sun.xyz);
    if under_water() {
        radiance = water_fog(radiance * exp(-max(-in.height, 0.0) / 80.0), dist);
    } else {
        radiance = through_air(radiance, p_km);
    }
    return vec4<f32>(tonemap(radiance), 1.0);
}

// Waves: plane waves in 3D tile coordinates, periodic in TILE_PERIOD, projected onto the surface.
fn wave_normal(p: vec3<f32>, up: vec3<f32>, t: f32, dist: f32) -> vec3<f32> {
    var g = vec3<f32>(0.0);
    // twelve directions from long swell (140 m) to ripples (4 m), integer multiples of 2 pi / 512 m
    var ks = array<vec3<f32>, 12>(
        vec3<f32>(3.0, 1.0, 2.0), vec3<f32>(-2.0, 3.0, 4.0), vec3<f32>(7.0, -4.0, 5.0),
        vec3<f32>(-11.0, 6.0, -9.0), vec3<f32>(5.0, 13.0, -8.0), vec3<f32>(23.0, -14.0, 17.0),
        vec3<f32>(-19.0, -21.0, 11.0), vec3<f32>(31.0, 9.0, -27.0), vec3<f32>(-44.0, 37.0, 20.0),
        vec3<f32>(58.0, -23.0, -49.0), vec3<f32>(-71.0, -66.0, 35.0), vec3<f32>(97.0, 54.0, 81.0));
    for (var i = 0; i < 12; i = i + 1) {
        let k = ks[i] * (16.0 * PI / TILE_PERIOD);
        let wl = 2.0 * PI / length(k);
        let amp = 0.007 * wl * exp(-dist / (wl * 40.0));
        let omega = sqrt(9.81 * length(k));
        g = g + k * amp * cos(dot(k, p) - omega * t);
    }
    g = g - up * dot(g, up);
    return normalize(up - g);
}

@fragment
fn fs_ocean(in: VOut) -> @location(0) vec4<f32> {
    let depth = -in.extra;
    if depth <= -2.0 {
        discard;
    }
    let up = normalize(in.model);
    let dist = length(in.rel);
    let view = normalize(in.rel);
    var n = wave_normal(in.tile, up, frame.cam_km.w, dist);
    // FFT waves near the camera (the analytic swell stays beyond), mips by the pixel footprint on the sea
    var crest = 0.0;
    if frame.waves.x > 0.5 {
        let foot = dist * 0.0012 / max(abs(dot(view, up)), 0.12);
        let lod_l = log2(max(foot / (frame.waves.y / 128.0), 1.0));
        let lod_s = log2(max(foot / (frame.waves.z / 128.0), 1.0));
        let w = ocean_waves(in.tile, up, lod_l, lod_s);
        let g = w.grad - up * dot(w.grad, up);
        let nf = normalize(up - g);
        n = normalize(mix(n, nf, max(w.w_long, w.w_short)));
        // breaking crests where the choppy displacement folds the surface (Jacobian near or below 0)
        crest = (1.0 - smoothstep(0.35, 0.9, w.jac)) * w.w_long;
    }
    if under_water() {
        // The surface from below: Snell's window (bright, the sky through the waves) inside 48.6 degrees of
        // straight up, the water's own colour reflected outside it.
        let window = smoothstep(0.6, 0.7, dot(view, normalize(mix(up, n, 0.3))));
        let sky = frame.solar.xyz * 0.25 * (0.6 + 0.4 * dot(frame.sun.xyz, up));
        let below = water_light(0.0) / PI * frame.solar.xyz;
        var c = mix(below, sky, window);
        c = water_fog(c, dist);
        return vec4<f32>(tonemap(c), 1.0);
    }
    let p_km = frame.cam_km.xyz + in.rel * 0.001;
    let sun = frame.sun.xyz;
    var light = sun_and_sky(p_km, up, sun);
    light.sun = light.sun * cloud_shadow(in.model, sun);
    let max_depth = max(surface.deep.w, 1.0);
    let body_col = mix(surface.shallow.rgb, surface.deep.rgb, smoothstep(0.0, 1.0, depth / max_depth));
    var radiance = body_col / PI * (light.sun + light.sky) * 0.6;
    // Shoreline foam (the original's _ShorelineFoamMaxDepth): the foam texture, drifting, where the sea is
    // shallow, faded out with distance.
    if surface.foam.z > 0.5 && dist < 4000.0 {
        let tiling = max(surface.foam.y, 1.0);
        let t = frame.cam_km.w;
        let fuv = vec2<f32>(dot(in.tile, vec3<f32>(1.0, 0.0, 0.0)) + dot(in.tile, vec3<f32>(0.0, 0.0, 1.0)) * 0.3,
                            dot(in.tile, vec3<f32>(0.0, 1.0, 0.0)) + dot(in.tile, vec3<f32>(0.0, 0.0, 1.0)) * 0.7) / tiling;
        let f1 = textureSampleLevel(foam_tex, wrap_samp, fuv + vec2<f32>(t * 0.01, 0.0), 0.0).r;
        let f2 = textureSampleLevel(foam_tex, wrap_samp, fuv * 0.37 - vec2<f32>(0.0, t * 0.006), 0.0).r;
        let shallow = 1.0 - smoothstep(0.0, max(surface.foam.x, 0.5), depth);
        let foam = clamp((f1 * f2 * 2.0) * shallow * (1.0 - smoothstep(1500.0, 4000.0, dist)), 0.0, 1.0);
        radiance = mix(radiance, vec3<f32>(0.85) / PI * (light.sun + light.sky), foam);
    }
    if crest > 0.0 {
        radiance = mix(radiance, vec3<f32>(0.8) / PI * (light.sun + light.sky), crest * 0.8);
    }
    // Fresnel reflection of the sky (rays that would point into the sea are bent back to the horizon).
    let cos_v = max(dot(-view, n), 0.0);
    let fres = 0.02 + 0.98 * pow(1.0 - cos_v, 5.0);
    var refl = reflect(view, n);
    if dot(refl, up) < 0.02 {
        refl = normalize(refl + up * (0.02 - dot(refl, up)));
    }
    var sky = vec3<f32>(0.0);
    if frame.rayleigh.w > 0.5 {
        sky = sky_radiance(p_km, refl, sun).radiance;
    }
    radiance = mix(radiance, sky, fres * (1.0 - crest * 0.8));
    // Sun glint.
    let h = normalize(sun - view);
    let smooth_ = surface.shallow.w;
    let shin = mix(64.0, 2048.0, smooth_ * smooth_);
    let spec = pow(max(dot(n, h), 0.0), shin) * (shin + 8.0) / (8.0 * PI);
    radiance = radiance + light.sun * spec * fres * 4.0;
    // From high up the sea becomes the scaled albedo, as the original's _ScaleAlbedoStart/EndHeight (66 to
    // 160 km): Kerbin's deep navy oceans from orbit, the glint fading with them.
    let to_scaled = smoothstep(66000.0, 160000.0, dist);
    let suv = equi_uv(up);
    let scaled = sample_equi(albedo_tex, suv, equi_grad(suv)).rgb;
    radiance = mix(radiance, scaled / PI * (light.sun + light.sky), to_scaled);
    radiance = through_air(radiance, p_km);
    let alpha = mix(mix(0.55, 1.0, smoothstep(0.0, max(surface.foam.w, 0.5), depth)), 1.0, to_scaled)
        * smoothstep(-2.0, 0.5, depth);
    return vec4<f32>(tonemap(radiance), alpha);
}

// Value noise on a lattice that wraps every `cells` cells, so quads that share an edge agree (tile coordinates
// are each quad's centre modulo TILE_PERIOD plus the offset within the quad).
fn hash3(c: vec3<i32>) -> f32 {
    var h = (u32(c.x) * 73856093u) ^ (u32(c.y) * 19349663u) ^ (u32(c.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffu) / 65535.0;
}

fn vnoise(p: vec3<f32>, cells: i32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let w = vec3<i32>(cells);
    var acc = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let c = ((i + o) % w + w) % w;
        let wt = mix(1.0 - u.x, u.x, f32(o.x)) * mix(1.0 - u.y, u.y, f32(o.y)) * mix(1.0 - u.z, u.z, f32(o.z));
        acc = acc + hash3(c) * wt;
    }
    return acc;
}

@fragment
fn fs_clouds(in: VOut) -> @location(0) vec4<f32> {
    let up = normalize(in.model);
    let uv = equi_uv(up);
    let eg = equi_grad(uv);
    let pu = pole_uv(up);
    var c = sample_bicubic(clouds_tex, uv, eg);
    if surface.poles.y > 0.5 && pu.w > 0.0 {
        c.a = mix(c.a, textureSampleGrad(pole_albedo_tex, equi_samp, pu.uv, pu.dx, pu.dy).a, pu.w);
    }
    let base = clamp(c.a * surface.clouds.y, 0.0, 1.0);
    if base < 0.01 {
        discard;
    }
    let dist = length(in.rel);
    // Break the baked coverage (1.8 km texels on Kerbin) into puffs up close: noise at 512, 256 and 128 m that
    // erodes the edges, faded out with distance where the texture alone reads right.
    let near = 1.0 - smoothstep(8000.0, 40000.0, dist);
    let drift = vec3<f32>(frame.cam_km.w * 2.0, 0.0, frame.cam_km.w * 1.3);
    let q = in.tile + drift;
    let n = vnoise(q / 512.0, 8) * 0.55 + vnoise(q / 256.0, 16) * 0.3 + vnoise(q / 128.0, 32) * 0.15;
    // Farther out (and from orbit) the baked coverage alone is a soft blanket: sharpen it into cumulus fields
    // with gaps, as the original's volume clouds read from orbit, broken by noise of 1.5 and 0.75 km on the
    // sphere (not tiled), faded out once a pixel covers that much.
    let pf = up * (frame.solar.w / 1500.0);
    let nf = vnoise(pf, 1048576) * 0.6 + vnoise(pf * 2.0, 1048576) * 0.4;
    let kf = 1.0 - smoothstep(300.0, 1200.0, dist * 0.0012);
    let far_cover = smoothstep(0.24, 0.74, base + (nf - 0.5) * 0.7 * kf);
    let cover = clamp(mix(far_cover, smoothstep(0.35, 0.75, base + (n - 0.5) * 0.9), near), 0.0, 1.0);
    let p_km = frame.cam_km.xyz + in.rel * 0.001;
    let sun = frame.sun.xyz;
    // Thin layer lit from either side: wrap the sun term so the edge of the day side is soft; seen from below
    // the thick middle is in its own shadow.
    let light = sun_and_sky(p_km, up, sun);
    let wrap = smoothstep(-0.15, 0.3, dot(up, sun));
    let cam_alt = length(frame.cam_km.xyz) * 1000.0 - frame.solar.w;
    let below = select(0.0, 1.0, cam_alt < surface.clouds.x);
    let self_shadow = mix(1.0, mix(1.0, 0.45, cover), below);
    let lit = light.sun / max(dot(up, sun), 0.05) * wrap * 0.8 * self_shadow + light.sky * 2.0;
    // the layer's colour map (Eve's purple and pink swirls), white elsewhere
    // thin edges are greyer than the thick, sunlit tops, which gives the sharpened fields some body
    let cloud_albedo = min(c.rgb * surface.cloud_col.x, vec3<f32>(1.0)) * 0.9 * (0.72 + 0.28 * cover);
    var radiance = cloud_albedo / PI * lit;
    radiance = through_air(radiance, p_km);
    // Fade the shell when the camera is close to its altitude (it would read as a flat sheet); with the volume
    // on, the shell only draws beyond the volume's reach.
    var fade = smoothstep(300.0, 2500.0, abs(cam_alt - surface.clouds.x));
    if frame.water.z > 0.5 {
        fade = smoothstep(VCLOUD_RANGE * 0.8, VCLOUD_RANGE, dist);
    }
    return vec4<f32>(tonemap(radiance), cover * fade);
}

// ---------------------------------------------------------------------------------------------------------
// Grass: two crossed textured cards per patch (Vegetation Studio's grass patches), alpha-tested, swaying.
// The chunk uniform carries the placement origin minus the camera (offset), the dry tint (center) and the
// healthy tint (tile).

@group(3) @binding(0) var grass_tex: texture_2d_array<f32>;
@group(3) @binding(1) var grass_samp: sampler;

struct GIn {
    @location(0) pos: vec3<f32>,
    @location(1) up: vec3<f32>,
    // scale, yaw, texture layer, tint (0 dry .. 1 healthy)
    @location(2) params: vec4<f32>,
};

struct GOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) up: vec3<f32>,
    @location(3) params: vec4<f32>,
};

@vertex
fn vs_grass(@builtin(vertex_index) vi: u32, g: GIn) -> GOut {
    var out: GOut;
    let card = f32(vi / 6u);
    var corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
                                      vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let c = corners[vi % 6u];
    let up = normalize(g.up);
    var side = cross(up, vec3<f32>(0.0, 1.0, 0.0));
    if dot(side, side) < 1e-6 {
        side = cross(up, vec3<f32>(1.0, 0.0, 0.0));
    }
    side = normalize(side);
    let fwd = cross(up, side);
    let a = g.params.y + card * 1.5707963;
    let dir = side * cos(a) + fwd * sin(a);
    let s = g.params.x;
    let w = 1.1 * s;
    let h = 0.7 * s;
    let t = frame.cam_km.w;
    let sway = sin(t * 1.7 + g.pos.x * 0.35 + g.pos.z * 0.27) * 0.08 * h * c.y;
    let p = g.pos + dir * ((c.x - 0.5) * w + sway) + up * (c.y * h);
    let rel = chunk.offset.xyz + p;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    out.up = up;
    out.params = g.params;
    return out;
}

@fragment
fn fs_grass(in: GOut) -> @location(0) vec4<f32> {
    let dist = length(in.rel);
    let tex = textureSample(grass_tex, grass_samp, in.uv, i32(in.params.z + 0.5));
    // thin out towards the edge of the patch field (dithered) so there is no hard line
    let fade = 1.0 - smoothstep(65.0, 88.0, dist);
    let dither = fract(sin(dot(in.clip.xy, vec2<f32>(12.9898, 78.233))) * 43758.547);
    if tex.a < 0.45 || dither > fade {
        discard;
    }
    let tint = mix(chunk.center.rgb, chunk.tile.rgb, in.params.w);
    let albedo = tex.rgb * tint;
    let p_km = frame.cam_km.xyz + in.rel * 0.001;
    let model = normalize(p_km);
    let light = sun_and_sky(p_km, in.up, frame.sun.xyz);
    let shade = cloud_shadow(model * frame.solar.w, frame.sun.xyz);
    // cards are thin: let some light through from behind
    var radiance = albedo / PI * (light.sun * shade * 0.85 + light.sky) + albedo * ambient(in.up, frame.sun.xyz);
    radiance = through_air(radiance, p_km);
    return vec4<f32>(tonemap(radiance), 1.0);
}

// ---------------------------------------------------------------------------------------------------------
// Far props as the original's Amplify octahedral impostors: a camera-facing quad showing the atlas frame
// whose view direction is closest to the camera's (frames x frames views over an octahedron, top view in the
// middle). Shares the grass layout: group 3 holds the atlases as a texture array.

struct FIn {
    @location(0) pos: vec3<f32>,
    // size (m), centre height (m), atlas layer, frames
    @location(1) params: vec4<f32>,
    // colour scale matching the mesh
    @location(2) colour: vec3<f32>,
};

struct FOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) up: vec3<f32>,
    @location(3) @interpolate(flat) layer: i32,
    @location(4) facing: vec3<f32>,
    @location(5) colour: vec3<f32>,
};

fn octa_encode(n_in: vec3<f32>) -> vec2<f32> {
    var n = n_in / (abs(n_in.x) + abs(n_in.y) + abs(n_in.z));
    var xz = n.xz;
    if n.y < 0.0 {
        xz = (1.0 - abs(n.zx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xz >= vec2<f32>(0.0));
    }
    return xz;
}

@vertex
fn vs_far(@builtin(vertex_index) vi: u32, f: FIn) -> FOut {
    var out: FOut;
    var corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
                                      vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let c = corners[vi % 6u];
    let base = chunk.offset.xyz + f.pos;
    let up = normalize(frame.cam_km.xyz * 1000.0 + base);
    let centre = base + up * f.params.y;
    let to_cam = normalize(-centre);
    var right = cross(up, to_cam);
    if dot(right, right) < 1e-8 {
        right = cross(vec3<f32>(1.0, 0.0, 0.0), to_cam);
    }
    right = normalize(right);
    let bup = cross(to_cam, right);
    let size = f.params.x;
    let rel = centre + right * (c.x - 0.5) * size + bup * (c.y - 0.5) * size;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.up = up;
    out.facing = to_cam;
    // the camera's direction in the prop's own frame (up = Y, an east/north pair for X and Z)
    var east = cross(vec3<f32>(0.0, 1.0, 0.0), up);
    if dot(east, east) < 1e-8 {
        east = vec3<f32>(1.0, 0.0, 0.0);
    }
    east = normalize(east);
    let north = cross(up, east);
    let d = vec3<f32>(dot(to_cam, east), dot(to_cam, up), dot(to_cam, north));
    let frames = f.params.w;
    let o = octa_encode(d) * 0.5 + 0.5;
    let frame_xy = floor(o * (frames - 1.0) + 0.5);
    out.uv = (frame_xy + vec2<f32>(c.x, 1.0 - c.y)) / frames;
    out.uv.y = 1.0 - out.uv.y;
    out.layer = i32(f.params.z + 0.5);
    out.colour = f.colour;
    return out;
}

@fragment
fn fs_far(in: FOut) -> @location(0) vec4<f32> {
    let tex = textureSampleLevel(grass_tex, grass_samp, in.uv, in.layer, 0.0);
    if tex.a < 0.5 {
        discard;
    }
    let dist = length(in.rel);
    // fade in past the meshes and out at the far edge (dithered)
    let dither = fract(sin(dot(in.clip.xy, vec2<f32>(12.9898, 78.233))) * 43758.547);
    if dither > 1.0 - smoothstep(2000.0, 2500.0, dist) {
        discard;
    }
    let n = normalize(in.up * 0.6 + in.facing * 0.4);
    let p_km = frame.cam_km.xyz + in.rel * 0.001;
    let sun = frame.sun.xyz;
    let shade = cloud_shadow(normalize(p_km) * frame.solar.w, sun);
    let albedo = min(tex.rgb * in.colour, vec3<f32>(1.0));
    // Lit as the renderer lights the props' meshes (ambient 0.3, the rest by the sun, no exposure), so a prop
    // keeps its brightness when it turns into its impostor; on a dark body the planet's exposure would blow
    // bright rocks out to white. That colour is taken back through the tone map so the air can veil it.
    let diffuse = max(dot(n, sun), 0.0) * eclipse(p_km, sun) * shade;
    let lit = min(albedo * (0.3 + 0.7 * diffuse * frame.quality.w), vec3<f32>(0.95));
    var radiance = -log(vec3<f32>(1.0) - lit) / max(frame.sun.w, 0.1);
    radiance = through_air(radiance, p_km);
    return vec4<f32>(tonemap(radiance), 1.0);
}

// ---------------------------------------------------------------------------------------------------------
// Volumetric clouds (the original's VolumeCloudConfiguration cumulus layer, simplified): the baked coverage
// map sets where clouds are, a height profile rounds their bases and tops, value noise erodes them into
// puffs, and the sun lights them through a short march towards it. Full-screen, within VCLOUD_RANGE metres;
// the shell takes over beyond.

const VCLOUD_RANGE: f32 = 24000.0;

// Underwater light shafts (the original's ocean god rays, `OceanFeatures.GodRayQuality`): march the water in
// front of the camera; each sample is lit by the sunlight the waves focused where its refracted sun ray crossed
// the surface (the caustics texture, softened), dimmed by the water above it and between it and the camera.
@fragment
fn fs_shafts(in: SkyOut) -> VcOut {
    var out: VcOut;
    out.colour = vec4<f32>(0.0);
    out.depth = 0.0;
    let up = normalize(frame.cam_km.xyz);
    let sun = frame.sun.xyz;
    let sun_up = dot(sun, up);
    if !under_water() || surface.caustics.x <= 0.0 || sun_up < 0.05 {
        discard;
    }
    let p4 = frame.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let view = normalize(p4.xyz / p4.w);
    // the sunlight's direction of travel in the water
    let sd = refract(-sun, up, 1.0 / 1.33);
    let sd_up = min(dot(sd, up), -0.05);
    let cam_depth = -frame.water.x;
    let tiling = max(surface.caustics.z, 0.5) * 3.0;
    let t_anim = frame.cam_km.w * 0.05 * surface.caustics.w;
    let jitter = fract(sin(dot(in.ndc, vec2<f32>(12.9898, 78.233))) * 43758.547);
    let steps = 32;
    let dt = 1.5;
    var acc = 0.0;
    for (var i = 0; i < steps; i = i + 1) {
        let t = (f32(i) + jitter) * dt;
        let p = view * t;
        let depth = cam_depth - dot(p, up);
        if depth < 0.0 {
            break;
        }
        // where this sample's sunlight came through the surface
        let q = frame.cam_tile.xyz + p - sd * (depth / -sd_up);
        let uv = vec2<f32>(q.x + q.z * 0.31, q.y + q.z * 0.73) / tiling;
        let c1 = textureSampleLevel(foam_tex, wrap_samp, uv + vec2<f32>(t_anim, t_anim * 0.6), 2.5).g;
        let c2 = textureSampleLevel(foam_tex, wrap_samp, uv * 1.13 - vec2<f32>(t_anim * 0.7, t_anim * 0.2), 2.5).g;
        let focus = max(min(c1, c2) - 0.08, 0.0);
        acc = acc + focus * exp(-depth / 25.0) * exp(-t / 20.0) * dt;
    }
    // looking towards the sun the shafts are brightest (forward scattering)
    let phase = 0.4 + 1.6 * pow(max(dot(view, sd * -1.0), 0.0), 4.0);
    let rad = frame.water_col.rgb * frame.solar.xyz * sun_up * acc * phase * surface.caustics.x * 0.6;
    out.colour = vec4<f32>(tonemap(rad), 0.0);
    // in front of anything further than a few metres
    let clip = frame.view_proj * vec4<f32>(view * 4.0, 1.0);
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    return out;
}

struct VcOut {
    @location(0) colour: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn vc_noise(p: vec3<f32>) -> f32 {
    // big lattices so the pattern does not repeat over a cloud field
    return vnoise(p / 900.0, 1 << 20) * 0.6 + vnoise(p / 330.0, 1 << 20) * 0.28 + vnoise(p / 120.0, 1 << 20) * 0.12;
}

fn vc_density(p: vec3<f32>, base_r: f32, top_r: f32, t: f32, detail: bool) -> f32 {
    let r = length(p);
    let hfrac = (r - base_r) / (top_r - base_r);
    if hfrac < 0.0 || hfrac > 1.0 {
        return 0.0;
    }
    let up = p / r;
    // wind: the coverage drifts east
    let drift = vec3<f32>(-up.z, 0.0, up.x) * (t * surface.clouds2.y);
    let uv = equi_uv(normalize(p + drift));
    let cover = textureSampleLevel(clouds_tex, equi_samp, uv, 2.0).a * surface.clouds.y;
    if cover < 0.04 {
        return 0.0;
    }
    // flat bases, rounded tops; thicker cover reaches higher
    let profile = smoothstep(0.0, 0.08, hfrac) * (1.0 - smoothstep(cover * 0.9, cover, hfrac));
    var d = cover * profile;
    if detail && d > 0.0 {
        let n = vc_noise(p + drift * 1.5);
        d = clamp((d - (1.0 - n) * 0.65) * 2.2, 0.0, 1.0);
    }
    return d;
}

@fragment
fn fs_vclouds(in: SkyOut) -> VcOut {
    var out: VcOut;
    out.colour = vec4<f32>(0.0);
    out.depth = 0.0;
    if frame.water.z < 0.5 || surface.clouds.z < 0.5 {
        discard;
    }
    let p4 = frame.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let view = normalize(p4.xyz / p4.w);
    let cam = frame.cam_km.xyz * 1000.0;
    let radius = frame.solar.w;
    let base_r = radius + surface.clouds.x;
    let top_r = radius + max(surface.clouds2.x, surface.clouds.x + 500.0);
    // the stretch of the ray inside the shell between the base and top spheres
    let b = dot(cam, view);
    let c_top = dot(cam, cam) - top_r * top_r;
    let disc_top = b * b - c_top;
    if disc_top <= 0.0 {
        discard;
    }
    let sq_top = sqrt(disc_top);
    var t0 = max(-b - sq_top, 0.0);
    var t1 = -b + sq_top;
    let c_base = dot(cam, cam) - base_r * base_r;
    let disc_base = b * b - c_base;
    let cam_r = length(cam);
    if disc_base > 0.0 {
        let sq_b = sqrt(disc_base);
        let tb0 = -b - sq_b;
        let tb1 = -b + sq_b;
        if cam_r < base_r {
            // below the layer: start where the ray leaves the base sphere (it hits the ground otherwise)
            t0 = max(tb1, 0.0);
        } else if tb0 > 0.0 {
            // above or inside, looking down: stop at the base
            t1 = min(t1, tb0);
        }
    }
    // the planet's surface ends the ray too
    let c_ground = dot(cam, cam) - radius * radius;
    let disc_g = b * b - c_ground;
    if disc_g > 0.0 && -b - sqrt(disc_g) > 0.0 {
        t1 = min(t1, -b - sqrt(disc_g));
    }
    t1 = min(t1, VCLOUD_RANGE);
    if t1 <= t0 {
        discard;
    }
    // 48 steps of at most 500 m: grazing rays through the layer stop early rather than stepping coarsely
    let steps = max(i32(frame.quality.y), 8);
    let dt = min((t1 - t0) / f32(steps), 500.0 * 48.0 / f32(steps));
    let jitter = fract(sin(dot(in.clip.xy, vec2<f32>(12.9898, 78.233))) * 43758.547);
    let sun = frame.sun.xyz;
    let time = frame.cam_km.w;
    let cos_t = dot(view, sun);
    // two-lobe phase: a forward silver lining and soft back scatter
    let g1 = 0.6;
    let g2 = -0.2;
    let hg1 = (1.0 - g1 * g1) / pow(1.0 + g1 * g1 - 2.0 * g1 * cos_t, 1.5);
    let hg2 = (1.0 - g2 * g2) / pow(1.0 + g2 * g2 - 2.0 * g2 * cos_t, 1.5);
    let phase = mix(hg2, hg1, 0.4) / (4.0 * PI);
    var trans = 1.0;
    var col = vec3<f32>(0.0);
    var first = -1.0;
    let sigma = 0.012;
    for (var i = 0; i < steps; i = i + 1) {
        let t = t0 + (f32(i) + jitter) * dt;
        let p = cam + view * t;
        let d = vc_density(p, base_r, top_r, time, true);
        if d <= 0.001 {
            continue;
        }
        if first < 0.0 {
            first = t;
        }
        // light reaching this point: a short march towards the sun
        var od = 0.0;
        for (var k = 1; k <= 3; k = k + 1) {
            let q = p + sun * (f32(k * k) * 120.0);
            od = od + vc_density(q, base_r, top_r, time, false) * f32(k * k) * 120.0;
        }
        let light_t = exp(-od * sigma * 0.6) * (1.0 - exp(-d * 2.0) * 0.5);
        let p_km = p * 0.001;
        let l = sun_and_sky(p_km, normalize(p), sun);
        let sun_in = l.sun / max(dot(normalize(p), sun), 0.05) * select(1.0, 0.0, dot(normalize(p), sun) < -0.1);
        // (a little light from the rest of the lit cloud, by day only)
        let daylit = smoothstep(-0.1, 0.1, dot(normalize(p), sun));
        let lum = sun_in * light_t * phase * 4.0 + l.sky * 1.2 + frame.solar.xyz * 0.01 * daylit;
        let ext = d * sigma * dt;
        let step_t = exp(-ext);
        let tint = min(textureSampleLevel(clouds_tex, equi_samp, equi_uv(normalize(p)), 2.0).rgb
            * surface.cloud_col.x, vec3<f32>(1.0));
        col = col + trans * (1.0 - step_t) * lum * 0.9 * tint;
        trans = trans * step_t;
        if trans < 0.02 {
            break;
        }
    }
    if first < 0.0 || trans > 0.995 {
        discard;
    }
    // the air between the camera and the cloud
    let entry = cam + view * first;
    let s_air = sky_to_point(frame.cam_km.xyz, entry * 0.001, sun);
    var rad = col;
    if frame.rayleigh.w > 0.5 {
        rad = col * s_air.transmittance + s_air.radiance * (1.0 - trans) * eclipse(frame.cam_km.xyz, sun);
    }
    let alpha = 1.0 - trans;
    // tone map the cloud's own colour and premultiply
    let mapped = tonemap(rad / max(alpha, 1e-3));
    out.colour = vec4<f32>(mapped * alpha, alpha);
    let clip = frame.view_proj * vec4<f32>(entry - cam, 1.0);
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    return out;
}
