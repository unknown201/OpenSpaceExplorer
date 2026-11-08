//! KSP2 parity for solar panel facing, tracking and shading (gap T22 follow-up), from the original's code and the
//! panels' own prefab transforms (`tools/export_solar_geometry.py`): `Module_Deployable.CalculateTracking` turns a
//! tracking panel about its pivot's Y toward the sun at `trackingSpeed`; `Module_SolarPanel` takes the sun dot on
//! the panel's face and raycasts toward the sun for parts in the way.
use ose_math::DVec3;
use ose_sim::structure::Structure;

fn data() -> (
    Vec<ose_data::PartDef>,
    Vec<ose_data::ResourceDef>,
    ose_data::craft::Craft,
) {
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json")).unwrap();
    let res: Vec<ose_data::ResourceDef> =
        serde_json::from_str(include_str!("../../../assets/own/resources.json")).unwrap();
    let crafts: Vec<ose_data::craft::Craft> =
        serde_json::from_str(include_str!("../../../assets/own/stock-craft.json")).unwrap();
    let k1 = crafts.into_iter().find(|c| c.name == "Kerbal K1").unwrap();
    (defs, res, k1)
}

/// A Mk1 pod with `parts` hung off it: (part id, Unity position).
fn craft(k1: &ose_data::craft::Craft, parts: &[(&str, DVec3)]) -> ose_data::craft::Craft {
    let mut c = k1.clone();
    let pod = c.parts[0].clone();
    c.parts = vec![pod.clone()];
    for (k, (id, at)) in parts.iter().enumerate() {
        let mut p = pod.clone();
        p.part_id = id.to_string();
        p.guid = format!("part-{k}");
        p.parent = Some(0);
        p.own_node = None;
        p.parent_node = None;
        p.local_position = *at;
        p.resources.clear();
        c.parts.push(p);
    }
    c.stages.clear();
    c
}

#[test]
fn ksp2_fixed_flat_panels_face_their_own_minus_x() {
    let (defs, res, k1) = data();
    // the K1's OX-STAT panels: their live face is part -X (KSP2's raycastTransform turns `PanelIncidenceDirection`
    // +X round to part -X); OSE took +X
    let flats: Vec<usize> = k1
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.part_id == "solararray_0v_flat")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(flats.len(), 8);
    let mut st = Structure::build(&k1, &defs, &res);
    for &i in &flats {
        let face = ose_sim::aero::rotate(st.part_rot[i], DVec3::new(-1.0, 0.0, 0.0));
        for (dir, lit) in [(face, true), (face * -1.0, false)] {
            st.sun_local = Some(dir);
            st.update_solar(1.0, 0.02);
            let dot = st.solar_incidence(i);
            assert!(
                if lit { dot > 0.99 } else { dot == 0.0 },
                "panel {i}, sun on its {} side: {dot}",
                if lit { "-X" } else { "+X" }
            );
        }
    }
}

#[test]
fn ksp2_tracking_panels_turn_to_the_sun_and_lose_the_cosine_out_of_their_plane() {
    let (defs, res, k1) = data();
    let c = craft(&k1, &[("solararray_1v_1x6", DVec3::new(0.0, -2.0, 0.0))]);
    let mut st = Structure::build(&c, &defs, &res);
    st.deployed[1] = true;
    // the 1x6 turns about its boom (part X); the sun square on: it swings round and gives the full rating
    let axis = ose_sim::aero::rotate(st.part_rot[1], DVec3::new(1.0, 0.0, 0.0));
    let across = if axis.y.abs() < 0.9 {
        axis.cross(DVec3::new(0.0, 1.0, 0.0)).normalized()
    } else {
        axis.cross(DVec3::new(0.0, 0.0, 1.0)).normalized()
    };
    st.sun_local = Some(across);
    let mut dots = Vec::new();
    for k in 0..(60 * 50) {
        st.update_solar(1.0, 0.02);
        if k % 250 == 0 {
            dots.push(st.solar_incidence(1));
        }
    }
    println!("turning toward the sun: {dots:.3?}");
    assert!(st.solar_incidence(1) > 0.999, "{}", st.solar_incidence(1));
    // it turns at trackingSpeed (0.25 of the gap a second), not at once
    assert!(dots[1] < dots[dots.len() - 1]);
    // 30 degrees out of its plane: cos 30
    for deg in [30.0, 60.0] {
        let (s, c) = f64::to_radians(deg).sin_cos();
        st.sun_local = Some((across * c + axis * s).normalized());
        for _ in 0..(60 * 50) {
            st.update_solar(1.0, 0.02);
        }
        let dot = st.solar_incidence(1);
        println!("{deg} deg out of plane: {dot:.4} (cos {:.4})", c);
        assert!((dot - c).abs() < 0.002, "{deg}: {dot} vs {c}");
    }
}

#[test]
fn ksp2_a_part_between_a_panel_and_the_sun_shades_it() {
    let (defs, res, k1) = data();
    let open = craft(&k1, &[("solararray_1v_1x6", DVec3::new(0.0, -2.0, 0.0))]);
    let blocked = craft(
        &k1,
        &[
            ("solararray_1v_1x6", DVec3::new(0.0, -2.0, 0.0)),
            // the 1x6's ray starts 1.4 m out along its boom (part -X), and the sun will be straight down (-Y):
            // a tank 4 m below that point is in the way
            (
                "fueltank_1v_inline_methalox_1x4",
                DVec3::new(-1.4, -6.0, 0.0),
            ),
        ],
    );
    let mut lit = Vec::new();
    for c in [&open, &blocked] {
        let mut st = Structure::build(c, &defs, &res);
        st.deployed[1] = true;
        let axis = ose_sim::aero::rotate(st.part_rot[1], DVec3::new(1.0, 0.0, 0.0));
        let across = axis.cross(DVec3::new(0.0, 0.0, 1.0)).normalized();
        st.sun_local = Some(across);
        for _ in 0..(60 * 50) {
            st.update_solar(1.0, 0.02);
        }
        lit.push(st.solar_incidence(1));
    }
    println!(
        "open {:.3}, with a tank over its root {:.3}",
        lit[0], lit[1]
    );
    assert!(lit[0] > 0.99);
    assert_eq!(lit[1], 0.0, "the tank is in the way");
}
