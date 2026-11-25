//! A craft that comes down on sloped ground rests on it as KSP2's colliders hold it: flush with the slope, with no
//! part under the ground (Josh's "the engine still sunk into the ground" on landing: the craft was held straight
//! up and stood on the ground under its centre, so the uphill rim of its engine bell sank into the hillside, 17 cm
//! on a 15 degree slope), and a craft too tall for the slope falls over.
use ose_math::DVec3;
use ose_sim::{bodies, flight, structure::Structure, surface, Universe, TICKS_PER_SECOND};

fn data() -> (
    Vec<ose_data::CelestialBody>,
    Vec<ose_data::PartDef>,
    Vec<ose_data::ResourceDef>,
) {
    (
        serde_json::from_str(include_str!("../../../assets/own/kerbol-system.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap(),
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap(),
    )
}

struct Landing {
    /// deepest any part's model (or box) went under the ground after the first second down, m
    sunk_m: f64,
    /// angle of the craft's axis from the local vertical at the end, degrees
    tilt_deg: f64,
    landed: bool,
    tipped: bool,
    /// how far it went along the ground from where it came down, m
    moved_m: f64,
    /// how far its top is turned about its axis from the local vertical at the end, degrees
    roll_deg: f64,
}

/// `json` (a parity-pilot drop craft) set down `drop_m` above ground sloping `slope_deg` to the east (a 1000 m
/// high ramp round the equator at 90 degrees east), left for 10 s: upright, or `lying` on its side with its axis
/// pointing north, along the slope's contour, and turned `roll` degrees about it.
fn land(json: &str, slope_deg: f64, drop_m: f64, lying: Option<f64>) -> Landing {
    let (bodies, defs, res) = data();
    let mut kerbin = bodies.into_iter().find(|b| b.id == "Kerbin").unwrap();
    kerbin.parent = None;
    kerbin.orbit = None;
    kerbin.soi_radius_m = None;
    let mut u = Universe::with_bodies(&[kerbin]);
    u.set_part_catalog(defs.clone(), res.clone());
    let body = u.body_entity("Kerbin").unwrap();
    let b = u.world.get::<bodies::Body>(body).unwrap().clone();
    let cell_m = std::f64::consts::TAU * b.radius_m / 3600.0;
    let rise_m = cell_m * slope_deg.to_radians().tan();
    let terrain = std::sync::Arc::new(ose_data::terrain::TerrainMap {
        width: 3600,
        height: 2,
        scale_m: rise_m * 65535.0,
        offset_m: 1000.0 - 2700.0 * rise_m,
        has_ocean: false,
        body_radius_m: b.radius_m,
        data: (0..7200).map(|i| (i % 3600) as u16).collect(),
        max_m: 1000.0 + 900.0 * rise_m,
        detail: None,
        overlay: None,
        flats: vec![],
    });
    u.world
        .entity_mut(body)
        .insert(flight::Terrain(terrain.clone()));
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = ose_importer::craft::craft_from_value(&v).unwrap();
    let report = ose_data::staging::delta_v_by_stage(&c, &defs, &res);
    let spec = flight::FlightSpec::from_report(&report, &[], &[], 1.5);
    let e = u.spawn_powered_on_surface(&c.name, body, spec, 0.0, 90.0);
    u.set_structure(e, Structure::build(&c, &defs, &res));
    u.set_craft(e, c.clone());
    u.step();
    let mut p = u.powered(e).unwrap().clone();
    let up = p.r.normalized();
    let ground = surface::solid_radius(&b, Some(&terrain), p.r, u.ut());
    p.r = up * (ground + p.ground_clearance_m + drop_m);
    p.facing = up;
    if let Some(roll) = lying {
        // on its side, its lowest model point `drop_m` over the ground
        let north = DVec3::new(0.0, 0.0, 1.0);
        let east = north.cross(up);
        p.facing = north;
        let roll = roll.to_radians();
        flight::set_vessel_top(&mut p, up * roll.cos() + east * roll.sin());
        let st = u.structure(e).unwrap();
        let f = flight::vessel_basis(&p);
        let centre = st.mass_properties().centre_m;
        let lowest = (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i])
            .flat_map(|i| st.ground_points(i))
            .map(|q| {
                let q = q - centre;
                (f.0 * q.x + f.1 * q.y + f.2 * q.z).dot(up)
            })
            .fold(f64::INFINITY, f64::min);
        p.r = up * (ground + 2.0 - lowest);
        let below = (0..st.rigid.len())
            .filter(|&i| st.rigid.present[i])
            .flat_map(|i| st.ground_points(i))
            .map(|q| {
                let q = q - centre;
                let at = p.r + f.0 * q.x + f.1 * q.y + f.2 * q.z;
                at.length() - surface::solid_radius(&b, Some(&terrain), at, u.ut())
            })
            .fold(f64::INFINITY, f64::min);
        p.r = p.r - up * (below - drop_m);
    }
    let (start, t0) = (p.r, u.ut());
    p.v = DVec3::new(0.0, 0.0, std::f64::consts::TAU / b.rotation_period_s).cross(p.r);
    p.landed = false;
    p.attitude = flight::Attitude::Free;
    p.angvel = DVec3::ZERO;
    u.set_powered(e, p);
    let mut sunk_m = 0.0_f64;
    for t in 0..(10 * TICKS_PER_SECOND) {
        u.step();
        if t < TICKS_PER_SECOND {
            continue;
        }
        let p = u.powered(e).unwrap();
        let st = u.structure(e).unwrap();
        let f = flight::vessel_basis(p);
        let centre = st.mass_properties().centre_m;
        for i in (0..st.rigid.len()).filter(|&i| st.rigid.present[i]) {
            for q in st.ground_points(i) {
                let q = q - centre;
                let at = p.r + f.0 * q.x + f.1 * q.y + f.2 * q.z;
                sunk_m =
                    sunk_m.max(surface::solid_radius(&b, Some(&terrain), at, u.ut()) - at.length());
            }
        }
    }
    let p = u.powered(e).unwrap();
    Landing {
        sunk_m,
        tilt_deg: p
            .facing
            .dot(p.r.normalized())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees(),
        landed: p.landed,
        tipped: p.tipped,
        roll_deg: {
            let up = p.r.normalized();
            let top = flight::vessel_top(p);
            let flat = up - p.facing * up.dot(p.facing);
            top.dot(flat.normalized())
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        },
        moved_m: {
            // where the ground under its start point turned to
            let a = std::f64::consts::TAU / b.rotation_period_s * (u.ut() - t0);
            let s = DVec3::new(
                start.x * a.cos() - start.y * a.sin(),
                start.x * a.sin() + start.y * a.cos(),
                start.z,
            );
            (p.r - s).length()
        },
    }
}

const PODTANK: &str = include_str!("../../../tools/ksp2-parity-pilot/craft-drop-podtank.json");
const STACK: &str = include_str!("../../../tools/ksp2-parity-pilot/craft-drop-stack.json");

#[test]
fn a_squat_craft_set_down_on_a_slope_rests_flush_with_it_and_nothing_sinks() {
    for slope in [5.0, 15.0, 30.0] {
        let l = land(PODTANK, slope, 0.05, None);
        println!(
            "pod and tank on {slope} degrees: sunk {:.3} m, tilt {:.1}",
            l.sunk_m, l.tilt_deg
        );
        assert!(l.landed && !l.tipped, "{slope} degrees: stands");
        assert!(l.sunk_m < 0.02, "{slope} degrees: sunk {:.3} m", l.sunk_m);
        assert!(
            (l.tilt_deg - slope).abs() < 0.5,
            "{slope} degrees: flush, tilt {:.1}",
            l.tilt_deg
        );
    }
}

#[test]
fn a_rocket_stands_on_a_gentle_slope_and_falls_over_on_a_steeper_one() {
    // the Swivel stack's centre of mass is 7.2 m over its bell's 0.38 m rim: KSP2's stood on 2.74 degrees and
    // fell over on 3.38 (reference\ksp2-slope on AI-House, `slope stack 1 <degrees>`)
    let gentle = land(STACK, 2.74, 0.05, None);
    assert!(gentle.landed && !gentle.tipped, "2.74 degrees: stands");
    assert!(
        gentle.sunk_m < 0.02,
        "2.74 degrees: bell sunk {:.3} m",
        gentle.sunk_m
    );
    assert!(
        (gentle.tilt_deg - 2.74).abs() < 0.5,
        "flush, tilt {:.1}",
        gentle.tilt_deg
    );
    let steep = land(STACK, 3.38, 0.05, None);
    assert!(steep.tipped, "3.38 degrees: falls over");
    assert!(
        steep.tilt_deg > 60.0,
        "lying down, tilt {:.1}",
        steep.tilt_deg
    );
}

#[test]
fn a_stack_lying_across_a_hillside_rolls_on_down_it_and_lies_still_on_the_flat() {
    // it meets the ground on its parts' model shapes (round tanks, the bell's rim), not on their boxes, and
    // comes to rest only with its centre of mass over what it lies on
    let flat = land(STACK, 0.0, 0.05, Some(0.0));
    println!("flat: moved {:.2} m, sunk {:.3}", flat.moved_m, flat.sunk_m);
    assert!(flat.landed && flat.moved_m < 1.0, "lies still on the flat");
    assert!(flat.sunk_m < 0.02, "sunk {:.3}", flat.sunk_m);
    let hill = land(STACK, 10.0, 0.05, Some(0.0));
    println!(
        "10 degrees: moved {:.2} m, sunk {:.3}",
        hill.moved_m, hill.sunk_m
    );
    assert!(
        hill.moved_m > 3.0,
        "rolls downhill, moved {:.2} m",
        hill.moved_m
    );
    assert!(hill.sunk_m < 0.05, "sunk {:.3}", hill.sunk_m);
}
