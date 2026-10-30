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
    /// Where each powered vessel was drawn when its latest state arrived, relative to where that state puts
    /// it: the prediction error, blended away over `CORRECTION_S` of wall time so it never shows as a jump.
    corrections: HashMap<u64, Correction>,
    /// Draw as before protocol 58 (`?netsmooth=0`): orbit elements for every vessel and no error blending.
    /// Kept so the smoothness benchmark can measure the difference on one build.
    pub legacy: bool,
}

/// A powered vessel's prediction error at the arrival of its latest state, and the display time it was
/// taken at.
#[derive(Clone, Copy, Debug)]
struct Correction {
    offset: DVec3,
    from_ut: f64,
    /// Universal-time span to blend it out over: `CORRECTION_S` of wall time at the warp of arrival.
    span_ut: f64,
}

/// Wall seconds a vessel's prediction error is blended away over at the arrival of a new state (one
/// snapshot interval).
pub const CORRECTION_S: f64 = 0.1;
/// Larger prediction errors are shown at once: they are teleports (loads, docking, decoupling), not jitter.
pub const MAX_CORRECTION_M: f64 = 25.0;

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
            corrections: HashMap::new(),
            legacy: false,
        }
    }

    pub fn reset_attitudes(&mut self) {
        self.attitudes.clear();
        self.corrections.clear();
    }

    /// A vessel under active physics and off the ground: its state is a point sample of a flight, so it is
    /// extrapolated kinematically and its prediction error is blended away.
    fn predicted(v: &VesselState) -> bool {
        !v.on_rails && v.flight.is_some() && !Self::standing(v)
    }

    fn standing(v: &VesselState) -> bool {
        v.flight.as_ref().is_some_and(|f| {
            matches!(
                f.situation,
                ose_net::FlightSituation::Landed | ose_net::FlightSituation::Crashed
            )
        })
    }

    /// Ingest authoritative state once; drawing samples the small attitude tracks without cloning craft.
    /// `display_ut` is the time the frame now on screen shows, so the new state can be compared with what
    /// was drawn and the difference faded out rather than shown as a jump.
    pub fn update_vessels(
        &mut self,
        vessels: Vec<VesselState>,
        ut: f64,
        now_ms: f64,
        warp: u32,
        display_ut: f64,
    ) {
        use crate::orientation::{from_axes, Context, Track};
        // Where each predicted vessel is drawn right now, before the new state replaces the old.
        let drawn: Vec<(u64, DVec3)> = vessels
            .iter()
            .filter(|v| Self::predicted(v) && !self.legacy)
            .filter_map(|v| {
                let old = self.vessel(v.id)?;
                (old.reference_body == v.reference_body && Self::predicted(old))
                    .then(|| (v.id, self.vessel_position(old, display_ut)))
            })
            .collect();
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
        self.corrections.clear();
        for (id, was) in drawn {
            let Some(v) = self.vessel(id) else { continue };
            let offset = was - self.raw_position(v, display_ut);
            let finite = offset.length().is_finite();
            if finite && offset.length() <= MAX_CORRECTION_M {
                self.corrections.insert(
                    id,
                    Correction {
                        offset,
                        from_ut: display_ut,
                        span_ut: CORRECTION_S * warp.max(1) as f64,
                    },
                );
            }
        }
    }

    /// The prediction error still shown for vessel `id` at `ut`: all of it at arrival, none a snapshot later.
    fn correction(&self, id: u64, ut: f64) -> DVec3 {
        match self.corrections.get(&id) {
            Some(c) if c.span_ut > 0.0 => {
                let left = 1.0 - ((ut - c.from_ut) / c.span_ut).clamp(0.0, 1.0);
                c.offset * left
            }
            _ => DVec3::ZERO,
        }
    }

    pub fn vessel_attitude(&self, id: u64, now_ms: f64) -> Option<(glam::DVec3, glam::DVec3)> {
        let rotation = self.attitudes.get(&id)?.sample(now_ms);
        Some((rotation * glam::DVec3::Y, rotation * glam::DVec3::Z))
    }

    /// Star-space position of a vessel at `ut`, as drawn: `raw_position` plus whatever is left of the
    /// prediction error being faded out since its latest state arrived.
    pub fn vessel_position(&self, v: &VesselState, ut: f64) -> DVec3 {
        self.raw_position(v, ut) + self.correction(v.id, ut)
    }

    /// Star-space position of a vessel at `ut` from its latest state alone, so it moves smoothly between
    /// `Vessels` messages: its reference body plus its orbit on rails, or the sent position and velocity
    /// carried on under gravity and its own acceleration under power. Falls back to the sent position if
    /// the reference body is unknown.
    fn raw_position(&self, v: &VesselState, ut: f64) -> DVec3 {
        let rb = v.reference_body as usize;
        // A craft standing on the ground turns with its body: keep its offset from the body's centre and rotate
        // it by the spin since the snapshot. Its orbit elements describe a fall through the planet, which made
        // a craft on the pad bob up and down between snapshots.
        let standing = Self::standing(v);
        if standing && rb < self.bodies.len() {
            let w = self.spin_omega.get(rb).copied().unwrap_or(0.0);
            let rel = v.position_m - self.position(rb, self.vessels_ut);
            let a = w * (ut - self.vessels_ut);
            let (s, c) = a.sin_cos();
            let turned = DVec3::new(rel.x * c - rel.y * s, rel.x * s + rel.y * c, rel.z);
            return self.position(rb, ut) + turned;
        }
        let Some(body) = self.bodies.get(rb) else {
            return v.position_m;
        };
        if Self::predicted(v) && !self.legacy {
            // Under power the osculating orbit is wrong by the thrust and drag of the next instant (and
            // degenerate just off the pad), so carry the sent state forward: constant acceleration over
            // a snapshot interval is accurate to millimetres.
            let r0 = v.position_m - self.position(rb, self.vessels_ut);
            let v0 = v.velocity_mps - self.body_velocity(rb, self.vessels_ut);
            let tau = ut - self.vessels_ut;
            let r = r0.length();
            let gravity = if r > 0.0 {
                r0 * (-body.mu / (r * r * r))
            } else {
                DVec3::ZERO
            };
            let rel = r0 + v0 * tau + (gravity + v.accel_mps2) * (0.5 * tau * tau);
            return self.position(rb, ut) + rel;
        }
        self.position(rb, ut) + orbit_from(body.mu, &v.orbit).position_at(ut)
    }

    /// Star-space velocity of body `i` at `ut`: the streamed snapshot's, which was taken at the same tick as
    /// the vessel states, else a central difference of its position.
    pub fn body_velocity(&self, i: usize, ut: f64) -> DVec3 {
        if (ut - self.vessels_ut).abs() < 1e-9 {
            if let Some(s) = self.snapshot.get(i) {
                return s.velocity_mps;
            }
        }
        let dt = 0.5;
        (self.position(i, ut + dt) - self.position(i, ut - dt)) * (0.5 / dt)
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

/// The client's estimate of server time. Each `TimeState` is one sample of (server time, arrival); the
/// samples carry network and scheduler jitter, so the clock does not jump to each one. It filters the offset
/// between server time and the page clock and slews the displayed time toward it at a bounded rate, so
/// everything drawn from it moves continuously. A warp change, a rewind or a disagreement beyond `SNAP_MS`
/// resets it outright. Between messages time is extrapolated, but never more than `MAX_EXTRAPOLATE_MS`, so
/// a stalled connection freezes rather than runs away.
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeSync {
    pub warp: u32,
    pub have_time: bool,
    /// Take every `TimeState` as the truth, as before protocol 58 (`?netsmooth=0`, for the benchmark).
    pub legacy: bool,
    /// Displayed server time at `base_now_ms`.
    base_ut: f64,
    base_now_ms: f64,
    /// What the displayed time still has to gain (or, negative, lose) to agree with the filtered samples, s.
    pending_s: f64,
}

pub const MAX_EXTRAPOLATE_MS: f64 = 500.0;
/// Fraction of the warp rate the displayed time may run fast or slow while it catches up.
pub const SLEW: f64 = 0.03;
/// Weight of each new sample in the filtered offset (at 10 Hz, a time constant of about 0.4 s).
const SAMPLE_WEIGHT: f64 = 0.25;
/// A filtered disagreement beyond this (wall-clock ms at the current warp) is a jump, not jitter: snap.
pub const SNAP_MS: f64 = 200.0;

impl TimeSync {
    pub fn new(legacy: bool) -> TimeSync {
        TimeSync {
            legacy,
            ..TimeSync::default()
        }
    }

    pub fn update(&mut self, ut: f64, warp: u32, now_ms: f64) {
        if self.legacy || !self.have_time || warp != self.warp {
            self.snap(ut, warp, now_ms);
            return;
        }
        // Bank the slew applied so far so the displayed time stays continuous through this update.
        let displayed = self.ut_at(now_ms);
        let applied = displayed - (self.base_ut + self.dt_s(now_ms) * self.warp as f64);
        self.pending_s -= applied;
        self.base_ut = displayed;
        self.base_now_ms = now_ms;
        let error = ut - displayed;
        self.pending_s += (error - self.pending_s) * SAMPLE_WEIGHT;
        if !self.pending_s.is_finite() || self.pending_s.abs() > SNAP_MS / 1000.0 * warp as f64 {
            self.snap(ut, warp, now_ms);
        }
    }

    fn snap(&mut self, ut: f64, warp: u32, now_ms: f64) {
        self.base_ut = ut;
        self.base_now_ms = now_ms;
        self.warp = warp;
        self.pending_s = 0.0;
        self.have_time = true;
    }

    fn dt_s(&self, now_ms: f64) -> f64 {
        (now_ms - self.base_now_ms).clamp(0.0, MAX_EXTRAPOLATE_MS) / 1000.0
    }

    pub fn ut_at(&self, now_ms: f64) -> f64 {
        let dt = self.dt_s(now_ms);
        let rate = self.warp as f64;
        let slew = self.pending_s.clamp(-SLEW * rate * dt, SLEW * rate * dt);
        self.base_ut + dt * rate + slew
    }

    /// The displayed time's disagreement with the filtered samples, ms of wall clock at the current warp:
    /// what the slew still has to make up.
    pub fn pending_ms(&self) -> f64 {
        self.pending_s * 1000.0 / self.warp.max(1) as f64
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
            accel_mps2: DVec3::ZERO,
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
            craft_rev: 0,
            on_rails: true,
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

    /// Deterministic jitter in [-amplitude, amplitude] ms.
    fn jitter(i: u32, amplitude: f64) -> f64 {
        let x = (i as f64 * 12.9898).sin() * 43758.5453;
        (x - x.floor()) * 2.0 * amplitude - amplitude
    }

    /// Ten snapshots a second arriving up to 30 ms early or late: the displayed time never jumps, runs within
    /// the slew bound of real time and stays close to it. The old clock, which took each arrival as the
    /// truth, jumped by the arrival jitter (up to 60 ms frame to frame) on every message.
    #[test]
    fn jittery_arrivals_keep_displayed_time_continuous() {
        let mut t = TimeSync::default();
        let mut arrivals: Vec<(f64, f64)> = (0..100)
            .map(|i| {
                let sent_ms = i as f64 * 100.0;
                (sent_ms + 80.0 + jitter(i, 30.0), 1000.0 + sent_ms / 1000.0)
            })
            .collect();
        arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut next = 0;
        let mut last: Option<(f64, f64)> = None;
        let mut worst_step = 0.0f64;
        let mut worst_error = 0.0f64;
        let mut now = 0.0;
        while now < 9_900.0 {
            while next < arrivals.len() && arrivals[next].0 <= now {
                t.update(arrivals[next].1, 1, arrivals[next].0);
                next += 1;
            }
            if t.have_time {
                let ut = t.ut_at(now);
                if let Some((prev_now, prev_ut)) = last {
                    let step = (ut - prev_ut) * 1000.0;
                    let frame = now - prev_now;
                    assert!(
                        step >= frame * (1.0 - SLEW) - 1e-9,
                        "ran back at {now}: {step} ms"
                    );
                    assert!(
                        step <= frame * (1.0 + SLEW) + 1e-9,
                        "jumped at {now}: {step} ms"
                    );
                    worst_step = worst_step.max((step - frame).abs());
                }
                if now > 1_000.0 {
                    // The server's time at `now` is 1000 + now/1000; the display lags it by the one-way delay.
                    let error = (ut - (1000.0 + now / 1000.0)) * 1000.0;
                    worst_error = worst_error.max((error + 80.0).abs());
                }
                last = Some((now, ut));
            }
            now += 16.0;
        }
        assert!(worst_step <= 16.0 * SLEW + 1e-9, "{worst_step}");
        assert!(
            worst_error < 25.0,
            "{worst_error} ms from the delayed server time"
        );
    }

    #[test]
    fn warp_changes_and_rewinds_snap_the_clock() {
        let mut t = TimeSync::default();
        t.update(100.0, 1, 0.0);
        t.update(100.1, 1, 100.0);
        t.update(100.2, 4, 200.0);
        assert_eq!(t.warp, 4);
        assert!((t.ut_at(300.0) - 100.6).abs() < 1e-9);
        // a quickload takes time back by a minute: shown at once
        t.update(40.0, 4, 300.0);
        assert!((t.ut_at(300.0) - 40.0).abs() < 1e-9);
        assert_eq!(t.pending_ms(), 0.0);
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
