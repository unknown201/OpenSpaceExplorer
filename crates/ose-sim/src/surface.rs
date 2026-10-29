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
        self.suspension_compression_m.fill(0.0);
        propulsion.per_part_contacts = true;
        if p.crashed {
            return;
        }
        let mass = propulsion.mass;
        let frame = flight::vessel_basis(p);
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
            if let Some(ocean) = &body.ocean {
                let water = body.radius_m;
                // Land and built facilities displace the water, too.
                if solid_radius(body, terrain, at, ut) < water - 1e-5 {
                    let (volume, centroid) =
                        submerged_box(half, local(frame, up), water - at.length());
                    if volume > 0.0 {
                        p.landed = false;
                        let r = offset + centroid;
                        let arm = world(frame, r);
                        let v = p.v + omega.cross(arm) - spin(body).cross(p.r + arm);
                        let density = ocean.density.max(0.0);
                        let buoyancy = up * (density * volume * gravity);
                        // Share an implicit generalized-mass drag cap across the parts. This bounds
                        // both translation and spin damping when the water centroid is off-centre.
                        let area = volume.powf(2.0 / 3.0);
                        let rate = 0.5 * density * area * v.length();
                        let part_mass = self.net.mass_of(&[i]).max(1e-6);
                        let damping_inverse = if v.length_squared() > 1e-16 {
                            inverse_effective_mass(mass, r, local(frame, v).normalized())
                                * mass.mass_t
                                / part_mass
                        } else {
                            0.0
                        };
                        let drag = v * (-rate / (1.0 + rate * dt * damping_inverse));
                        let force = buoyancy + drag;
                        propulsion.environment_force_kn = propulsion.environment_force_kn + force;
                        propulsion.environment_torque_knm =
                            propulsion.environment_torque_knm + arm.cross(force);
                        self.surface_loads.push(PartForce {
                            part: i,
                            force_kn: force,
                            offset_m: centre + centroid - self.rigid.rest_position(i),
                        });
                    }
                }
            }
            if self.gear_out[i] {
                if let Some(gear) = &self.gear_defs[i] {
                    let n = normal(body, terrain, at, ut);
                    let nl = local(frame, n);
                    let bottom = centre
                        + DVec3::new(
                            -half.x * nl.x.signum(),
                            -half.y * nl.y.signum(),
                            -half.z * nl.z.signum(),
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
        if p.crashed || dt <= 0.0 {
            return;
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
            let radius = solid_radius(body, terrain, p.r, ut + dt) - lowest;
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
                let from_clearance = from.length() - solid_radius(body, terrain, from, ut) + travel;
                let to_clearance = to.length() - solid_radius(body, terrain, to, ut + dt) + travel;
                if from_clearance < 0.0 && to_clearance > from_clearance + 1e-8 {
                    continue;
                }
                // The hard stop lies one suspension travel below the extended foot.
                contacts.push((i, offset, travel));
                if let Some(t) = crate::contact::sweep(distance, |t| {
                    let r = from + (to - from) * t;
                    r.length() - solid_radius(body, terrain, r, ut + dt * t) + travel
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
            at.length() - solid_radius(body, terrain, at, ut + hit * dt) + travel <= 0.01
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
            let n = local(frame, normal(body, terrain, at, ut + hit * dt));
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
            if speed > tolerance {
                out.crashed = true;
                if !out.contact_damage.contains(&i) {
                    out.contact_damage.push(i);
                }
            }
            // A broken foot absorbs only its tolerance's share of normal impact energy;
            // it cannot stop the surviving hull before the next layer strikes the ground.
            targets.push(if speed > tolerance {
                -((v.dot(n).min(0.0)).powi(2) - tolerance * tolerance)
                    .max(0.0)
                    .sqrt()
            } else {
                0.0
            });
            if speed <= tolerance {
                deepest = deepest
                    .max(solid_radius(body, terrain, at, ut + hit * dt) - at.length() - travel);
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
                solid_radius(body, terrain, at, ut + dt) - at.length() - travel
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
        assert!(st.contact_damage(&out.contact_damage, &p).is_some());
        assert!(st.rigid.present[0] && !st.rigid.present[1]);
        assert!(st.net.present[0] && !st.net.present[1]);
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
        // A 2t, 8m³ hull displaces 2m³ at its analytical quarter-submerged equilibrium.
        p.r.y = body.radius_m + 0.5;
        let mut engine = st.propulsion(&p, Default::default(), 0.02);
        st.configure_surface(&mut p, &body, Some(&terrain), 0.0, 0.02, &mut engine);
        assert!((engine.environment_force_kn.y - 20.0).abs() < 0.001);
        assert!(engine.environment_torque_knm.length() < 1e-8);
        for _ in 0..1500 {
            step(&mut st, &mut p, &body, Some(&terrain), 0.02);
        }
        assert!((p.r.y - body.radius_m - 0.5).abs() < 0.02, "{p:?}");
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
