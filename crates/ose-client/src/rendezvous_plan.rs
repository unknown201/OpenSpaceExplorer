//! A rolling, advisory flight plan. These nodes never become executable server maneuver nodes.
use ose_math::{rendezvous as mechanics, DVec3, Orbit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Transfer,
    Correction,
    MatchVelocity,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Transfer => "Transfer",
            Self::Correction => "Course correction",
            Self::MatchVelocity => "Match velocity",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Burn {
    pub number: u32,
    pub kind: Kind,
    pub ut: f64,
    pub delta_v: DVec3,
    pub before: Orbit,
    pub after: Orbit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Partial,
    Missed,
}
impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "Burn complete",
            Self::Partial => "Partial burn · path updated",
            Self::Missed => "Missed · path updated",
        }
    }
}
#[derive(Clone, Debug)]
pub struct PastBurn {
    pub number: u32,
    pub kind: Kind,
    pub outcome: Outcome,
}
#[derive(Clone, Copy)]
pub struct State {
    pub chaser: Orbit,
    pub target: Orbit,
    pub now: f64,
    pub minimum_radius: f64,
    pub budget: f64,
    pub acceleration: f64,
}

#[derive(Default)]
pub struct Route {
    pub nodes: Vec<Burn>,
    pub history: Vec<PastBurn>,
    pub remaining: DVec3,
    pub revision: u32,
    pub arrival: Option<f64>,
    pub handoff: bool,
    gate_delay: f64,
    next_number: u32,
    sample: Option<(f64, DVec3, DVec3, bool)>,
    burned: bool,
    idle_since: Option<f64>,
}
impl Route {
    pub fn segments(&self, now: f64) -> Vec<(Orbit, f64, f64)> {
        let mut spans = Vec::new();
        if let Some(first) = self.nodes.first() {
            if first.ut > now {
                spans.push((first.before, now, first.ut));
            }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            let end = self
                .nodes
                .get(i + 1)
                .map_or(n.ut + n.after.period.min(3_000.0) * 0.1, |next| next.ut);
            if end > now {
                spans.push((n.after, n.ut.max(now), end));
            }
        }
        spans
    }
    pub fn new(state: State, clearance_m: f64) -> Self {
        let slowest = if state.target.period.is_finite() {
            state
                .target
                .state_at_true_anomaly(std::f64::consts::PI)
                .1
                .length()
        } else {
            state.target.state_at(state.now).1.length()
        };
        let mut route = Self {
            gate_delay: clearance_m.max(500.0) * 1.01 / slowest,
            ..Self::default()
        };
        route.rebuild(state);
        route
    }

    /// An orbit phased ahead of the station: encounter at a stand-off gate, never its centre.
    pub fn gate(&self, target: Orbit, now: f64) -> Orbit {
        let (r, v) = target.state_at(now + self.gate_delay);
        Orbit::from_state_vectors(target.mu, r, v, now)
    }

    fn push(&mut self, kind: Kind, ut: f64, delta_v: DVec3, before: Orbit) {
        self.next_number += 1;
        let (r, v) = before.state_at(ut);
        self.nodes.push(Burn {
            number: self.next_number,
            kind,
            ut,
            delta_v,
            before,
            after: Orbit::from_state_vectors(before.mu, r, v + delta_v, ut),
        });
    }

    fn rebuild(&mut self, s: State) {
        self.revision += 1;
        self.nodes.clear();
        // Number the current sequence, retaining the arrival number when no correction is inserted.
        self.next_number = self.history.last().map_or(0, |n| n.number);
        self.sample = None;
        self.burned = false;
        self.idle_since = None;
        let gate = self.gate(s.target, s.now);
        let (r, v) = s.chaser.state_at(s.now);
        let (tr, tv) = s.target.state_at(s.now);
        let distance = (tr - r).length();
        self.handoff = distance < 3_000.0 && (tv - v).length() < 2.0;
        if self.handoff {
            self.remaining = DVec3::ZERO;
            return;
        }
        if distance < 8_000.0 && (tv - v).length() > 2.0 {
            self.arrival = Some(s.now);
            if (tv - v).length() <= s.budget {
                self.push(Kind::MatchVelocity, s.now, tv - v, s.chaser);
            }
        } else {
            let lead = (s.budget / (2.0 * s.acceleration.max(0.01))).clamp(30.0, 120.0);
            // First test the actual coast. A good departure needs only the arrival burn.
            let start = (s.now + 20.0).max(self.arrival.unwrap_or(s.now) - 300.0);
            let end = self
                .arrival
                .map_or(s.now + s.target.period * 3.0, |t| t + 300.0);
            let encounter = closest(s.chaser, gate, start, end);
            if let Some(t) = encounter.filter(|t| {
                let (a, av) = s.chaser.state_at(*t);
                let (b, bv) = gate.state_at(*t);
                // Keep coast error inside the gate's extra 500 m clearance margin.
                (a - b).length() < 200.0 && (bv - av).length() <= s.budget
            }) {
                self.arrival = Some(t);
                self.push(
                    Kind::MatchVelocity,
                    t,
                    gate.state_at(t).1 - s.chaser.state_at(t).1,
                    s.chaser,
                );
            } else {
                let correction = self.arrival.and_then(|at| {
                    mechanics::correction(
                        s.chaser,
                        gate,
                        s.now + lead,
                        at,
                        s.minimum_radius,
                        s.budget,
                    )
                });
                if let Some(p) = correction.or_else(|| {
                    mechanics::plan(s.chaser, gate, s.now + lead, s.minimum_radius, s.budget)
                }) {
                    self.arrival = Some(p.arrival_ut);
                    self.push(
                        if self.history.is_empty() {
                            Kind::Transfer
                        } else {
                            Kind::Correction
                        },
                        p.departure_ut,
                        p.delta_v,
                        s.chaser,
                    );
                    self.push(
                        Kind::MatchVelocity,
                        p.arrival_ut,
                        p.arrival_delta_v,
                        p.orbit,
                    );
                }
            }
        }
        self.remaining = self.nodes.first().map_or(DVec3::ZERO, |n| n.delta_v);
    }

    /// Observe authoritative motion. Time crossing alone can only mark a burn missed, never complete.
    /// A cutoff triggers one replan after the engine tail has settled. Long/warped samples are not
    /// integrated as impulses; their actual resulting orbit is still used for the new plan.
    pub fn observe(&mut self, s: State, firing: bool, controls_idle: bool) -> bool {
        let Some(node) = self.nodes.first() else {
            return false;
        };
        let (r, v) = s.chaser.state_at(s.now);
        let (tr, tv) = s.target.state_at(s.now);
        if !self.burned
            && !firing
            && controls_idle
            && node.kind == Kind::MatchVelocity
            && (tr - r).length() < 3_000.0
            && (tv - v).length() < 2.0
        {
            self.rebuild(s);
            return true;
        }
        if let Some((at, r0, v0, was_firing)) = self.sample {
            if s.now <= at {
                return false;
            }
            if firing || was_firing {
                self.burned = true;
                if s.now - at < 10.0 {
                    self.remaining = self.remaining
                        - mechanics::delivered_delta_v(s.chaser.mu, r0, v0, r, v, s.now - at);
                }
            }
        }
        self.sample = Some((s.now, r, v, firing));
        if firing {
            self.burned = true;
        }
        if controls_idle && !firing && self.burned {
            let idle = *self.idle_since.get_or_insert(s.now);
            if s.now - idle >= 0.75 {
                let tolerance = (node.delta_v.length() * 0.03).clamp(0.1, 0.5);
                let outcome = if self.remaining.length() <= tolerance {
                    Outcome::Complete
                } else {
                    Outcome::Partial
                };
                self.history.push(PastBurn {
                    number: node.number,
                    kind: node.kind,
                    outcome,
                });
                self.rebuild(s);
                return true;
            }
        } else {
            self.idle_since = None;
        }
        let late = 120.0_f64.max(node.delta_v.length() / s.acceleration.max(0.01) * 0.5 + 30.0);
        if !self.burned && controls_idle && !firing && s.now > node.ut + late {
            self.history.push(PastBurn {
                number: node.number,
                kind: node.kind,
                outcome: Outcome::Missed,
            });
            self.rebuild(s);
            return true;
        }
        false
    }
}

fn closest(a: Orbit, b: Orbit, start: f64, end: f64) -> Option<f64> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return None;
    }
    let distance = |t| (a.state_at(t).0 - b.state_at(t).0).length_squared();
    let step = (end - start) / 256.0;
    let at = (0..=256)
        .map(|i| start + step * i as f64)
        .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))?;
    let (mut lo, mut hi) = ((at - step).max(start), (at + step).min(end));
    for _ in 0..40 {
        let x = lo + (hi - lo) / 3.0;
        let y = hi - (hi - lo) / 3.0;
        if distance(x) < distance(y) {
            hi = y;
        } else {
            lo = x;
        }
    }
    Some((lo + hi) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MU: f64 = 3.5316e12;
    fn circular(radius: f64, phase: f64) -> Orbit {
        let (s, c) = phase.sin_cos();
        Orbit::from_state_vectors(
            MU,
            DVec3::new(c, s, 0.0) * radius,
            DVec3::new(-s, c, 0.0) * (MU / radius).sqrt(),
            0.0,
        )
    }
    fn state() -> State {
        State {
            chaser: circular(690_000.0, 0.0),
            target: circular(710_000.0, 0.8),
            now: 0.0,
            minimum_radius: 671_000.0,
            budget: 2_000.0,
            acceleration: 5.0,
        }
    }
    fn burn(route: &mut Route, mut s: State, fraction: f64) -> State {
        let n = route.nodes[0].clone();
        s.now = n.ut - 0.01;
        route.observe(s, false, true);
        let (r, v) = s.chaser.state_at(n.ut);
        s.chaser = Orbit::from_state_vectors(MU, r, v + n.delta_v * fraction, n.ut);
        s.now = n.ut + 0.01;
        assert!(!route.observe(s, true, false));
        s.now += 0.1;
        assert!(!route.observe(s, false, true));
        s.now += 1.0;
        assert!(route.observe(s, false, true), "cutoff must rebuild once");
        assert!(
            !route.observe(s, false, true),
            "same snapshot must not rebuild twice"
        );
        s
    }
    #[test]
    fn initial_route_chains_departure_and_braking_at_a_stand_off_gate() {
        let s = state();
        let route = Route::new(s, 600.0);
        assert_eq!(route.nodes.len(), 2);
        let (a, b) = (&route.nodes[0], &route.nodes[1]);
        assert_eq!(a.kind, Kind::Transfer);
        assert_eq!(b.kind, Kind::MatchVelocity);
        assert!(b.ut > a.ut);
        assert!((a.after.state_at(b.ut).0 - b.before.state_at(b.ut).0).length() < 0.1);
        assert!(
            (b.before.state_at(b.ut).0 - route.gate(s.target, s.now).state_at(b.ut).0).length()
                < 5.0
        );
        assert!((b.before.state_at(b.ut).0 - s.target.state_at(b.ut).0).length() > 590.0);
        assert!(
            (b.after.state_at(b.ut).1 - route.gate(s.target, s.now).state_at(b.ut).1).length()
                < 0.01
        );
    }
    #[test]
    fn eccentric_station_keeps_gate_clearance_at_its_slowest_point() {
        let mut s = state();
        let r = DVec3::new(720_000.0, 0.0, 0.0);
        let v = DVec3::new(
            0.0,
            (MU * (2.0 / 720_000.0 - 1.0 / 1_000_000.0)).sqrt(),
            0.0,
        );
        s.target = Orbit::from_state_vectors(MU, r, v, 0.0);
        let route = Route::new(s, 600.0);
        let gate = route.gate(s.target, 0.0);
        for i in 0..=100 {
            let t = s.target.period * i as f64 / 100.0;
            assert!((gate.state_at(t).0 - s.target.state_at(t).0).length() >= 600.0);
        }
    }
    #[test]
    fn flown_departure_rebuilds_arrival_then_hands_off_after_braking() {
        let s = state();
        let mut route = Route::new(s, 500.0);
        let arrival = route.arrival.unwrap();
        let s = burn(&mut route, s, 1.0);
        assert_eq!(route.history[0].outcome, Outcome::Complete);
        assert_eq!(route.nodes.len(), 1);
        assert_eq!(route.nodes[0].kind, Kind::MatchVelocity);
        assert!((route.arrival.unwrap() - arrival).abs() < 2.0);
        let s = burn(&mut route, s, 1.0);
        assert_eq!(route.history[1].outcome, Outcome::Complete);
        assert!(route.handoff);
        assert!(route.nodes.is_empty());
        assert!((s.chaser.state_at(s.now).0 - s.target.state_at(s.now).0).length() > 450.0);
    }
    #[test]
    fn partial_burn_inserts_a_correction_and_keeps_the_encounter() {
        let s = state();
        let mut route = Route::new(s, 500.0);
        let eta = route.arrival.unwrap();
        let s = burn(&mut route, s, 0.8);
        assert_eq!(route.history[0].outcome, Outcome::Partial);
        assert_eq!(route.nodes[0].kind, Kind::Correction);
        assert!((route.arrival.unwrap() - eta).abs() < 0.01);
        let s = burn(&mut route, s, 1.0);
        assert_eq!(route.nodes[0].kind, Kind::MatchVelocity);
        burn(&mut route, s, 1.0);
        assert!(route.handoff);
        assert_eq!(route.history.len(), 3);
    }
    #[test]
    fn crossing_time_without_thrust_is_missed_and_replanned() {
        let mut s = state();
        let mut route = Route::new(s, 500.0);
        s.now = route.nodes[0].ut + 200.0;
        assert!(route.observe(s, false, true));
        assert_eq!(route.history[0].outcome, Outcome::Missed);
        assert_eq!(route.revision, 2);
        assert!(route.nodes[0].ut > s.now);
    }
    #[test]
    fn gravity_and_coasting_do_not_consume_the_burn() {
        let mut s = state();
        let mut route = Route::new(s, 500.0);
        let initial = route.remaining;
        for t in [0.0, 0.1, 0.5, 1.0, 10.0] {
            s.now = t;
            assert!(!route.observe(s, false, true));
        }
        assert_eq!(route.remaining, initial);
        assert!(route.history.is_empty());
    }
    #[test]
    fn unrelated_early_thrust_updates_the_route_without_claiming_completion() {
        let mut s = state();
        let mut route = Route::new(s, 500.0);
        route.observe(s, false, true);
        let (r, v) = s.chaser.state_at(1.0);
        s.chaser = Orbit::from_state_vectors(MU, r, v + DVec3::new(0.0, 0.0, 1.0), 1.0);
        s.now = 1.0;
        route.observe(s, true, false);
        s.now = 1.1;
        route.observe(s, false, true);
        s.now = 2.0;
        assert!(route.observe(s, false, true));
        assert_eq!(route.history[0].outcome, Outcome::Partial);
        assert_eq!(route.revision, 2);
        assert!(!route.nodes.is_empty());
    }

    #[test]
    fn finite_manual_burns_reach_the_gate_with_automatic_corrections() {
        // Independent RK4 powered integration, rather than applying the planner's impulsive orbit.
        fn step(s: &mut State, dt: f64, thrust: DVec3) {
            let (r, v) = s.chaser.state_at(s.now);
            let acceleration = |p: DVec3| p * (-MU / p.length().powi(3)) + thrust;
            let (k1r, k1v) = (v, acceleration(r));
            let (k2r, k2v) = (v + k1v * (dt * 0.5), acceleration(r + k1r * (dt * 0.5)));
            let (k3r, k3v) = (v + k2v * (dt * 0.5), acceleration(r + k2r * (dt * 0.5)));
            let (k4r, k4v) = (v + k3v * dt, acceleration(r + k3r * dt));
            s.now += dt;
            s.chaser = Orbit::from_state_vectors(
                MU,
                r + (k1r + k2r * 2.0 + k3r * 2.0 + k4r) * (dt / 6.0),
                v + (k1v + k2v * 2.0 + k3v * 2.0 + k4v) * (dt / 6.0),
                s.now,
            );
            s.budget -= thrust.length() * dt;
        }
        let mut s = state();
        let mut route = Route::new(s, 600.0);
        for _ in 0..8 {
            if route.handoff {
                break;
            }
            let n = route
                .nodes
                .first()
                .expect("feasible remaining plan")
                .clone();
            s.now = s.now.max(n.ut - n.delta_v.length() / s.acceleration * 0.5);
            route.observe(s, false, true);
            for _ in 0..20_000 {
                if route.remaining.length() < 0.1 {
                    break;
                }
                let thrust = route.remaining.normalized() * s.acceleration;
                step(&mut s, 0.02, thrust);
                assert!(!route.observe(s, true, false), "never replan during a burn");
            }
            assert!(route.remaining.length() < 0.1, "burn must converge");
            let revision = route.revision;
            for _ in 0..20 {
                step(&mut s, 0.1, DVec3::ZERO);
                route.observe(s, false, true);
                if route.revision > revision {
                    break;
                }
            }
            assert!(route.revision > revision);
        }
        assert!(route.handoff, "finite burns must converge to port approach");
        assert!(s.budget > 0.0);
        assert!(route.history.len() >= 2);
        assert!((s.chaser.state_at(s.now).1 - s.target.state_at(s.now).1).length() < 2.0);
    }
}
