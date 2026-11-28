//! Planes built in the hangar as a player builds them: a cockpit, a fuselage and wings put on with mirror
//! symmetry. The wings must span away from the fuselage, and the second wing of a pair must be the first's
//! mirror image across the plane's middle, as the stock jet's wings (placed by KSP2's own editor) are.
use ose_client::vab::{self, Catalog, Editor, PartSpec};
use ose_data::craft::{Craft, Quat};
use ose_math::DVec3;

fn catalog() -> Catalog {
    let defs: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    defs.into_iter()
        .map(|mut d| {
            if let Some(b) = d.get("bounds").cloned() {
                d["size_m"] = b["size_m"].clone();
                d["center_m"] = b["center_m"].clone();
            }
            let s: PartSpec = serde_json::from_value(d).unwrap();
            (s.id.clone(), s)
        })
        .collect()
}

fn stock(name: &str) -> Craft {
    let crafts: Vec<Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    crafts.into_iter().find(|c| c.name.contains(name)).unwrap()
}

const WINGS: [&str; 6] = [
    "wing_1v_procedural_light",
    "wing_2v_procedural_heavy",
    "wing_3v_procedural_spaceplane",
    "stabilizer_1v_procedural_light",
    "controlsurface_1v_procedural_light",
    "fin_0v_procedural",
];

/// A plane: cockpit and one fuselage, then `wing` put on the fuselage's side at `side` (+1 right, -1 left)
/// with `symmetry` 1 or 2.
fn plane(cat: &Catalog, wing: &str, side: f64, symmetry: u32) -> (Editor, usize, Vec<usize>) {
    let mut e = Editor::new("test");
    e.plane_new = true;
    e.add_part(&cat["cockpit_1v_m1_crew"], cat).unwrap();
    let f = e.add_part(&cat["fuselage_1v_m1_structural"], cat).unwrap();
    assert!(e.is_plane());
    let p = e.craft.parts[f].local_position;
    let q = e.craft.parts[f].local_rotation;
    let hit = p + vab::rotate_by(q, DVec3::new(0.625 * side, 0.2, 0.0));
    let spot = e.surface_spot(f, hit, cat, None).unwrap();
    let made = e.add_surface(&cat[wing], cat, spot, symmetry).unwrap();
    (e, f, made)
}

fn mirrored(e: &Editor, i: usize) -> bool {
    e.craft.parts[i].flight.as_ref().is_some_and(|f| f.mirrored)
}

/// The wing's baked meshes in the craft frame, as the hangar and flight draw them.
fn world_points(e: &Editor, i: usize) -> Vec<DVec3> {
    let p = &e.craft.parts[i];
    let shape = ose_client::wing_mesh::shape_of(p).unwrap();
    let r = ose_data::procedural::evaluate(&p.part_id, &shape, mirrored(e, i)).unwrap();
    r.meshes
        .iter()
        .flat_map(|m| m.positions.iter())
        .map(|v| p.local_position + vab::rotate_by(p.local_rotation, DVec3::new(v[0], v[1], v[2])))
        .collect()
}

fn bounds(pts: &[DVec3]) -> (DVec3, DVec3) {
    let mut lo = DVec3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut hi = DVec3::new(f64::MIN, f64::MIN, f64::MIN);
    for v in pts {
        lo = DVec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = DVec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
    }
    (lo, hi)
}

/// Every point of `b` lies on a point of `a` reflected across the craft's middle (x = 0), vertex for vertex.
fn assert_mirror_images(a: &[DVec3], b: &[DVec3], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}");
    for (pa, pb) in a.iter().zip(b) {
        let r = DVec3::new(-pa.x, pa.y, pa.z);
        assert!((r - *pb).length() < 1e-6, "{what}: {pa:?} vs {pb:?}");
    }
}

fn same_turn(a: Quat, b: Quat) -> bool {
    let d = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    (d.abs() - 1.0).abs() < 1e-6
}

#[test]
fn a_mirrored_wing_pair_spans_out_and_mirrors_exactly() {
    let cat = catalog();
    for wing in WINGS {
        for side in [1.0, -1.0] {
            let (e, _, made) = plane(&cat, wing, side, 2);
            assert_eq!(made.len(), 2, "{wing}");
            let (a, b) = (made[0], made[1]);
            // The one on the right (+X) is the one baked mirrored, whichever was put on first.
            let right = if e.craft.parts[a].local_position.x > 0.0 {
                a
            } else {
                b
            };
            let left = if right == a { b } else { a };
            assert!(
                mirrored(&e, right) && !mirrored(&e, left),
                "{wing} side {side}"
            );
            let (pr, pl) = (world_points(&e, right), world_points(&e, left));
            // Both stay outside the 1.25 m fuselage: the span goes away from it.
            let (lo, _) = bounds(&pr);
            let (_, hi) = bounds(&pl);
            assert!(
                lo.x > 0.6 && hi.x < -0.6,
                "{wing}: right from {lo:?}, left to {hi:?}"
            );
            assert_mirror_images(&pl, &pr, wing);
        }
    }
}

#[test]
fn hangar_wings_sit_as_the_stock_jets_wings() {
    // KSP2's own editor placed the stock jet's wings: left unmirrored, right mirrored, both turned so the
    // span goes out, the chord along the nose and the top up.
    let jet = stock("Jet");
    let pick = |x: f64| {
        jet.parts
            .iter()
            .find(|p| p.part_id == "wing_1v_procedural_light" && p.local_position.x * x > 0.0)
            .unwrap()
    };
    let cat = catalog();
    let (e, _, made) = plane(&cat, "wing_1v_procedural_light", 1.0, 2);
    for &k in &made {
        let p = &e.craft.parts[k];
        let s = pick(p.local_position.x);
        assert_eq!(
            mirrored(&e, k),
            s.flight.as_ref().is_some_and(|f| f.mirrored),
            "{:?}",
            p.local_position
        );
        // The jet's wings sit 2.5 degrees off the editor's square pose (put on a little off the side).
        let d = (p.local_rotation.x * s.local_rotation.x
            + p.local_rotation.y * s.local_rotation.y
            + p.local_rotation.z * s.local_rotation.z
            + p.local_rotation.w * s.local_rotation.w)
            .abs();
        assert!(
            d > 0.999,
            "{:?} vs stock {:?}",
            p.local_rotation,
            s.local_rotation
        );
    }
}

#[test]
fn a_single_wing_bakes_mirrored_on_the_right_only() {
    let cat = catalog();
    for wing in WINGS {
        let (e, _, r) = plane(&cat, wing, 1.0, 1);
        let (e2, _, l) = plane(&cat, wing, -1.0, 1);
        assert!(mirrored(&e, r[0]) && !mirrored(&e2, l[0]), "{wing}");
        let (pr, pl) = (world_points(&e, r[0]), world_points(&e2, l[0]));
        assert_mirror_images(&pl, &pr, wing);
        // Put on alone or as a pair, the right wing is the same.
        let (e3, _, pair) = plane(&cat, wing, 1.0, 2);
        assert!(same_turn(
            e.craft.parts[r[0]].local_rotation,
            e3.craft.parts[pair[0]].local_rotation
        ));
    }
}

#[test]
fn rocket_fins_span_out_from_the_tank() {
    let cat = catalog();
    let mut e = Editor::new("rocket");
    e.add_part(&cat["pod_1v_conical_crew"], &cat).unwrap();
    let t = e
        .add_part(&cat["fueltank_1v_m1_inline_methane"], &cat)
        .unwrap();
    assert!(!e.is_plane());
    let p = e.craft.parts[t].local_position;
    let spot = e
        .surface_spot(t, p + DVec3::new(0.625, -0.3, 0.0), &cat, None)
        .unwrap();
    let fins = e
        .add_surface(&cat["fin_0v_procedural"], &cat, spot, 4)
        .unwrap();
    assert_eq!(fins.len(), 4);
    for k in fins {
        assert!(!mirrored(&e, k));
        let fp = e.craft.parts[k].local_position;
        let out = DVec3::new(fp.x - p.x, 0.0, fp.z - p.z).normalized();
        for v in world_points(&e, k) {
            let r = DVec3::new(v.x - p.x, 0.0, v.z - p.z).dot(out);
            assert!(r > 0.6, "fin {k} reaches {r} m from the tank's axis");
        }
    }
}

#[test]
fn landing_gear_hangs_under_the_fuselage_as_the_stock_jets() {
    // The stock jet's nose gear (put on by KSP2's editor under the cockpit): leg up into the fuselage (+Y
    // up), wheel rolling along the nose (+Z forward), the editor's square pose to within a degree.
    let jet = stock("Jet");
    let nose = jet
        .parts
        .iter()
        .find(|p| p.part_id == "landinggear_0v_steerable")
        .unwrap();
    let cat = catalog();
    for gear in [
        "landinggear_0v_steerable",
        "landinggear_1v",
        "landinggear_2v",
        "landinggear_3v",
    ] {
        let mut e = Editor::new("test");
        e.plane_new = true;
        e.add_part(&cat["cockpit_1v_m1_crew"], &cat).unwrap();
        let f = e.add_part(&cat["fuselage_1v_m1_structural"], &cat).unwrap();
        let p = e.craft.parts[f].local_position;
        let q = e.craft.parts[f].local_rotation;
        // The fuselage's underside (its own +Z, as it lies along the floor).
        let local = DVec3::new(0.0, 0.0, 0.625);
        let spot = e
            .surface_spot_on(
                f,
                p + vab::rotate_by(q, local),
                Some(vab::rotate_by(q, local.normalized())),
                &cat,
                None,
            )
            .unwrap();
        let k = e.add_surface(&cat[gear], &cat, spot, 1).unwrap()[0];
        let g = e.craft.parts[k].local_rotation;
        for axis in [DVec3::new(0.0, 1.0, 0.0), DVec3::new(0.0, 0.0, 1.0)] {
            let ours = vab::rotate_by(g, axis);
            let theirs = vab::rotate_by(nose.local_rotation, axis);
            assert!(ours.dot(theirs) > 0.999, "{gear}: {ours:?} vs {theirs:?}");
        }
    }
}

// -- Test planes built as a player builds them, flown by `ose-sim`'s `editor_planes` test.

/// Put `spec` on part `target` where the pointer meets its drawn shape at craft-frame `hit` with outward
/// `normal`, as the hangar does under the cursor.
fn put(
    e: &mut Editor,
    cat: &Catalog,
    target: usize,
    hit: DVec3,
    normal: DVec3,
    spec: &str,
    symmetry: u32,
) -> Vec<usize> {
    let spot = e
        .surface_spot_on(target, hit, Some(normal), cat, None)
        .unwrap();
    e.add_surface(&cat[spec], cat, spot, symmetry).unwrap()
}

/// The craft-frame point on part `i` (a fuselage lying along the floor) at its own `local` offset.
fn on(e: &Editor, i: usize, local: DVec3) -> (DVec3, DVec3) {
    let p = &e.craft.parts[i];
    let n = DVec3::new(local.x, 0.0, local.z).normalized();
    (
        p.local_position + vab::rotate_by(p.local_rotation, local),
        vab::rotate_by(p.local_rotation, n),
    )
}

/// The underside of wing `w` at craft-frame `x`, as near `z` as the wing reaches there.
fn under_wing(e: &Editor, w: usize, x: f64, z: f64) -> DVec3 {
    let pts: Vec<DVec3> = world_points(e, w)
        .into_iter()
        .filter(|v| (v.x - x).abs() < 0.3)
        .collect();
    let near = pts
        .iter()
        .min_by(|a, b| (a.z - z).abs().total_cmp(&(b.z - z).abs()))
        .unwrap_or_else(|| panic!("no wing at x {x}"));
    let y = pts
        .iter()
        .filter(|v| (v.z - near.z).abs() < 0.3)
        .map(|v| v.y)
        .fold(f64::MAX, f64::min);
    DVec3::new(x, y, near.z)
}

fn set_shape(e: &mut Editor, i: usize, sliders: &[(&str, f64)]) {
    let mut s = ose_client::wing_mesh::shape_of(&e.craft.parts[i]).unwrap();
    for (n, v) in sliders {
        s.set_slider(n, *v);
    }
    e.craft.parts[i].shape = Some(s);
    e.sync_symmetry(i);
}

/// A plane: cockpit, `tanks` fuselage tanks, a wing pair on the first, main gear under the wings (this far
/// out and back from the right wing's root), a nose
/// wheel under the cockpit, a tail (stabilizer pair and a fin) and a radial intake on the last tank, a jet
/// engine behind it.
fn build(
    cat: &Catalog,
    name: &str,
    tanks: usize,
    wing: &str,
    sliders: &[(&str, f64)],
    (gear_out, gear_back): (f64, f64),
    engine: &str,
) -> Craft {
    let mut e = Editor::new(name);
    e.plane_new = true;
    let cockpit = e.add_part(&cat["cockpit_1v_m1_crew"], cat).unwrap();
    let mut last = cockpit;
    let mut body = Vec::new();
    for _ in 0..tanks {
        last = e
            .add_part_on(&cat["fueltank_1v_m1_inline_methane"], cat, last, "bottom")
            .unwrap();
        body.push(last);
    }
    e.add_part_on(&cat[engine], cat, last, "top").unwrap();
    // Wings on the first tank's right side, half a metre behind its middle; the left one follows.
    let (hit, n) = on(&e, body[0], DVec3::new(0.625, -0.5, 0.0));
    let wings = put(&mut e, cat, body[0], hit, n, wing, 2);
    set_shape(&mut e, wings[0], sliders);
    // Main gear a little out on the right wing's underside, behind its root; on a set it goes on both.
    let root = e.craft.parts[wings[0]].local_position;
    let at = under_wing(&e, wings[0], root.x + gear_out, root.z - gear_back);
    let gear = put(
        &mut e,
        cat,
        wings[0],
        at,
        DVec3::new(0.0, -1.0, 0.0),
        "landinggear_1v",
        1,
    );
    assert_eq!(gear.len(), 2, "{name}: main gear on both wings");
    // The nose wheel under the cockpit's front.
    let (hit, n) = on(&e, cockpit, DVec3::new(0.0, 0.85, 0.63));
    put(&mut e, cat, cockpit, hit, n, "landinggear_0v_steerable", 1);
    // The tail on the last tank: stabilizers out its sides, a fin on top, the intake underneath.
    let (hit, n) = on(&e, last, DVec3::new(0.625, -0.6, 0.0));
    put(
        &mut e,
        cat,
        last,
        hit,
        n,
        "stabilizer_1v_procedural_light",
        2,
    );
    let (hit, n) = on(&e, last, DVec3::new(0.0, -0.6, -0.625));
    put(
        &mut e,
        cat,
        last,
        hit,
        n,
        "stabilizer_1v_procedural_light",
        1,
    );
    let (hit, n) = on(&e, last, DVec3::new(0.0, 0.2, 0.625));
    put(&mut e, cat, last, hit, n, "intake_1v_radial", 1);
    assert!(e.craft.is_tree());
    e.craft.clone()
}

fn editor_planes(cat: &Catalog) -> Vec<Craft> {
    vec![
        // Small: the stock jet's layout, default wings.
        build(
            cat,
            "Small",
            2,
            "wing_1v_procedural_light",
            &[],
            (0.5, 0.9),
            "engine_1v_turbofan_methane_wheesley",
        ),
        // Medium: a longer fuselage, the heavy wing a little swept back, a Panther.
        build(
            cat,
            "Medium heavy wing",
            3,
            "wing_2v_procedural_heavy",
            &[("WING_ANGLE", 0.4), ("WING_SPAN", 0.3)],
            (0.8, 0.9),
            "engine_1v_turbofan_methane_panther",
        ),
        // Small with its wings swept well back.
        build(
            cat,
            "Small swept",
            2,
            "wing_1v_procedural_light",
            &[("WING_ANGLE", 0.15), ("WING_SPAN", 0.3)],
            (0.5, 1.5),
            "engine_1v_turbofan_methane_wheesley",
        ),
        // Small with long straight wings (`WING_ANGLE` 0.5 is unswept).
        build(
            cat,
            "Small straight",
            2,
            "wing_1v_procedural_light",
            &[("WING_ANGLE", 0.5), ("WING_SPAN", 0.6), ("TIP_LENGTH", 0.6)],
            (0.6, 1.5),
            "engine_1v_turbofan_methane_wheesley",
        ),
    ]
}

const PLANES: &str = "../ose-sim/tests/data/editor-planes.json";

/// The planes `ose-sim` flies are the ones this editor builds; `OSE_WRITE_PLANES=1` writes them anew.
#[test]
fn editor_planes_fixture_is_current() {
    let cat = catalog();
    let planes = editor_planes(&cat);
    let text = serde_json::to_string_pretty(&planes).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PLANES);
    if std::env::var_os("OSE_WRITE_PLANES").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        on_disk == text,
        "editor-planes.json is stale: run with OSE_WRITE_PLANES=1"
    );
}
