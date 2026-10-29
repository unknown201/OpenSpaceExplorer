//! Map-view scene: the body catalog, where each body is at a given UT, and the camera that maps star-space
//! metres to canvas pixels. Pure data and math; no DOM, so it is unit-tested natively.
//!
//! Positions come from the catalog's Keplerian elements evaluated at the client's time estimate (ADR-0002
//! allows Keplerian extrapolation for smooth presentation); the server's `Snapshot` is kept as the fallback
//! for bodies without elements.

use ose_math::orbit::{Elements, Orbit};
use ose_math::DVec3;
use ose_net::{BodyInfo, BodyState, OrbitElements, TrajectoryPatch, VesselState};
use std::collections::HashMap;

pub struct Scene {
    pub bodies: Vec<BodyInfo>,
    orbits: Vec<Option<Orbit>>,
    pub snapshot: Vec<BodyState>,
    /// Vessels from the last `Vessels` message, in the order sent, and the universal time of that message.
    pub vessels: Vec<VesselState>,
    pub vessels_ut: f64,
    /// Each body's spin rate (rad/s about star-space +Z), from `BodyRotation`.
    pub spin_omega: Vec<f64>,
    /// Predicted patched-conic trajectory per vessel id, from the last `Trajectory` message.
    pub trajectories: HashMap<u64, Vec<TrajectoryPatch>>,
    attitudes: HashMap<u64, crate::orientation::Track>,
}

/// `ose-math` orbit from wire elements around a body with gravitational parameter `mu`.
pub fn orbit_from(mu: f64, o: &OrbitElements) -> Orbit {
    Orbit::from_elements(
        mu,
        Elements {
            semi_major_axis: o.semi_major_axis_m,
            eccentricity: o.eccentricity,
            inclination_deg: o.inclination_deg,
            longitude_of_ascending_node_deg: o.longitude_of_ascending_node_deg,
            argument_of_periapsis_deg: o.argument_of_periapsis_deg,
            mean_anomaly_at_epoch: o.mean_anomaly_at_epoch_rad,
            epoch: o.epoch_s,
        },
    )
}

impl Scene {
    pub fn new(bodies: Vec<BodyInfo>) -> Scene {
        let orbits = bodies
            .iter()
            .map(|b| {
                let parent = b.parent.map(|p| &bodies[p as usize])?;
                let o = b.orbit?;
                Some(orbit_from(parent.mu, &o))
            })
            .collect();
        Scene {
            bodies,
            orbits,
            snapshot: Vec::new(),
            vessels: Vec::new(),
            vessels_ut: 0.0,
            spin_omega: Vec::new(),
            trajectories: HashMap::new(),
            attitudes: HashMap::new(),
        }
    }

    pub fn reset_attitudes(&mut self) {
        self.attitudes.clear();
    }

    /// Ingest authoritative state once; drawing samples the small attitude tracks without cloning craft.
    pub fn update_vessels(&mut self, vessels: Vec<VesselState>, ut: f64, now_ms: f64, warp: u32) {
        use crate::orientation::{from_axes, Context, Track};
        let live: std::collections::HashSet<_> = vessels
            .iter()
            .filter(|v| v.flight.is_some())
            .map(|v| v.id)
            .collect();
        self.attitudes.retain(|id, _| live.contains(id));
        for v in &vessels {
            let Some(flight) = &v.flight else { continue };
            let g = |v: DVec3| glam::DVec3::new(v.x, v.y, v.z);
            let Some(rotation) = from_axes(g(flight.facing), g(flight.roll_ref)) else {
                self.attitudes.remove(&v.id);
                continue;
            };
            let context = Context {
                body: v.reference_body,
                warp,
                grounded: matches!(
                    flight.situation,
                    ose_net::FlightSituation::Landed | ose_net::FlightSituation::Crashed
                ),
            };
            match self.attitudes.entry(v.id) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().update(rotation, ut, now_ms, context);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Track::new(rotation, ut, now_ms, context));
                }
            }
            if let Some(track) = self.attitudes.get_mut(&v.id) {
                track.set_world_velocity(g(flight.angvel));
            }
        }
        self.vessels_ut = ut;
        self.vessels = vessels;
    }

    pub fn vessel_attitude(&self, id: u64, now_ms: f64) -> Option<(glam::DVec3, glam::DVec3)> {
        let rotation = self.attitudes.get(&id)?.sample(now_ms);
        Some((rotation * glam::DVec3::Y, rotation * glam::DVec3::Z))
    }

    /// Star-space position of a vessel at `ut`: its reference body plus its orbit, so it moves smoothly
    /// between `Vessels` messages. Falls back to the sent position if the reference body is unknown.
    pub fn vessel_position(&self, v: &VesselState, ut: f64) -> DVec3 {
        let rb = v.reference_body as usize;
        // A craft standing on the ground turns with its body: keep its offset from the body's centre and rotate
        // it by the spin since the snapshot. Its orbit elements describe a fall through the planet, which made
        // a craft on the pad bob up and down between snapshots.
        let standing = v.flight.as_ref().is_some_and(|f| {
            matches!(
                f.situation,
                ose_net::FlightSituation::Landed | ose_net::FlightSituation::Crashed
            )
        });
        if standing && rb < self.bodies.len() {
            let w = self.spin_omega.get(rb).copied().unwrap_or(0.0);
            let rel = v.position_m - self.position(rb, self.vessels_ut);
            let a = w * (ut - self.vessels_ut);
            let (s, c) = a.sin_cos();
            let turned = DVec3::new(rel.x * c - rel.y * s, rel.x * s + rel.y * c, rel.z);
            return self.position(rb, ut) + turned;
        }
        match self.bodies.get(v.reference_body as usize) {
            Some(body) => {
                self.position(v.reference_body as usize, ut)
                    + orbit_from(body.mu, &v.orbit).position_at(ut)
            }
            None => v.position_m,
        }
    }

    pub fn vessel(&self, id: u64) -> Option<&VesselState> {
        self.vessels.iter().find(|v| v.id == id)
    }

    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.bodies
            .iter()
            .position(|b| b.id.eq_ignore_ascii_case(id))
    }

    pub fn root(&self) -> Option<usize> {
        self.bodies.iter().position(|b| b.parent.is_none())
    }

    pub fn orbit(&self, i: usize) -> Option<&Orbit> {
        self.orbits.get(i).and_then(|o| o.as_ref())
    }

    /// Star-space position of body `i` at `ut`, walking the parent chain.
    pub fn position(&self, i: usize, ut: f64) -> DVec3 {
        let mut pos = DVec3::ZERO;
        let mut cur = Some(i);
        let mut hops = 0;
        while let Some(k) = cur {
            match (&self.orbits[k], self.bodies[k].parent) {
                (Some(orbit), Some(p)) => {
                    pos = pos + orbit.position_at(ut);
                    cur = Some(p as usize);
                }
                (None, Some(_)) => {
                    // No elements: fall back to the last snapshot, which is already in star space.
                    if let Some(s) = self.snapshot.get(k) {
                        return pos + s.position_m;
                    }
                    cur = None;
                }
                _ => cur = None,
            }
            hops += 1;
            if hops > 32 {
                break;
            }
        }
        pos
    }

    /// Children of `parent` in catalog order.
    pub fn children(&self, parent: usize) -> impl Iterator<Item = usize> + '_ {
        self.bodies
            .iter()
            .enumerate()
            .filter(move |(_, b)| b.parent == Some(parent as u16))
            .map(|(i, _)| i)
    }

    /// Radius of the largest child orbit of `i` (apoapsis), or a few body radii if it has no children.
    pub fn system_extent(&self, i: usize) -> f64 {
        self.children(i)
            .filter_map(|c| self.orbit(c).map(|o| o.apoapsis()))
            .fold(self.bodies[i].radius_m * 4.0, f64::max)
    }
}

/// The client's estimate of server time: the last `TimeState`, when it arrived (performance.now ms), and the
/// warp it is advancing at. Between messages time is extrapolated, but never more than `MAX_EXTRAPOLATE_MS`
/// so a stalled connection freezes rather than runs away.
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeSync {
    pub base_ut: f64,
    pub base_now_ms: f64,
    pub warp: u32,
    pub have_time: bool,
}

pub const MAX_EXTRAPOLATE_MS: f64 = 500.0;

impl TimeSync {
    pub fn update(&mut self, ut: f64, warp: u32, now_ms: f64) {
        self.base_ut = ut;
        self.base_now_ms = now_ms;
        self.warp = warp;
        self.have_time = true;
    }

    pub fn ut_at(&self, now_ms: f64) -> f64 {
        let dt = (now_ms - self.base_now_ms).clamp(0.0, MAX_EXTRAPOLATE_MS) / 1000.0;
        self.base_ut + dt * self.warp as f64
    }
}

/// Top-down camera on the ecliptic (star-space X right, Y up on screen) centred on a body plus an offset.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub focus: usize,
    /// Pan offset from the focus body, metres.
    pub offset: DVec3,
    /// Metres per CSS pixel.
    pub m_per_px: f64,
    pub width: f64,
    pub height: f64,
}

impl Camera {
    pub fn new(focus: usize) -> Camera {
        Camera {
            focus,
            offset: DVec3::ZERO,
            m_per_px: 1.0e8,
            width: 800.0,
            height: 600.0,
        }
    }

    /// Choose a scale that fits a radius of `extent_m` into the smaller half-dimension with a margin.
    pub fn frame(&mut self, extent_m: f64) {
        let half = (self.width.min(self.height) / 2.0).max(1.0);
        self.m_per_px = extent_m / (half * 0.85);
        self.offset = DVec3::ZERO;
    }

    pub fn to_screen(&self, p: DVec3, centre: DVec3) -> (f64, f64) {
        let d = p - centre - self.offset;
        (
            self.width / 2.0 + d.x / self.m_per_px,
            self.height / 2.0 - d.y / self.m_per_px,
        )
    }

    /// Zoom by `factor` keeping the star-space point under screen `(sx, sy)` fixed.
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64) {
        let dx = sx - self.width / 2.0;
        let dy = self.height / 2.0 - sy;
        let before = DVec3::new(dx * self.m_per_px, dy * self.m_per_px, 0.0);
        self.m_per_px = (self.m_per_px * factor).clamp(1.0, 1.0e13);
        let after = DVec3::new(dx * self.m_per_px, dy * self.m_per_px, 0.0);
        self.offset = self.offset + before - after;
    }

    pub fn pan_px(&mut self, dx_px: f64, dy_px: f64) {
        self.offset = self.offset - DVec3::new(dx_px * self.m_per_px, -dy_px * self.m_per_px, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_net::OrbitElements;

    fn kerbol_kerbin_mun() -> Vec<BodyInfo> {
        let body = |id: &str, parent: Option<u16>, radius_m, mu, orbit| BodyInfo {
            id: id.into(),
            parent,
            radius_m,
            mu,
            soi_radius_m: None,
            is_star: parent.is_none(),
            has_atmosphere: false,
            atmosphere_depth_m: 0.0,
            orbit,
        };
        let circ = |a: f64| OrbitElements {
            semi_major_axis_m: a,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            longitude_of_ascending_node_deg: 0.0,
            argument_of_periapsis_deg: 0.0,
            mean_anomaly_at_epoch_rad: 0.0,
            epoch_s: 0.0,
        };
        vec![
            body("Kerbol", None, 261_600_000.0, 1.1723328e18, None),
            body(
                "Kerbin",
                Some(0),
                600_000.0,
                3.5316e12,
                Some(circ(13_599_840_256.0)),
            ),
            body(
                "Mun",
                Some(1),
                200_000.0,
                6.5138398e10,
                Some(circ(12_000_000.0)),
            ),
        ]
    }

    #[test]
    fn positions_walk_the_parent_chain() {
        let s = Scene::new(kerbol_kerbin_mun());
        let kerbin = s.position(1, 0.0);
        let mun = s.position(2, 0.0);
        assert!((kerbin.length() - 13_599_840_256.0).abs() < 1.0);
        assert!(((mun - kerbin).length() - 12_000_000.0).abs() < 1.0);
        assert_eq!(s.root(), Some(0));
        assert_eq!(s.index_of("kerbin"), Some(1));
        assert!(s.system_extent(0) > 13.0e9);
    }

    #[test]
    fn vessel_rides_its_reference_body() {
        let s = Scene::new(kerbol_kerbin_mun());
        let v = VesselState {
            id: 7,
            name: "probe".into(),
            owner: Some(1),
            reference_body: 1,
            position_m: DVec3::ZERO,
            velocity_mps: DVec3::ZERO,
            orbit: OrbitElements {
                semi_major_axis_m: 700_000.0,
                eccentricity: 0.0,
                inclination_deg: 0.0,
                longitude_of_ascending_node_deg: 0.0,
                argument_of_periapsis_deg: 0.0,
                mean_anomaly_at_epoch_rad: 0.0,
                epoch_s: 0.0,
            },
            target: None,
            flight: None,
        };
        let kerbin = s.position(1, 0.0);
        let p = s.vessel_position(&v, 0.0);
        assert!(((p - kerbin).length() - 700_000.0).abs() < 1.0);
    }

    #[test]
    fn time_extrapolates_but_is_capped() {
        let mut t = TimeSync::default();
        t.update(100.0, 1000, 5000.0);
        assert_eq!(t.ut_at(5100.0), 200.0);
        assert_eq!(t.ut_at(9000.0), 600.0);
        assert_eq!(t.ut_at(4000.0), 100.0);
    }

    #[test]
    fn camera_round_trips_and_zooms_about_cursor() {
        let mut c = Camera::new(0);
        c.width = 1000.0;
        c.height = 500.0;
        c.frame(1.0e9);
        let centre = DVec3::ZERO;
        let (x, y) = c.to_screen(DVec3::new(1.0e9, 0.0, 0.0), centre);
        assert!((y - 250.0).abs() < 1e-6 && x > 500.0);
        let p = DVec3::new(3.0e8, -2.0e8, 0.0);
        let (sx, sy) = c.to_screen(p, centre);
        c.zoom_at(0.5, sx, sy);
        let (sx2, sy2) = c.to_screen(p, centre);
        assert!((sx - sx2).abs() < 1e-6 && (sy - sy2).abs() < 1e-6);
    }
}
