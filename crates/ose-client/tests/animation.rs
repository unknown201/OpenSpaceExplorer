use glam::{Mat4, Quat, Vec3};
use ose_client::animation::{skin_vertex, Accessors, ClipSample, Deployment, Rig};
use serde_json::{json, Value};

fn fixture(interpolation: &str, rotation: bool) -> (Value, Vec<u8>) {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |values: &[f32], width: usize, ty: &str| {
        let start = bin.len();
        for v in values {
            bin.extend(v.to_le_bytes());
        }
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":values.len()*4}));
        accessors.push(json!({"bufferView":views.len()-1,"componentType":5126,"count":values.len()/width,"type":ty}));
    };
    add(&[0.0, 2.0], 1, "SCALAR");
    if rotation {
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        add(&[0.0, 0.0, 0.0, 1.0, q.x, q.y, q.z, q.w], 4, "VEC4");
    } else if interpolation == "CUBICSPLINE" {
        // A zero-tangent cubic goes from 0 to 4; at t=0.25 it is 0.625 (not linear 1).
        add(
            &[
                0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 4., 0., 0., 0., 0., 0.,
            ],
            3,
            "VEC3",
        );
    } else {
        add(&[0., 0., 0., 4., 0., 0.], 3, "VEC3");
    }
    (
        json!({
            "scene":0,"scenes":[{"nodes":[1]}],
            // Child precedes parent to catch implementations that assume file-order traversal.
            "nodes":[{"name":"joint","translation":[0,1,0]}, {"children":[0],"translation":[10,0,0]}],
            "bufferViews":views,"accessors":accessors,
            "animations":[{"name":"deploy","samplers":[{"input":0,"output":1,"interpolation":interpolation}],
                "channels":[{"sampler":0,"target":{"node":0,"path":if rotation {"rotation"} else {"translation"}}}]}],
            "extras":{"ose_deployment":{"clip":0}}
        }),
        bin,
    )
}

#[test]
fn solar_pose_is_authoritative_mirrored_and_independent_for_shared_models() {
    let (mut json, bytes) = fixture("LINEAR", false);
    json["nodes"][0]["name"] = json!("bone_rotator");
    let rig = Rig::from_gltf(&json, &bytes).unwrap();
    let (_, left) = rig.solar_turn("bone_rotator", 90.).unwrap();
    let (_, right) = rig.solar_turn("bone_rotator", -90.).unwrap();
    assert!(
        (left * Vec3::Z + Vec3::X).length() < 1e-6,
        "Unity Y turns are reflected in glTF"
    );
    assert!((right * Vec3::Z - Vec3::X).length() < 1e-6);
    assert_eq!(
        rig.pose(None)[0].rotation,
        Quat::IDENTITY,
        "per-instance turn cannot mutate shared rest pose"
    );
    assert!(rig.solar_turn("missing-pivot", 30.).is_none());
    assert!(rig.solar_turn("bone_rotator", f32::NAN).is_none());
}

#[test]
fn hierarchy_sampling_looping_and_instance_isolation() {
    let (j, b) = fixture("LINEAR", false);
    let rig = Rig::from_gltf(&j, &b).unwrap();
    let a = rig.pose(Some(ClipSample {
        clip: 0,
        time: 1.0,
        looping: false,
    }));
    let c = rig.pose(Some(ClipSample {
        clip: 0,
        time: 2.5,
        looping: true,
    }));
    assert!(
        (rig.world_matrices(&a)[0].transform_point3(Vec3::ZERO) - Vec3::new(12., 0., 0.)).length()
            < 1e-6
    );
    assert_eq!(c[0].translation, Vec3::X);
    assert_eq!(a[0].translation, Vec3::X * 2.0);
    assert_eq!(rig.pose(None)[0].translation, Vec3::Y);
    assert_eq!(
        rig.pose(rig.deployment_sample(1.0))[0].translation,
        Vec3::X * 4.0
    );
}

#[test]
fn interpolation_modes_and_quaternion_normalization() {
    for (mode, expected) in [("STEP", 0.0), ("LINEAR", 1.0), ("CUBICSPLINE", 0.625)] {
        let (j, b) = fixture(mode, false);
        let rig = Rig::from_gltf(&j, &b).unwrap();
        assert!(
            (rig.pose(Some(ClipSample {
                clip: 0,
                time: 0.5,
                looping: false
            }))[0]
                .translation
                .x
                - expected)
                .abs()
                < 1e-5
        );
    }
    let (j, b) = fixture("LINEAR", true);
    let rig = Rig::from_gltf(&j, &b).unwrap();
    let q = rig.pose(Some(ClipSample {
        clip: 0,
        time: 1.0,
        looping: false,
    }))[0]
        .rotation;
    assert!((q * Vec3::X - Vec3::new(0.5f32.sqrt(), 0.5f32.sqrt(), 0.)).length() < 1e-5);
    assert!((q.length() - 1.0).abs() < 1e-6);
}

#[test]
fn inverse_bind_and_mesh_transform_are_applied_once() {
    let j = json!({"nodes":[{"translation":[10,0,0],"skin":0},{"translation":[12,0,0]}],
        "skins":[{"joints":[1]}]});
    let mut rig = Rig::from_gltf(&j, &[]).unwrap();
    rig.skins[0].inverse_bind[0] = Mat4::from_translation(Vec3::new(-2., 0., 0.));
    let mut pose = rig.pose(None);
    let rest = rig.palette(0, &rig.world_matrices(&pose)).unwrap();
    assert!((rest[0].transform_point3(Vec3::X) - Vec3::X).length() < 1e-6);
    pose[1].translation.y = 3.0;
    let palette = rig.palette(0, &rig.world_matrices(&pose)).unwrap();
    let (p, n) = skin_vertex(Vec3::X, Vec3::Y, [0; 4], [0.25, 0., 0., 0.], &palette);
    assert!((p - Vec3::new(1., 3., 0.)).length() < 1e-6);
    assert_eq!(n, Vec3::Y);
    assert_eq!(
        skin_vertex(Vec3::X, Vec3::Y, [0; 4], [0.; 4], &palette),
        (Vec3::X, Vec3::Y)
    );
}

#[test]
fn skin_normals_handle_nonuniform_scale() {
    let n = Vec3::new(1., 1., 0.).normalize();
    let (_, normal) = skin_vertex(
        Vec3::ZERO,
        n,
        [0; 4],
        [1., 0., 0., 0.],
        &[Mat4::from_scale(Vec3::new(2., 1., 1.))],
    );
    assert!((normal - Vec3::new(0.5, 1., 0.).normalize()).length() < 1e-6);
}

#[test]
fn deployment_reverses_without_jumping_pauses_and_late_joins() {
    let mut d = Deployment::new(false, 0.0);
    assert_eq!(d.update(true, 0.0, 2.0), 0.0);
    assert_eq!(d.update(true, 1.0, 2.0), 0.5);
    assert_eq!(d.update(false, 1.0, 2.0), 0.5);
    assert_eq!(d.update(false, 1.0, 2.0), 0.5);
    assert_eq!(d.update(false, 1.5, 2.0), 0.25);
    assert_eq!(d.update(false, 20.0, 2.0), 0.0);
    assert_eq!(Deployment::new(true, 100.0).update(true, 100.0, 2.0), 1.0);
}

#[test]
fn corrupt_assets_are_rejected_without_panics() {
    for nodes in [
        json!([{"children":[9]}]),
        json!([{"children":[1]},{"children":[0]}]),
        json!([{"children":[1,1]},{}]),
    ] {
        assert!(Rig::from_gltf(&json!({"nodes":nodes}), &[]).is_err());
    }
    let (mut j, b) = fixture("LINEAR", false);
    j["bufferViews"][0]["byteLength"] = json!(1);
    assert!(Rig::from_gltf(&j, &b).is_err());
    let (j, mut b) = fixture("LINEAR", false);
    b[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(Rig::from_gltf(&j, &b).is_err());
}

#[test]
fn interleaved_normalized_weights_and_offsets() {
    let j = json!({"accessors":[{"bufferView":0,"byteOffset":2,"count":2,"type":"VEC4","componentType":5121,"normalized":true}],
        "bufferViews":[{"byteOffset":1,"byteLength":14,"byteStride":8}]});
    let b = [0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 128, 127, 0, 0];
    let w = Accessors { json: &j, bin: &b }.floats(0, 4).unwrap();
    assert_eq!(&w[..4], &[1., 0., 0., 0.]);
    assert!((w[5] + w[6] - 1.0).abs() < 1e-6);
}

#[test]
fn masked_layers_and_late_constraints_work_through_parents() {
    let j =
        json!({"nodes":[{"translation":[10,0,0],"children":[1]}, {}, {"translation":[12,1,0]}]});
    let rig = Rig::from_gltf(&j, &[]).unwrap();
    let mut pose = rig.pose(None);
    let mut layer = pose.clone();
    layer[0].translation = Vec3::ZERO;
    layer[1].translation = Vec3::new(2., 0., 0.);
    rig.blend_pose(&mut pose, &layer, 0.5, Some(&[0., 1., 0.]));
    assert_eq!(pose[0].translation, Vec3::X * 10.0);
    assert_eq!(pose[1].translation, Vec3::X);
    rig.aim_at(&mut pose, 1, Vec3::new(11., 1., 0.), Vec3::X, 1.0);
    assert!((rig.world_matrices(&pose)[1].transform_vector3(Vec3::X) - Vec3::Y).length() < 1e-5);
    rig.copy_position(&mut pose, 1, 2, 1.0);
    assert!(
        (rig.world_matrices(&pose)[1].transform_point3(Vec3::ZERO) - Vec3::new(12., 1., 0.))
            .length()
            < 1e-5
    );
}

/// Two legs from a pelvis at the origin's height, knees bent a little forward: on a 20 degree slope the
/// solver lowers the model and bends the uphill leg so both feet land on the slope, at the legs' own lengths.
#[test]
fn feet_plant_on_a_slope() {
    let leg = |side: f32, base: usize| {
        vec![
            json!({"name": format!("thigh{base}"), "translation":[side*0.1, 0.0, 0.0], "children":[base+1]}),
            json!({"name": format!("knee{base}"), "translation":[0.0, -0.5, 0.05], "children":[base+2]}),
            json!({"name": format!("foot{base}"), "translation":[0.0, -0.5, -0.05]}),
        ]
    };
    let mut nodes = vec![json!({"name":"pelvis","translation":[0.0, 1.0, 0.0],"children":[1, 4]})];
    nodes.extend(leg(1.0, 1));
    nodes.extend(leg(-1.0, 4));
    let j = json!({"scene":0,"scenes":[{"nodes":[0]}],"nodes":nodes});
    let rig = Rig::from_gltf(&j, &[]).unwrap();
    let legs = [("thigh1", "knee1", "foot1"), ("thigh4", "knee4", "foot4")];
    assert!(
        rig.plant_feet(None, Vec3::Y, &legs).is_none(),
        "flat ground"
    );
    // the ground rises toward -x (thigh4's side)
    let n = Quat::from_rotation_z(20f32.to_radians()) * Vec3::Y;
    let (drop, turns) = rig.plant_feet(None, n, &legs).unwrap();
    assert!(drop < 0.0, "lowered for the downhill foot ({drop})");
    let mut pose = rig.pose(None);
    for (i, t) in &turns {
        pose[*i].rotation = (pose[*i].rotation * *t).normalize();
    }
    let world = rig.world_matrices(&pose);
    let at = |name: &str| {
        let i = rig.nodes.iter().position(|b| b.name == name).unwrap();
        world[i].w_axis.truncate()
    };
    // the ground in the model's frame: the slope through the stand point (y 0), raised by the drop
    let ground = |p: Vec3| -(n.x * p.x + n.z * p.z) / n.y - drop;
    for (thigh, knee, foot) in legs {
        let f = at(foot);
        assert!(
            (f.y - ground(f)).abs() < 0.01,
            "{foot} on the slope: {} vs {}",
            f.y,
            ground(f)
        );
        assert!(
            ((at(knee) - at(thigh)).length() - 0.5025).abs() < 1e-3,
            "thigh length kept"
        );
        assert!(
            ((f - at(knee)).length() - 0.5025).abs() < 1e-3,
            "shin length kept"
        );
    }
}
