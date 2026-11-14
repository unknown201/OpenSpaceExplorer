//! Bounds-based part contacts, displacement buoyancy and provisional spring/damper suspension.
//! This is a sampled height-field solver; imported collider meshes/materials remain future work.
use crate::{
    bodies::Body,
    flight::{self, Powered, Propulsion, StepOutcome},
    rigid::PartForce,
    structure::Structure,
};
use ose_math::DVec3;

fn world(frame: (DVec3, DVec3, DVec3), v: DVec3) -> DVec3 {
    frame.0 * v.x + frame.1 * v.y + frame.2 * v.z
}
fn local(frame: (DVec3, DVec3, DVec3), v: DVec3) -> DVec3 {
    DVec3::new(v.dot(frame.0), v.dot(frame.1), v.dot(frame.2))
}
fn corners(h: DVec3) -> [DVec3; 8] {
    std::array::from_fn(|i| {
        DVec3::new(
            if i & 1 == 0 { -h.x } else { h.x },
            if i & 2 == 0 { -h.y } else { h.y },
            if i & 4 == 0 { -h.z } else { h.z },
        )
    })
}

/// Exact volume and centroid of a box clipped by `normal.dot(x) <= depth`.
pub fn submerged_box(half: DVec3, normal: DVec3, depth: f64) -> (f64, DVec3) {
    let extent = half.x * normal.x.abs() + half.y * normal.y.abs() + half.z * normal.z.abs();
    if depth >= extent {
        return (8.0 * half.x * half.y * half.z, DVec3::ZERO);
    }
    if depth <= -extent {
        return (0.0, DVec3::ZERO);
    }
    let c = corners(half);
    let mut faces = Vec::new();
    let mut cut: Vec<DVec3> = Vec::new();
    for ids in [
        [0, 2, 6, 4],
        [1, 5, 7, 3],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 6, 7, 5],
    ] {
        let mut polygon = Vec::new();
        for j in 0..4 {
            let a = c[ids[j]];
            let b = c[ids[(j + 1) % 4]];
            let da = normal.dot(a) - depth;
            let db = normal.dot(b) - depth;
            if da <= 0.0 {
                polygon.push(a);
            }
            if da.abs() < 1e-10 && !cut.iter().any(|q| (*q - a).length_squared() < 1e-20) {
                cut.push(a);
            }
            if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
                let p = a + (b - a) * (da / (da - db));
                polygon.push(p);
                if !cut.iter().any(|q| (*q - p).length_squared() < 1e-20) {
                    cut.push(p);
                }
            }
        }
        if polygon.len() >= 3 {
            faces.push(polygon);
        }
    }
    if cut.len() >= 3 {
        let centre = cut.iter().fold(DVec3::ZERO, |s, p| s + *p) * (1.0 / cut.len() as f64);
        let a = (cut[0] - centre).normalized();
        let b = normal.cross(a);
        cut.sort_by(|p, q| {
            let p = *p - centre;
            let q = *q - centre;
            p.dot(b)
                .atan2(p.dot(a))
                .total_cmp(&q.dot(b).atan2(q.dot(a)))
        });
        faces.push(cut);
    }
    let count: usize = faces.iter().map(Vec::len).sum();
    if count == 0 {
        return (0.0, DVec3::ZERO);
    }
    let inside = faces.iter().flatten().fold(DVec3::ZERO, |s, p| s + *p) * (1.0 / count as f64);
    let (mut volume, mut moment) = (0.0, DVec3::ZERO);
    for f in faces {
        for j in 1..f.len() - 1 {
            let v = ((f[0] - inside).dot((f[j] - inside).cross(f[j + 1] - inside))).abs() / 6.0;
            volume += v;
            moment = moment + (inside + f[0] + f[j] + f[j + 1]) * (v / 4.0);
        }
    }
    (
        volume,
        if volume > 1e-15 {
            moment * (1.0 / volume)
        } else {
            DVec3::ZERO
        },
    )
}

/// Terrain includes flattened height-map sites but excludes the sea surface.
pub fn solid_radius(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> f64 {
    let (lat, lon) = flight::lat_lon(body, r, ut);
    if let Some(radius) = body
        .site
        .as_ref()
        .and_then(|s| s.ground_radius(body.radius_m, crate::campus::direction(lat, lon)))
    {
        return radius;
    }
    body.radius_m + terrain.map(|t| t.height_m(lat, lon)).unwrap_or(0.0)
}
fn normal(
    body: &Body,
    terrain: Option<&ose_data::terrain::TerrainMap>,
    r: DVec3,
    ut: f64,
) -> DVec3 {
    let (up, east, north) = flight::surface_frame(r);
    let slope =
        |d| (solid_radius(body, terrain, r + d, ut) - solid_radius(body, terrain, r - d, ut)) * 0.5;
    (up - east * slope(east) - north * slope(north)).normalized()
}
fn spin(body: &Body) -> DVec3 {
    DVec3::new(
        0.0,
        0.0,
        if body.rotation_period_s > 0.0 {
            std::f64::consts::TAU / body.rotation_period_s
        } else {
            0.0
        },
    )
}

/// Effective inverse mass at an offset in a specified impulse direction, craft frame.
pub fn inverse_effective_mass(
    mass: crate::dynamics::MassProperties,
    r: DVec3,
    direction: DVec3,
) -> f64 {
    1.0 / mass.mass_t + direction.dot(mass.inertia.solve(r.cross(direction)).cross(r))
}

/// Landing legs' VehiclePhysics friction (landingleg_*v): `FrictionLimit` and `FrictionAdherent`.
pub const LEG_FRICTION_LIMIT: f64 = 1.0;
pub const LEG_FRICTION_ADHERENT_MPS: f64 = 0.1;

/// Implicit spring/damper force. Travel and supported mass set a provisional 2g-at-full-travel spring.
pub fn suspension_force(
    compression: f64,
    speed: f64,
    travel: f64,
    mass: f64,
    gravity: f64,
    dt: f64,
) -> f64 {
    if compression <= 0.0 || travel <= 0.0 || mass <= 0.0 {
        return 0.0;
    }
    let k = 2.0 * mass * gravity.max(0.1) / travel;
    let c = 2.0 * (k * mass).sqrt();
    ((k * compression.min(travel) - c * speed) / (1.0 + c * dt / mass + k * dt * dt / mass))
        .max(0.0)
}

impl Structure {
    /// Prepare water and suspension forces before the common flight integrator.
    pub fn configure_surface(
        &mut self,
        p: &mut Powered,
        body: &Body,
        terrain: Option<&ose_data::terrain::TerrainMap>,
        ut: f64,
        dt: f64,
        propulsion: &mut Propulsion,
    ) {
        self.surface_loads.clear();
        self.water_strikes.clear();
        self.jets.clear();
        self.suspension_compression_m.fill(0.0);
        propulsion.per_part_contacts = true;
        p.splashed = false;
        p.ground_contact = false;
        if p.crashed || p.ground_anchored {
            return;
        }
        let mass = propulsion.mass;
        let frame = flight::vessel_basis(p);
        // engine exhaust blasting the craft's own parts: heat (taken up by the next heat step) and a push
        for push in self.exhaust_blast(p.throttle) {
            let r = self.rigid.rest_position(push.part) + push.offset_m - mass.centre_m;
            propulsion.force_kn = propulsion.force_kn + push.force_kn;
            propulsion.torque_knm = propulsion.torque_knm + r.cross(push.force_kn);
            self.surface_loads.push(PartForce {
                force_kn: world(frame, push.force_kn),
                ..push
            });
        }
        let up = p.r.normalized();
        let gravity = body.mu / p.r.length_squared();
        let omega = p.angvel + p.facing * p.roll_rate;
        let gear_count = (0..self.rigid.len())
            .filter(|&i| {
                self.rigid.present[i]
                    && self.gear_out[i]
                    && self.gear_defs[i]
                        .as_ref()
                        .is_some_and(|g| g.suspension_m > 0.0)
            })
            .count()
            .max(1);
        for i in (0..self.rigid.len()).filter(|&i| self.rigid.present[i]) {
            let (centre, half) = self.part_box(i);
            let offset = centre - mass.centre_m;
            let at = p.r + world(frame, offset);
            let applied_water = std::mem::take(&mut self.water_pending[i]);
            let was_wet = std::mem::replace(&mut self.in_water[i], false);
            if let Some(ocean) = &body.ocean {
                let water = body.radius_m;
                // Land and built facilities displace the water, too.
                if solid_radius(body, terrain, at, ut) < water - 1e-5 {
                    // how deep each corner of the part's bounds is (`CollectPartBoundsAndDepths`)
                    let origin = self.rigid.rest_position(i);
                    let rotate =
                        |v| crate::rigid::to_vessel(crate::aero::rotate(self.part_rot[i], v));
                    let water_corners = self.water_bounds[i].map(rotate);
                    let depths = water_corners
                        .map(|c| water - (p.r + world(frame, origin - mass.centre_m + c)).length());
                    let min_depth = depths.iter().copied().fold(f64::INFINITY, f64::min);
                    let max_depth = depths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if max_depth > 0.0 {
                        p.landed = false;
                        p.splashed = true;
                        self.in_water[i] = true;
                        let ws = crate::water::settings();
                        // the vessel's own surface motion, as the original takes it for every part
                        let v_srf = p.v - spin(body).cross(p.r);
                        let speed = v_srf.length();
                        let crash_axis = propulsion
                            .water_reference
                            .map(|f| f.axis(self.buoyancy_center_m[i]))
                            .unwrap_or(up);
                        let down = if speed > 0.0 {
                            v_srf.dot(-crash_axis) / speed
                        } else {
                            1.0
                        };
                        let dot_abs = if speed > 0.0 { down.abs() } else { 1.0 };
                        // Striking the water: the part breaks if the vessel comes down (or a tenth of its
                        // speed) faster than 1.2 times the part's crash tolerance.
                        let tolerance = if self.crash_tolerance[i] > 0.0 {
                            self.crash_tolerance[i]
                        } else {
                            flight::CRASH_SPEED_MPS
                        };
                        if !was_wet && crate::water::breaks_on_entry(speed, down * speed, tolerance)
                        {
                            self.water_strikes
                                .push((i, (ws.min_crash_mult * speed).max(down * speed)));
                        }
                        let density = ocean.density.max(0.0);
                        let submerged = crate::water::submerged_share(min_depth, max_depth);
                        let displaced_t = self.displacement_m3[i]
                            * density
                            * ws.buoyancy_scalar
                            * self.buoyancy[i]
                            * submerged;
                        let buoyancy = up * (displaced_t * gravity);
                        let at_part = crate::water::force_point(
                            water_corners,
                            depths,
                            rotate(self.buoyancy_center_m[i]),
                        );
                        // the water slows each part as a damping of its own motion
                        let part_mass = self.rigid.masses_t[i].max(1e-6);
                        let horizontal = (v_srf - up * v_srf.dot(up)).length();
                        let steering = p.rotation_input.length_squared();
                        let entering = !was_wet;
                        let dir = if speed > 0.0 {
                            local(frame, v_srf) * (1.0 / speed)
                        } else {
                            DVec3::ZERO
                        };
                        let mach = speed / 340.0;
                        let damping = {
                            let me = &*self;
                            let mut state = me.water_drag[i];
                            let damping = state.step(
                                speed,
                                horizontal,
                                dot_abs,
                                entering,
                                steering,
                                || me.area_drag_along(i, dir, mach),
                                submerged,
                                part_mass,
                                density,
                                dt,
                            );
                            self.water_drag[i] = state;
                            damping
                        };
                        self.water_pending[i] = crate::water::PendingWater {
                            force_local: local(frame, buoyancy),
                            point: at_part,
                            damping,
                        };
                    }
                }
            }
            let buoyancy = world(frame, applied_water.force_local);
            let origin = self.rigid.rest_position(i);
            let arm = world(frame, origin + applied_water.point - mass.centre_m);
            let part_centre = self.rigid.rest_mass_position(i);
            let part_arm = world(frame, part_centre - mass.centre_m);
            let v_part = p.v + omega.cross(part_arm) - spin(body).cross(p.r + part_arm);
            let part_mass = self.rigid.masses_t[i].max(1e-6);
            let c = applied_water.damping.linear;
            let drag = crate::water::damping_force(
                v_part,
                buoyancy - up * (part_mass * gravity),
                part_mass,
                c,
                dt,
            );
            let mut torque = arm.cross(buoyancy) + part_arm.cross(drag);
            if let Some(ca) = applied_water.damping.angular {
                let own = self.part_own_inertia(i);
                torque = torque
                    + world(frame, own.apply(local(frame, omega - spin(body))))
                        * (-ca / (1. + ca * dt));
            }
            propulsion.environment_force_kn = propulsion.environment_force_kn + buoyancy + drag;
            propulsion.environment_torque_knm = propulsion.environment_torque_knm + torque;
            if buoyancy.length_squared() > 0. {
                self.surface_loads.push(PartForce {
                    part: i,
                    force_kn: buoyancy,
                    offset_m: applied_water.point,
                });
            }
            if drag.length_squared() > 0. {
                self.surface_loads.push(PartForce {
                    part: i,
                    force_kn: drag,
                    offset_m: part_centre - origin,
                });
            }
            if self.gear_out[i] {
                if let Some(gear) = &self.gear_defs[i] {
                    let n = normal(body, terrain, at, ut);
                    let nl = local(frame, n);
                    // the lowest point of the gear's box along the ground normal; an axis lying flat to the
                    // ground keeps the foot centred on it (`signum` of 0.0 is 1.0, which put a level leg's
                    // foot on a corner, off the craft's axis, and let foot friction rock it)
                    let lowest = |n: f64| if n.abs() < 1e-6 { 0.0 } else { n.signum() };
                    let bottom = centre
                        + DVec3::new(
                            -half.x * lowest(nl.x),
                            -half.y * lowest(nl.y),
                            -half.z * lowest(nl.z),
                        );
                    let arm = world(frame, bottom - mass.centre_m);
                    let foot = p.r + arm;
                    let compression =
                        (solid_radius(body, terrain, foot, ut) - foot.length()).max(0.0);
                    self.suspension_compression_m[i] = compression.min(gear.suspension_m);
                    let speed = (p.v + omega.cross(arm) - spin(body).cross(foot)).dot(n);
                    let force = n * suspension_force(
                        compression,
                        speed,
                        gear.suspension_m,
                        mass.mass_t / gear_count as f64,
                        gravity,
                        dt,
                    );
                    // the foot grips the ground: friction against its sliding, up to `FrictionLimit` times
                    // the load, rising from nothing over the first `FrictionAdherent` m/s of slide
                    // (VehiclePhysics' friction curve on the original's legs), never more than stops
                    // the foot's share of the craft this tick
                    let load = force.length();
                    let foot_v = p.v + omega.cross(arm) - spin(body).cross(foot);
                    let slide = foot_v - n * foot_v.dot(n);
                    let slide_mps = slide.length();
                    // only a leg's foot: a wheel rolls, its grip is the tyre's (`wheels`), and a wheel
                    // that scrubbed here held a plane on the runway with its full load (T41)
                    let grip = if gear.leg && load > 0.0 && slide_mps > 1e-9 {
                        let along = local(frame, slide * (1.0 / slide_mps));
                        let inverse = inverse_effective_mass(mass, local(frame, arm), along);
                        let stop = slide_mps / (dt * inverse * gear_count as f64);
                        let mu =
                            LEG_FRICTION_LIMIT * (slide_mps / LEG_FRICTION_ADHERENT_MPS).min(1.0);
                        slide * (-(mu * load).min(stop) / slide_mps)
                    } else {
                        DVec3::ZERO
                    };
                    if compression > 0.0 {
                        p.ground_contact = true;
                    }
                    let force = force + grip;
                    propulsion.environment_force_kn = propulsion.environment_force_kn + force;
                    propulsion.environment_torque_knm =
                        propulsion.environment_torque_knm + arm.cross(force);
                    if force.length_squared() > 0.0 {
                        self.surface_loads.push(PartForce {
                            part: i,
                            force_kn: force,
                            offset_m: bottom - self.rigid.rest_position(i),
                        });
                    }
                }
            }
        }
    }

    /// Sweep each part's box corners, then resolve accumulated normal/friction impulses at the contact.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_surface(
        &mut self,
        before: &Powered,
        p: &mut Powered,
        body: &Body,
        terrain: Option<&ose_data::terrain::TerrainMap>,
        ut: f64,
        dt: f64,
        out: &mut StepOutcome,
    ) {
        if p.crashed || p.ground_anchored || dt <= 0.0 {
            return;
        }
        // another craft's hull under this one is solid too (`Universe::crafts_find_decks`)
        let deck = before
            .deck
            .filter(|_| !before.walker)
            .map(|d| d.radius_m)
            .unwrap_or(f64::NEG_INFINITY);
        let deck_mass = before.deck.map(|d| d.part_mass_t).unwrap_or(0.0);
        let on_deck = |r: DVec3, t: f64| deck > solid_radius(body, terrain, r, t);
        let support = |r: DVec3, t: f64| solid_radius(body, terrain, r, t).max(deck);
        for (i, speed) in std::mem::take(&mut self.water_strikes) {
            out.crashed = true;
            out.impact_mps = out.impact_mps.max(speed);
            if !out.contact_damage.contains(&i) {
                out.contact_damage.push(i);
            }
        }
        if before.landed {
            // The rolling/parked proxy changes pitch on the runway. Seat its current rotated bounds
            // on the support plane before the first airborne sweep, so rotating the nose does not
            // create a fictitious high-speed tail strike from an overlapping initial pose.
            let f = flight::vessel_basis(p);
            let up = p.r.normalized();
            let centre = self.mass_properties().centre_m;
            let lowest = (0..self.rigid.len())
                .filter(|&i| self.rigid.present[i])
                .map(|i| {
                    let (c, h) = self.part_box(i);
                    world(f, c - centre).dot(up)
                        - h.x * f.0.dot(up).abs()
                        - h.y * f.1.dot(up).abs()
                        - h.z * f.2.dot(up).abs()
                })
                .fold(f64::INFINITY, f64::min);
            let radius = support(p.r, ut + dt) - lowest;
            if p.r.length() < radius {
                p.r = up * radius;
            }
            return;
        }
        let mass = self.mass_properties();
        let old_frame = flight::vessel_basis(before);
        let mut frame = flight::vessel_basis(p);
        let mut hit = 1.0_f64;
        let mut contacts = Vec::new();
        let mut crossed = false;
        let ceiling = (body.radius_m + terrain.map(|t| t.max_m.max(0.0)).unwrap_or(0.0)).max(
            body.site
                .as_ref()
                .map(|s| s.surface_radius_bound(body.radius_m))
                .unwrap_or(body.radius_m),
        );
        for i in (0..self.rigid.len()).filter(|&i| self.rigid.present[i]) {
            let (centre, half) = self.part_box(i);
            let travel = if self.gear_out[i] {
                self.gear_defs[i]
                    .as_ref()
                    .map(|g| g.suspension_m)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            for corner in corners(half) {
                let offset = centre + corner - mass.centre_m;
                let from = before.r + world(old_frame, offset);
                let to = p.r + world(frame, offset);
                let distance = (to - from).length();
                if from.length().min(to.length()) > ceiling + distance {
                    continue;
                }
                let from_clearance = from.length() - support(from, ut) + travel;
                let to_clearance = to.length() - support(to, ut + dt) + travel;
                if from_clearance < 0.0 && to_clearance > from_clearance + 1e-8 {
                    continue;
                }
                // The hard stop lies one suspension travel below the extended foot.
                contacts.push((i, offset, travel));
                if let Some(t) = crate::contact::sweep(distance, |t| {
                    let r = from + (to - from) * t;
                    r.length() - support(r, ut + dt * t) + travel
                }) {
                    hit = hit.min(t);
                    crossed = true;
                }
            }
        }
        if !crossed {
            return;
        }
        p.r = before.r + (p.r - before.r) * hit;
        p.facing = (before.facing + (p.facing - before.facing) * hit).normalized();
        let top = old_frame.2 + (frame.2 - old_frame.2) * hit;
        flight::set_vessel_top(p, (top - p.facing * top.dot(p.facing)).normalized());
        frame = flight::vessel_basis(p);
        contacts.retain(|&(_, r, travel)| {
            let at = p.r + world(frame, r);
            at.length() - support(at, ut + hit * dt) + travel <= 0.01
        });
        if contacts.is_empty() {
            return;
        }
        let mut velocity = local(frame, p.v - spin(body).cross(p.r));
        let mut omega = local(frame, p.angvel + p.facing * p.roll_rate);
        let w_before_contact = p.angvel + p.facing * p.roll_rate;
        let mut impulses = vec![DVec3::ZERO; contacts.len()];
        let mut impact = 0.0_f64;
        let mut normal_impact = 0.0_f64;
        let mut deepest = 0.0_f64;
        // Accumulated impulses can be released in later iterations; a symmetric landing cannot
        // gain a spurious torque merely because one corner was visited first.
        let mut normals = Vec::new();
        let mut targets = Vec::new();
        for &(i, r, travel) in &contacts {
            let at = p.r + world(frame, r);
            let n = if on_deck(at, ut + hit * dt) {
                local(frame, at.normalized())
            } else {
                local(frame, normal(body, terrain, at, ut + hit * dt))
            };
            normals.push(n);
            let v = velocity + omega.cross(r);
            normal_impact = normal_impact.max((-v.dot(n)).max(0.0));
            let speed = if self.gear_out[i] && self.gear_defs[i].is_some() {
                (-v.dot(n)).max(0.0)
            } else {
                v.length()
            };
            impact = impact.max(speed);
            let tolerance = if self.crash_tolerance[i] > 0.0 {
                self.crash_tolerance[i]
            } else {
                flight::CRASH_SPEED_MPS
            };
            // on another craft's hull a part breaks as part on part (KSP2 `IsSurvivableCrash`)
            let breaks = if on_deck(at, ut + hit * dt) {
                crate::collide::crash_breaks(speed, deck_mass, tolerance)
            } else {
                speed > tolerance
            };
            if breaks {
                out.crashed = true;
                if !out.contact_damage.contains(&i) {
                    out.contact_damage.push(i);
                }
            }
            // A part struck well over its tolerance breaks without slowing the rest of the craft, which
            // strikes with its next layer as it reaches the ground; one struck barely over it breaks and
            // the craft stops on it (KSP2 crash drops, `flight::CRASH_PASS_THROUGH_MPS`).
            targets.push(
                if speed > tolerance && speed - tolerance > flight::CRASH_PASS_THROUGH_MPS {
                    v.dot(n).min(0.0)
                } else {
                    0.0
                },
            );
            if speed <= tolerance {
                deepest = deepest.max(support(at, ut + hit * dt) - at.length() - travel);
            }
        }
        for _ in 0..24 {
            for (j, &(_, r, _)) in contacts.iter().enumerate() {
                let n = normals[j];
                let v = velocity + omega.cross(r);
                let old_n = impulses[j].dot(n);
                let new_n =
                    (old_n + (targets[j] - v.dot(n)) / inverse_effective_mass(mass, r, n)).max(0.0);
                let tangent = v - n * v.dot(n);
                let mut friction = impulses[j] - n * old_n;
                if tangent.length() > 1e-10 {
                    let d = tangent.normalized();
                    friction =
                        friction - d * (tangent.length() / inverse_effective_mass(mass, r, d));
                }
                let cap = crate::contact::SLIDING_FRICTION * new_n;
                if friction.length() > cap {
                    friction = friction * (cap / friction.length());
                }
                let next = n * new_n + friction;
                let delta = next - impulses[j];
                velocity = velocity + delta * (1.0 / mass.mass_t);
                omega = omega + mass.inertia.solve(r.cross(delta));
                impulses[j] = next;
            }
        }
        // Correct initial overlap, including a bottomed-out suspension. Keep attitude and angular impulse.
        p.r = p.r + p.r.normalized() * deepest.max(0.0);
        p.v = spin(body).cross(p.r) + world(frame, velocity);
        let w = world(frame, omega);
        p.roll_rate = w.dot(p.facing);
        p.angvel = w - p.facing * p.roll_rate;
        out.angvel = w;
        out.angular_accel = out.angular_accel + (w - w_before_contact) * (1.0 / dt);
        out.normal_impact_mps = normal_impact;
        out.impact_mps = impact;
        out.touched_down = true;
        for (&(part, r, _), impulse) in contacts.iter().zip(impulses) {
            if impulse.length_squared() > 1e-16 {
                self.surface_loads.push(PartForce {
                    part,
                    force_kn: world(frame, impulse) * (1.0 / dt),
                    offset_m: r + mass.centre_m - self.rigid.rest_position(part),
                });
            }
        }
        let remaining = dt * (1.0 - hit);
        let top = flight::vessel_top(p);
        p.r = p.r + p.v * remaining;
        p.facing = flight::rotate(p.facing, w, remaining).normalized();
        p.roll_rate = w.dot(p.facing);
        p.angvel = w - p.facing * p.roll_rate;
        flight::set_vessel_top(p, flight::rotate(top, w, remaining));
        let end_frame = flight::vessel_basis(p);
        let penetration = (0..self.rigid.len())
            .filter(|&i| self.rigid.present[i] && !out.contact_damage.contains(&i))
            .flat_map(|i| {
                let (c, h) = self.part_box(i);
                let travel = if self.gear_out[i] {
                    self.gear_defs[i]
                        .as_ref()
                        .map(|g| g.suspension_m)
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                corners(h).map(|q| (c + q - mass.centre_m, travel))
            })
            .map(|(r, travel)| {
                let at = p.r + world(end_frame, r);
                support(at, ut + dt) - at.length() - travel
            })
            .fold(0.0, f64::max);
        p.r = p.r + p.r.normalized() * penetration;
        let up = p.r.normalized();
        let aligned = if p.level {
            p.facing.dot(up).abs() < 0.2
        } else {
            p.facing.dot(up) > 0.98
        };
        let lean = if p.level {
            p.facing.dot(up).abs().asin()
        } else {
            p.facing.dot(up).clamp(-1.0, 1.0).acos()
        };
        p.tipped = lean.tan().abs() > p.footprint_m / p.ground_clearance_m.max(0.1);
        p.landed = out.contact_damage.is_empty()
            && (aligned || p.tipped)
            && omega.length() < 0.1
            && velocity.dot(local(frame, up)).abs() < 0.1;
        if p.landed {
            p.angvel = DVec3::ZERO;
            p.roll_rate = 0.0;
        }
        if p.landed && p.level {
            p.drive.heading_rad = flight::heading_of(p.r, p.facing);
            p.drive.speed_mps = (p.v - spin(body).cross(p.r)).dot(p.facing);
        }
        p.crashed = out.crashed;
        if p.crashed {
            p.throttle = 0.0;
        }
        if out.touched_down {
            p.coasting_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(two_parts: bool) -> (Structure, Powered, Body) {
        let (mut craft, mut defs, res) = crate::structure::test_capsule(false);
        craft.parts.truncate(1);
        craft.parts[0].local_position = DVec3::ZERO;
        craft.parts[0].resources.clear();
        craft.parts[0].local_rotation = ose_data::craft::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        craft.stages.clear();
        let def = defs
            .iter_mut()
            .find(|d| d.id == craft.parts[0].part_id)
            .unwrap();
        def.mass_t = 2.0;
        def.mass_offset_m = DVec3::ZERO;
        def.flight_geometry = Default::default();
        def.resource_containers.clear();
        def.engines.clear();
        def.electric = Default::default();
        def.reaction_wheel_torque_knm = 0.0;
        def.reaction_wheel_axes_knm = None;
        def.crash_tolerance = 1000.0;
        def.bounds = Some(ose_data::PartBounds {
            center_m: DVec3::ZERO,
            size_m: DVec3::new(2.0, 2.0, 2.0),
        });
        def.drag_cubes[0].size = DVec3::new(2.0, 2.0, 2.0);
        def.drag_cubes[0].center = DVec3::ZERO;
        // a solid box: it displaces its whole 8 m^3
        def.drag_cubes[0].area = [4.0; 6];
        def.buoyancy = 1.0;
        def.buoyancy_center_m = DVec3::ZERO;
        if two_parts {
            let mut child = craft.parts[0].clone();
            child.parent = Some(0);
            child.local_position = DVec3::new(0.0, -2.0, 0.0);
            child.own_node = None;
            child.parent_node = None;
            craft.parts.push(child);
        }
        let mut st = Structure::build(&craft, &defs, &res);
        let radius = 1_000_000.0;
        let body = Body {
            id: "Test".into(),
            mu: 10.0 * radius * radius,
            mass_kg: 0.0,
            radius_m: radius,
            soi_radius_m: None,
            rotation_period_s: 0.0,
            initial_rotation_deg: 0.0,
            is_star: false,
            site: None,
            ocean: None,
        };
        let mut p = Powered::new(
            flight::FlightSpec {
                stages: vec![flight::StageSpec::default()],
                drag_area_m2: 0.0,
                drag_cd: 0.0,
            },
            DVec3::new(0.0, radius + 5.0, 0.0),
            DVec3::ZERO,
            false,
        );
        p.facing = DVec3::new(0.0, 1.0, 0.0);
        p.attitude = flight::Attitude::Free;
        p.controlled = false;
        st.sync(&mut p);
        (st, p, body)
    }

    fn step(
        st: &mut Structure,
        p: &mut Powered,
        body: &Body,
        terrain: Option<&ose_data::terrain::TerrainMap>,
        dt: f64,
    ) -> StepOutcome {
        let env = flight::EngineEnvironment::default();
        let mut engine = st.propulsion(p, env, dt);
        st.configure_actuators(p, env, &mut engine);
        st.configure_surface(p, body, terrain, 0.0, dt, &mut engine);
        let before = p.clone();
        let mut out =
            flight::integrate_with_propulsion(p, body, None, terrain, 0.0, dt, Some(engine));
        st.resolve_surface(&before, p, body, terrain, 0.0, dt, &mut out);
        out
    }

    #[test]
    fn symmetric_contacts_stop_without_spin_and_load_the_struck_parts_joint() {
        let (mut st, mut p, body) = fixture(true);
        p.r.y = body.radius_m + 2.01;
        p.v = DVec3::new(0.0, -2.0, 0.0);
        let out = step(&mut st, &mut p, &body, None, 0.02);
        assert!(out.touched_down && !out.crashed);
        assert!(
            p.angvel.length() < 0.001 && p.roll_rate.abs() < 0.001,
            "{p:?}"
        );
        assert!(p.v.length() < 0.001);
        assert!(st.surface_loads.iter().all(|f| f.part == 1));
        let impulse = st
            .surface_loads
            .iter()
            .fold(DVec3::ZERO, |s, f| s + f.force_kn)
            * 0.02;
        assert!((impulse.y - 4.0 * 2.2).abs() < 0.001);
        st.apply(&p, &out, 0.02);
        assert!(
            st.rigid.links.iter().any(|l| l.force_kn > 10.0),
            "ground reactions must reach joints even on a landed craft"
        );
        assert!(st.surface_loads.is_empty(), "an impulse is consumed once");
    }

    #[test]
    fn an_edge_impact_turns_the_hull_and_dissipates_energy() {
        let (mut st, mut p, body) = fixture(false);
        let angle = 0.3_f64;
        p.facing = DVec3::new(angle.sin(), angle.cos(), 0.0);
        p.r.y = body.radius_m + angle.sin() + angle.cos() + 0.01;
        p.v = DVec3::new(0.0, -3.0, 0.0);
        let before = p.clone();
        let out = step(&mut st, &mut p, &body, None, 0.02);
        assert!(out.touched_down && !out.crashed);
        assert!(
            p.angvel.length() > 0.2,
            "off-centre reaction must turn the hull"
        );
        let mass = st.mass_properties();
        let f = flight::vessel_basis(&p);
        let w = local(f, p.angvel + p.facing * p.roll_rate);
        let energy = 0.5 * p.mass_t * p.v.length_squared() + 0.5 * w.dot(mass.inertia.apply(w));
        let incoming = 0.5 * p.mass_t * (before.v + DVec3::new(0.0, -0.2, 0.0)).length_squared();
        assert!(
            energy < incoming,
            "inelastic frictional impact cannot add kinetic energy: {energy} > {incoming}"
        );
        assert!(
            !p.landed,
            "a spinning hull continues rotating after first contact"
        );
    }

    #[test]
    fn bounds_sweeps_catch_fast_falls_and_ridges_between_clear_endpoints() {
        let (mut st, mut p, mut body) = fixture(false);
        body.mu = 0.0;
        p.r.y = body.radius_m + 20.0;
        p.v = DVec3::new(0.0, -500.0, 0.0);
        let out = step(&mut st, &mut p, &body, None, 0.1);
        assert!(out.touched_down && !out.crashed);
        assert!(p.r.length() > body.radius_m + 0.99 && p.v.length() < 0.01);
        let terrain = ose_data::terrain::TerrainMap {
            width: 2,
            height: 2,
            scale_m: 0.0,
            offset_m: 0.0,
            has_ocean: false,
            body_radius_m: body.radius_m,
            data: vec![0; 4],
            max_m: 10.0,
            detail: None,
            overlay: None,
            flats: vec![ose_data::terrain::Flat {
                lat_deg: 0.0,
                lon_deg: 90.0,
                radius_m: 2.0,
                falloff_m: 1.0,
                altitude_m: 10.0,
            }],
        };
        let (mut st, mut p, _) = fixture(false);
        p.r = DVec3::new(-10.0, body.radius_m + 2.0, 0.0);
        p.v = DVec3::new(100.0, 0.0, 0.0);
        assert!(solid_radius(&body, Some(&terrain), p.r, 0.0) < p.r.length() - 1.0);
        assert!(
            solid_radius(&body, Some(&terrain), p.r + p.v * 0.2, 0.2)
                < (p.r + p.v * 0.2).length() - 1.0
        );
        let out = step(&mut st, &mut p, &body, Some(&terrain), 0.2);
        assert!(
            out.touched_down,
            "a raised feature between clear endpoints must be detected"
        );
        assert!(!st.surface_loads.is_empty());
    }

    #[test]
    fn impact_damage_removes_the_struck_part_and_retains_residual_motion() {
        let (mut st, mut p, body) = fixture(true);
        st.crash_tolerance[1] = 1.0;
        p.r.y = body.radius_m + 2.01;
        p.v = DVec3::new(0.0, -5.0, 0.0);
        let out = step(&mut st, &mut p, &body, None, 0.02);
        assert_eq!(out.contact_damage, vec![1]);
        assert!(
            p.v.y < -4.0,
            "breaking a weak foot cannot erase the surviving hull's impact"
        );
        // where the surviving part is before the struck one goes
        let f = flight::vessel_basis(&p);
        let survivor = |st: &Structure, p: &Powered| {
            p.r + world(f, st.part_box(0).0 - st.mass_properties().centre_m)
        };
        let before = survivor(&st, &p);
        let centre = st.mass_properties().centre_m;
        assert!(st.contact_damage(&out.contact_damage, &p).is_some());
        assert!(st.rigid.present[0] && !st.rigid.present[1]);
        assert!(st.net.present[0] && !st.net.present[1]);
        // the vessel's position follows the centre of mass up into the survivor; the survivor stays put
        let r = p.r;
        st.keep_in_place(&mut p, centre);
        assert!((survivor(&st, &p) - before).length() < 1e-9);
        assert!(
            ((p.r - r).dot(r.normalized()) - 1.0).abs() < 1e-9,
            "{:?}",
            p.r - r
        );
    }

    #[test]
    fn water_drag_acts_at_the_part_mass_centre_instead_of_the_bounds_centre() {
        let (mut craft, defs, res) = crate::structure::test_capsule(false);
        craft.parts.truncate(1);
        craft.stages.clear();
        craft.parts[0].local_position = DVec3::ZERO;
        craft.parts[0].local_rotation = ose_data::craft::Quat {
            w: 1.,
            ..Default::default()
        };
        let mut st = Structure::build(&craft, &defs, &res);
        let (_, mut p, mut body) = fixture(false);
        body.mu = 0.;
        body.ocean = Some(ose_data::Ocean {
            altitude_m: 0.,
            density: 1.,
        });
        let terrain = ose_data::terrain::TerrainMap {
            width: 2,
            height: 2,
            scale_m: 0.,
            offset_m: -100.,
            has_ocean: true,
            body_radius_m: body.radius_m,
            data: vec![0; 4],
            flats: vec![],
            max_m: -100.,
            detail: None,
            overlay: None,
        };
        st.sync(&mut p);
        let mass = st.mass_properties();
        p.r = DVec3::new(0., body.radius_m + mass.centre_m.y, 0.);
        p.v = DVec3::new(5., 0., 0.);
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, Some(&terrain), 0., 0.02, &mut engine);
        assert_eq!(
            engine.environment_force_kn,
            DVec3::ZERO,
            "water is prepared after the rigidbody update"
        );
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, Some(&terrain), 0.02, 0.02, &mut engine);
        assert!(
            engine.environment_force_kn.x < 0.,
            "water must slow the capsule"
        );
        assert!(
            engine.environment_torque_knm.length() < 1e-10,
            "single-part linear damping cannot add spin: {:?}",
            engine.environment_torque_knm
        );
        assert!(
            (st.part_box(0).0 - mass.centre_m).length() > 0.1,
            "fixture needs separated bounds and mass centres"
        );
    }

    #[test]
    fn buoyancy_float_depth_tracks_density_and_heavy_hulls_sink() {
        let (mut st, mut p, mut body) = fixture(false);
        body.ocean = Some(ose_data::Ocean {
            altitude_m: 4270.0,
            density: 1.0,
        });
        let terrain = ose_data::terrain::TerrainMap {
            width: 2,
            height: 2,
            scale_m: 0.0,
            offset_m: -100.0,
            has_ocean: true,
            body_radius_m: body.radius_m,
            data: vec![0; 4],
            flats: Vec::new(),
            max_m: -100.0,
            detail: None,
            overlay: None,
        };
        // A 2 t hull of 8 m^3, 2 m tall, half under: KSP2's buoyancy is 8 m^3 x 1.2 x the wet half of its
        // height, 4.8 t of water: 48 kN. It floats with 2 / 4.8 of a metre under.
        p.r.y = body.radius_m;
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, Some(&terrain), 0.0, 0.02, &mut engine);
        assert_eq!(
            engine.environment_force_kn,
            DVec3::ZERO,
            "first water update prepares the next rigidbody step"
        );
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, Some(&terrain), 0.02, 0.02, &mut engine);
        assert!(
            (engine.environment_force_kn.y - 46.88).abs() < 0.001,
            "{engine:?}"
        );
        assert!(
            (st.water_pending[0].force_local.y - 48.).abs() < 0.001,
            "raw buoyancy is 48 kN; force-updated damping reduces the net environmental force"
        );
        assert!(engine.environment_torque_knm.length() < 1e-8);
        for _ in 0..1500 {
            step(&mut st, &mut p, &body, Some(&terrain), 0.02);
        }
        let float = 1.0 - 2.0 / 4.8;
        assert!((p.r.y - body.radius_m - float).abs() < 0.02, "{p:?}");
        assert!(!p.landed && !p.crashed, "water is not solid ground");
        body.ocean.as_mut().unwrap().density = 0.1;
        for _ in 0..250 {
            step(&mut st, &mut p, &body, Some(&terrain), 0.02);
        }
        assert!(
            p.r.y < body.radius_m - 2.0 && p.v.y < 0.0,
            "an over-dense hull sinks"
        );
        assert!(!p.landed);
    }

    #[test]
    fn suspension_compresses_and_arrests_a_drop_before_the_hard_stop() {
        let (mut st, mut p, body) = fixture(false);
        st.gear_defs[0] = Some(ose_data::LandingGear {
            leg: true,
            suspension_m: 1.0,
            retractable: true,
            starts_extended: true,
            extended: None,
            radius_m: 0.0,
            motor: None,
            brake_torque_knm: 0.0,
            steerable: false,
            dynamics: Default::default(),
        });
        st.gear_out[0] = true;
        p.r.y = body.radius_m + 1.2;
        let mut compression = 0.0_f64;
        for _ in 0..500 {
            let out = step(&mut st, &mut p, &body, None, 0.02);
            assert!(!out.crashed);
            compression = compression.max(st.suspension_compression_m[0]);
        }
        assert!(
            compression > 0.2 && compression < 1.0,
            "spring uses its travel: {compression}"
        );
        assert!(p.v.y.abs() < 0.02, "spring/damper settles: {}", p.v.y);
        assert!(p.r.y > body.radius_m && p.r.y < body.radius_m + 0.8);
        st.gear_out[0] = false;
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, None, 0.0, 0.02, &mut engine);
        assert_eq!(
            engine.environment_force_kn,
            DVec3::ZERO,
            "stowed gear cannot support the hull"
        );
    }
    #[test]
    fn displaced_volume_and_centre_follow_the_waterline_and_bank() {
        let h = DVec3::new(1.0, 1.0, 1.0);
        let n = DVec3::new(0.0, 1.0, 0.0);
        assert_eq!(submerged_box(h, n, -2.0).0, 0.0);
        let full = submerged_box(h, n, 2.0);
        assert!((full.0 - 8.0).abs() < 1e-10 && full.1.length() < 1e-10);
        assert_eq!(submerged_box(h, n, 1.0), (8.0, DVec3::ZERO));
        assert_eq!(submerged_box(h, n, -1.0), (0.0, DVec3::ZERO));
        let half = submerged_box(h, n, 0.0);
        assert!((half.0 - 4.0).abs() < 1e-10);
        assert!((half.1 - DVec3::new(0.0, -0.5, 0.0)).length() < 1e-10);
        let bank = submerged_box(h, DVec3::new(1.0, 1.0, 0.0).normalized(), 0.0);
        assert!((bank.0 - 4.0).abs() < 1e-10);
        assert!((bank.1.x + 1.0 / 3.0).abs() < 1e-10 && (bank.1.y + 1.0 / 3.0).abs() < 1e-10);
    }
    #[test]
    fn off_centre_impulse_uses_rotational_effective_mass() {
        let mass = crate::dynamics::MassProperties {
            mass_t: 2.0,
            inertia: crate::dynamics::Inertia {
                x: DVec3::new(2.0, 0.0, 0.0),
                y: DVec3::new(0.0, 2.0, 0.0),
                z: DVec3::new(0.0, 0.0, 2.0),
            },
            ..Default::default()
        };
        assert!(
            (inverse_effective_mass(mass, DVec3::new(1.0, 0.0, 0.0), DVec3::new(0.0, 1.0, 0.0))
                - 1.0)
                .abs()
                < 1e-10
        );
    }
    #[test]
    fn suspension_damps_compression_and_cannot_pull_on_ground() {
        let spring = suspension_force(0.5, 0.0, 1.0, 2.0, 10.0, 0.02);
        assert!(spring > 0.0 && spring < 20.0);
        assert!(suspension_force(0.5, -2.0, 1.0, 2.0, 10.0, 0.02) > spring);
        assert_eq!(suspension_force(0.5, 100.0, 1.0, 2.0, 10.0, 0.02), 0.0);
        assert_eq!(suspension_force(0.0, -10.0, 1.0, 2.0, 10.0, 0.02), 0.0);
    }
}
