//! A planet preview for checking the planet renderer on any body without flying there: open the client with
//! `?planet=Mun&lat=0&lon=0&alt=2000&heading=90&pitch=-10&sunaz=120&sunel=30` (degrees and metres above the
//! ground; all but `planet` optional). The page skips the menu and draws only the body (terrain, ocean, sky,
//! clouds) from that viewpoint, the way the flight view would. Used by the screenshot checks in
//! docs/reference-notes/planet-rendering.md.

use crate::planet::{PlanetRenderer, PlanetView};
use crate::planet_lod;
use crate::render::Renderer;
use glam::DVec3;

pub struct Preview {
    pub body: String,
    radius_m: f64,
    lat: f64,
    lon: f64,
    alt: f64,
    heading: f64,
    pitch: f64,
    sun_az: f64,
    sun_el: f64,
    debug: f32,
    exposure: f32,
    /// `quality=low|medium|high` overrides Settings > Graphics > Planet detail for the preview.
    quality: Option<u8>,
    /// `occ=<distance m>,<radius m>,<offset deg>`: a body that hides the sun, for checking eclipses.
    occ: Option<(f64, f64, f64)>,
    /// `part=<part id>`: draw that part 6 m in front of the camera (checks that the planet never covers craft).
    part: Option<String>,
    /// `plasma=<0..1>`: the re-entry sheath around that part (then 30 m away) at the corresponding entry speed, the air streaming away from the
    /// camera's right and a little down, seen from the side (reentry.rs).
    plasma: f64,
    planet: Option<PlanetRenderer>,
}

fn v(d: [f64; 3]) -> DVec3 {
    DVec3::new(d[0], d[1], d[2])
}

impl Preview {
    /// From the page's query string; `None` without `planet=`.
    pub fn from_query(query: &str) -> Option<Preview> {
        let mut get = std::collections::HashMap::new();
        for kv in query.trim_start_matches('?').split('&') {
            let mut it = kv.splitn(2, '=');
            if let (Some(k), Some(val)) = (it.next(), it.next()) {
                get.insert(k.to_string(), val.to_string());
            }
        }
        let body = get.get("planet")?.clone();
        let num = |k: &str, d: f64| get.get(k).and_then(|s| s.parse().ok()).unwrap_or(d);
        let text = include_str!("../../../assets/own/kerbol-system.json");
        let radius_m = serde_json::from_str::<serde_json::Value>(text)
            .ok()?
            .as_array()?
            .iter()
            .find(|b| b["id"].as_str() == Some(body.as_str()))?["radius_m"]
            .as_f64()?;
        Some(Preview {
            body,
            radius_m,
            lat: num("lat", -0.68),
            lon: num("lon", -74.82),
            alt: num("alt", 2_000.0),
            heading: num("heading", 90.0),
            pitch: num("pitch", -10.0),
            sun_az: num("sunaz", 120.0),
            sun_el: num("sunel", 30.0),
            debug: num("debug", 0.0) as f32,
            exposure: num("exp", 1.0) as f32,
            quality: get
                .get("quality")
                .map(|q| crate::settings::planet_quality(Some(q))),
            part: get.get("part").cloned(),
            plasma: num("plasma", 0.0),
            occ: get.get("occ").and_then(|s| {
                let v: Vec<f64> = s.split(',').filter_map(|x| x.parse().ok()).collect();
                (v.len() == 3).then(|| (v[0], v[1], v[2]))
            }),
            planet: None,
        })
    }

    pub fn frame(&mut self, gpu: &mut Renderer, now_ms: f64) {
        let debug = self.debug;
        let exposure = self.exposure;
        if let Some(q) = self.quality {
            crate::planet::set_quality(q);
        }
        let planet = self.planet.get_or_insert_with(|| {
            let mut p = PlanetRenderer::new(gpu);
            p.debug = debug;
            p.exposure_scale = exposure;
            p
        });
        let up = v(planet_lod::direction(self.lat, self.lon));
        let ground = planet.ground_m(&self.body, up).unwrap_or(0.0).max(0.0);
        let cam = up * (self.radius_m + ground + self.alt);
        let north = (DVec3::Y - up * up.y).normalize_or_zero();
        let east = north.cross(up);
        let dir_at = |az: f64, el: f64| {
            let (az, el) = (az.to_radians(), el.to_radians());
            (north * az.cos() + east * az.sin()) * el.cos() + up * el.sin()
        };
        let fwd = dir_at(self.heading, self.pitch);
        let sun = dir_at(self.sun_az, self.sun_el);
        let occluders: Vec<(DVec3, f64)> = self
            .occ
            .map(|(d, r, off)| (cam + dir_at(self.sun_az + off, self.sun_el) * d, r))
            .into_iter()
            .collect();
        gpu.set_camera(
            fwd,
            up,
            60f64.to_radians(),
            if self.part.is_some() {
                0.3
            } else {
                (self.alt * 0.01).clamp(0.5, 5_000.0)
            },
            1e9,
        );
        let ready = planet.prepare(
            gpu,
            &PlanetView {
                body_id: &self.body,
                radius_m: self.radius_m,
                cam,
                sun,
                time_s: now_ms / 1000.0,
                occluders: &occluders,
                sun_radius_rad: 0.0047,
            },
        );
        let view = PlanetView {
            body_id: &self.body,
            radius_m: self.radius_m,
            cam,
            sun,
            time_s: now_ms / 1000.0,
            occluders: &occluders,
            sun_radius_rad: 0.0047,
        };
        let mut items = match (&mut self.planet, ready) {
            (Some(p), true) => p.scatter_items(gpu, &view),
            _ => Vec::new(),
        };
        let mut engines = Vec::new();
        if let Some(part) = &self.part {
            let name = format!("part:{part}");
            // farther off with a plasma sheath, so the camera is outside it
            let dist = if self.plasma > 0.0 { 30.0 } else { 6.0 };
            gpu.request_model(&name, &format!("assets/parts/{part}.glb"));
            if gpu.has_model(&name) {
                let mut it =
                    crate::render::DrawItem::lit(name, fwd * dist, glam::DQuat::IDENTITY, 1.0, sun);
                it.ambient = 0.25;
                items.push(it);
                if self.plasma > 0.0 {
                    let speed = 1200.0 + 1800.0 * self.plasma.clamp(0.0, 1.0);
                    engines.push(crate::engine_fx_gpu::EngineDraw {
                        instance: 0,
                        part: format!("reentry:{part}"),
                        model: glam::DMat4::from_translation(fwd * dist),
                        throttle: 1.0,
                        commanded: 0.0,
                        pressure: 0.1,
                        atmosphere_density: 0.05,
                        gravity_mps2: 0.0,
                        relative_velocity: speed,
                        mode: 0,
                        time: now_ms * 0.001,
                        simulation_time: now_ms * 0.001,
                        thrust_kn: 0.0,
                        distance_from_spawn: 0.0,
                        vertical_deviation: 1.0,
                        vapor: None,
                        reentry: Some(crate::reentry::State {
                            heading: -(fwd.cross(up).normalize() - up * 0.3).normalize(),
                            density: 0.05,
                            altitude: self.alt,
                            speed,
                            mach: speed / 300.0,
                            gradient: match self.body.as_str() {
                                "Eve" => 1,
                                "Jool" => 2,
                                "Laythe" => 3,
                                _ => 0,
                            },
                        }),
                    });
                }
            }
        }
        let extra = self
            .planet
            .as_ref()
            .filter(|_| ready)
            .map(|p| p as &dyn crate::render::ExtraPass);
        gpu.frame_with_effects(
            "assets/parts/",
            &items,
            &[],
            [0.0, 0.0, 0.0],
            crate::render::FrameEffects {
                camera: cam,
                engines: &engines,
                extra,
            },
        );
    }
}
