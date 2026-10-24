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
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var transmittance_tex: texture_2d<f32>;
@group(0) @binding(2) var scattering_tex: texture_3d<f32>;
@group(0) @binding(3) var irradiance_tex: texture_2d<f32>;
@group(0) @binding(4) var lut_samp: sampler;

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

// Direct sun and sky irradiance on a surface at `p` (km) with normal `n`.
fn sun_and_sky(p: vec3<f32>, n: vec3<f32>, sun: vec3<f32>) -> Light {
    var out: Light;
    let has_atmo = frame.rayleigh.w > 0.5;
    let ndl = max(dot(n, sun), 0.0);
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
    let c = textureSampleLevel(clouds_tex, equi_samp, equi_uv(hit), 3.0);
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
// the camera and what it sees (_UnderWaterMaxVisibleDistance-like, 60 m).
fn water_light(depth_m: f32) -> vec3<f32> {
    let sun_up = max(dot(normalize(frame.cam_km.xyz), frame.sun.xyz), 0.0);
    let col = frame.water_col.rgb;
    return col * (0.08 + 0.6 * sun_up) * exp(-max(depth_m, 0.0) / 60.0);
}

fn water_fog(radiance: vec3<f32>, dist: f32) -> vec3<f32> {
    let f = 1.0 - exp(-dist / 60.0);
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
    return radiance * s.transmittance + s.radiance;
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
        let c = water_light(-frame.water.x) / PI * frame.solar.xyz * (0.5 + 1.5 * looking_up);
        return vec4<f32>(tonemap(c), 1.0);
    }
    var radiance = vec3<f32>(0.0);
    var t = vec3<f32>(1.0);
    if frame.rayleigh.w > 0.5 {
        let s = sky_radiance(frame.cam_km.xyz, view, sun);
        radiance = s.radiance;
        t = s.transmittance;
    }
    // The sun's disc.
    let a = frame.mie.w;
    if dot(view, sun) > cos(a) {
        radiance = radiance + t * frame.solar.xyz / (PI * a * a);
    }
    return vec4<f32>(tonemap(radiance), 1.0);
}

// ---------------------------------------------------------------------------------------------------------
// Terrain and ocean quads

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) height: f32,
    @location(3) extra: f32,
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
    var out: VOut;
    let rel = chunk.offset.xyz + in.pos;
    out.clip = frame.view_proj * vec4<f32>(rel, 1.0);
    out.rel = rel;
    out.normal = in.normal;
    out.tile = chunk.tile.xyz + in.pos;
    out.height = in.height;
    out.extra = in.extra;
    out.model = chunk.center.xyz + in.pos;
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
    let u2 = fract(uv.x + 0.5) - 0.5;
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

fn sample_equi(t: texture_2d<f32>, uv: vec2<f32>, g: EquiGrad) -> vec4<f32> {
    return textureSampleGrad(t, equi_samp, uv, g.dx, g.dy);
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

fn tri_sample(slot: i32, p: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, w: vec3<f32>, period: f32) -> vec4<f32> {
    let q = p / period;
    let qx = dpx / period;
    let qy = dpy / period;
    let a = textureSampleGrad(detail_a, wrap_samp, q.yz, slot, qx.yz, qy.yz);
    let b = textureSampleGrad(detail_a, wrap_samp, q.zx, slot, qx.zx, qy.zx);
    let c = textureSampleGrad(detail_a, wrap_samp, q.xy, slot, qx.xy, qy.xy);
    return a * w.x + b * w.y + c * w.z;
}

// Detail normal (the normSAO slices: x in alpha, y in green) projected like `tri_sample`, as a perturbation
// of the surface normal in the model frame.
fn tri_normal(slot: i32, p: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, w: vec3<f32>, period: f32) -> vec3<f32> {
    let q = p / period;
    let qx = dpx / period;
    let qy = dpy / period;
    let a = textureSampleGrad(detail_n, wrap_samp, q.yz, slot, qx.yz, qy.yz);
    let b = textureSampleGrad(detail_n, wrap_samp, q.zx, slot, qx.zx, qy.zx);
    let c = textureSampleGrad(detail_n, wrap_samp, q.xy, slot, qx.xy, qy.xy);
    let na = vec2<f32>(a.a, a.g) * 2.0 - 1.0;
    let nb = vec2<f32>(b.a, b.g) * 2.0 - 1.0;
    let nc = vec2<f32>(c.a, c.g) * 2.0 - 1.0;
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
    var albedo = sample_equi(albedo_tex, uv, eg).rgb;

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
                    if dist < 600.0 {
                        bump = bump + w * tri_normal(s, in.tile, dpx, dpy, tw, 8.0);
                    }
                    total = total + w;
                }
                if total > 0.0 {
                    let r = clamp(ratio / total, vec3<f32>(0.0), vec3<f32>(3.0));
                    dbg = r * 0.5;
                    albedo = albedo * mix(vec3<f32>(1.0), r, k);
                    let nk = k * (1.0 - smoothstep(150.0, 600.0, dist));
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
    let light = sun_and_sky(p_km, n, frame.sun.xyz);
    let shade = cloud_shadow(in.model, frame.sun.xyz);
    if surface.flags.z > 3.5 && surface.flags.z < 4.5 {
        return vec4<f32>(albedo, 1.0);
    }
    if surface.flags.z > 4.5 && surface.flags.z < 5.5 {
        return vec4<f32>(light.sun * 0.25, 1.0);
    }
    if surface.flags.z > 5.5 {
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
    let n = wave_normal(in.tile, up, frame.cam_km.w, dist);
    let view = normalize(in.rel);
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
    radiance = mix(radiance, sky, fres);
    // Sun glint.
    let h = normalize(sun - view);
    let smooth_ = surface.shallow.w;
    let shin = mix(64.0, 2048.0, smooth_ * smooth_);
    let spec = pow(max(dot(n, h), 0.0), shin) * (shin + 8.0) / (8.0 * PI);
    radiance = radiance + light.sun * spec * fres * 4.0;
    radiance = through_air(radiance, p_km);
    let alpha = mix(0.55, 1.0, smoothstep(0.0, 25.0, depth)) * smoothstep(-2.0, 0.5, depth);
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
    let c = sample_equi(clouds_tex, uv, equi_grad(uv));
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
    let cover = clamp(mix(base, smoothstep(0.35, 0.75, base + (n - 0.5) * 0.9), near), 0.0, 1.0);
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
    var radiance = vec3<f32>(0.9) / PI * lit;
    radiance = through_air(radiance, p_km);
    // Fade the shell when the camera is close to its altitude (it would read as a flat sheet).
    let fade = smoothstep(300.0, 2500.0, abs(cam_alt - surface.clouds.x));
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
    let fade = 1.0 - smoothstep(35.0, 55.0, dist);
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
