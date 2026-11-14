//! Shaped procedural fairings drawn: each panel (a clamshell piece, or the whole shroud) is its own small
//! model, a lathe of the wall's cross sections over the panel's arc, so flight can send each one off on its
//! own once jettisoned. The panels borrow `assets/fx/fairing_ring.glb`'s material.

use ose_data::fairing::{FairingDef, FairingShape};
use serde_json::{json, Value};

pub const BASE_URL: &str = "assets/fx/fairing_ring.glb";

/// How many panels a shape comes apart into (a shroud is one).
pub fn piece_count(shape: &FairingShape) -> usize {
    (shape.pieces as usize).max(1)
}

/// The model name of panel `k` of `shape` on `part_id`.
pub fn model_name(part_id: &str, shape: &FairingShape, k: usize) -> String {
    let key = format!("{part_id}{shape:?}");
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    format!("fairing:{hash:016x}:{k}")
}

/// The middle of panel `k`'s arc, unit, in the part's glTF frame (X mirrored): where it flies off to.
pub fn piece_direction(shape: &FairingShape, k: usize) -> glam::Vec3 {
    let n = piece_count(shape);
    if shape.pieces == 0 {
        return glam::Vec3::Y;
    }
    let a = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
    glam::Vec3::new(-a.cos(), 0.0, a.sin())
}

/// Panel `k`'s lathe: positions (glTF frame, the part's origin), uvs, counter-clockwise triangles.
pub fn piece_mesh(
    shape: &FairingShape,
    def: &FairingDef,
    k: usize,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let n = piece_count(shape);
    let sides = (def.side_count as usize).max(8);
    let per = (sides / n).max(2);
    let (a0, a1) = (
        k as f64 / n as f64 * std::f64::consts::TAU,
        (k + 1) as f64 / n as f64 * std::f64::consts::TAU,
    );
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    let mut tris = Vec::new();
    let rows = shape.sections.len();
    let height = shape.height().max(1e-6);
    for (h, r) in &shape.sections {
        for s in 0..=per {
            let a = a0 + (a1 - a0) * s as f64 / per as f64;
            // Unity (x, y, z) -> glTF (-x, y, z)
            pos.push([
                -(r * a.cos()) as f32,
                (def.base_height + h) as f32,
                (r * a.sin()) as f32,
            ]);
            uv.push([s as f32 / per as f32, (h / height) as f32]);
        }
    }
    let w = (per + 1) as u32;
    for row in 0..rows.saturating_sub(1) as u32 {
        for s in 0..per as u32 {
            let (a, b) = (row * w + s, row * w + s + 1);
            let (c, d) = (a + w, b + w);
            tris.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // A capped wall closes over its top section.
    if shape.capped {
        if let Some((h, _)) = shape.sections.last() {
            let apex = pos.len() as u32;
            pos.push([0.0, (def.base_height + h) as f32, 0.0]);
            uv.push([0.5, 1.0]);
            let top = (rows as u32 - 1) * w;
            for s in 0..per as u32 {
                tris.extend_from_slice(&[top + s, apex, top + s + 1]);
            }
        }
    }
    // Make sure the faces point outward (the panel's middle normal away from the axis).
    if let Some(t) = tris.get(0..3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| glam::Vec3::from_array(pos[i as usize]));
        let n = (b - a).cross(c - a);
        let out = glam::Vec3::new(a.x, 0.0, a.z);
        if n.dot(out) < 0.0 {
            for t in tris.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
    }
    (pos, uv, tris)
}

/// Panel `k` as a GLB with `base`'s material.
pub fn piece_glb(
    base: &[u8],
    shape: &FairingShape,
    def: &FairingDef,
    k: usize,
) -> Result<Vec<u8>, String> {
    let (pos, uv, tris) = piece_mesh(shape, def, k);
    let src = crate::wing_mesh::read_json(base)?;
    let mut normals = vec![glam::Vec3::ZERO; pos.len()];
    for t in tris.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| glam::Vec3::from_array(pos[i as usize]));
        let n = (b - a).cross(c - a);
        for i in t {
            normals[*i as usize] += n;
        }
    }
    let normals: Vec<[f32; 3]> = normals
        .iter()
        .map(|n| n.normalize_or(glam::Vec3::Y).to_array())
        .collect();
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut push = |bytes: Vec<u8>, target: u32, acc: Value| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len(), "target": target}));
        bin.extend_from_slice(&bytes);
        let mut acc = acc;
        acc["bufferView"] = json!(views.len() - 1);
        accessors.push(acc);
        accessors.len() - 1
    };
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in &pos {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let p = push(
        floats(pos.as_flattened()),
        34962,
        json!({"componentType": 5126, "count": pos.len(), "type": "VEC3", "min": lo, "max": hi}),
    );
    let nn = push(
        floats(normals.as_flattened()),
        34962,
        json!({"componentType": 5126, "count": normals.len(), "type": "VEC3"}),
    );
    let t = push(
        floats(uv.as_flattened()),
        34962,
        json!({"componentType": 5126, "count": uv.len(), "type": "VEC2"}),
    );
    let i = push(
        tris.iter().flat_map(|i| i.to_le_bytes()).collect(),
        34963,
        json!({"componentType": 5125, "count": tris.len(), "type": "SCALAR"}),
    );
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut prim =
        json!({"attributes": {"POSITION": p, "NORMAL": nn, "TEXCOORD_0": t}, "indices": i});
    if src.get("materials").is_some() {
        prim["material"] = json!(0);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "ose fairing_mesh"},
        "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"name": format!("fairing_panel_{k}"), "mesh": 0}],
        "meshes": [{"name": format!("fairing_panel_{k}"), "primitives": [prim]}],
        "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": bin.len()}],
    });
    for key in ["materials", "textures", "images", "samplers"] {
        if let Some(v) = src.get(key) {
            doc[key] = v.clone();
        }
    }
    Ok(crate::wing_mesh::glb_bytes(&doc, &bin))
}

/// Ask for every panel of part `p`'s shaped fairing; their names, once all are loaded.
#[cfg(target_arch = "wasm32")]
pub fn models_for(
    gpu: &mut crate::render::Renderer,
    p: &ose_data::craft::CraftPart,
) -> Option<Vec<String>> {
    let shape = p.fairing.as_ref().filter(|f| f.valid_for(&p.part_id))?;
    let def = ose_data::fairing::def(&p.part_id)?;
    let names: Vec<String> = (0..piece_count(shape))
        .map(|k| model_name(&p.part_id, shape, k))
        .collect();
    for (k, name) in names.iter().enumerate() {
        let (s, d) = (shape.clone(), def.clone());
        gpu.request_built_model(name, BASE_URL, move |base| piece_glb(base, &s, &d, k));
    }
    names.iter().all(|n| gpu.has_model(n)).then_some(names)
}

/// An engine's shroud as the original builds it (`ose_data::engine_shroud`): one panel, a lathe from the
/// bottom ring (`target_radius`) up to the top ring, in the part's frame.
pub fn shroud_shape(
    d: &ose_data::engine_shroud::ShroudDef,
    target_radius: f64,
) -> (FairingShape, FairingDef) {
    let [lo, hi] = d.rings(target_radius);
    let shape = FairingShape {
        sections: vec![(0.0, lo.1), (hi.0 - lo.0, hi.1)],
        capped: false,
        pieces: 0,
        ejection_force: 0.0,
    };
    let def = FairingDef {
        mass_t: 0.0,
        base_radius: d.base_radius,
        cap_radius: 0.0,
        max_radius: d.base_radius.max(target_radius),
        mass_area_ratio: 0.0,
        height_min: 0.0,
        height_max: d.height,
        side_count: d.side_count,
        length_snap: 0.0,
        radius_snap: 0.0,
        ejection_force: 0.0,
        pieces: 0,
        base_height: lo.0,
    };
    (shape, def)
}

/// The model name of `part_id`'s shroud flaring to `target_radius`.
pub fn shroud_model_name(part_id: &str, target_radius: f64) -> String {
    format!("shroud:{part_id}:{:.4}", target_radius)
}

/// Part `i`'s engine shroud model, when a part hangs from its shroud node, once it is loaded.
#[cfg(target_arch = "wasm32")]
pub fn shroud_model_for(
    gpu: &mut crate::render::Renderer,
    craft: &ose_data::craft::Craft,
    i: usize,
) -> Option<String> {
    let (d, target) = ose_data::engine_shroud::shroud_for(craft, i)?;
    let name = shroud_model_name(&craft.parts[i].part_id, target);
    let (shape, def) = shroud_shape(d, target);
    gpu.request_built_model(&name, BASE_URL, move |base| {
        piece_glb(base, &shape, &def, 0)
    });
    gpu.has_model(&name).then_some(name)
}

/// The shroud drawn beside its engine's draw item: the engine's pose and look, the wall tinted in the vessel's
/// base colour (the original paints `FairingsProceduralMat` with `Module_Color`'s base and accent; the
/// stand-in material here has no paint mask).
#[cfg(target_arch = "wasm32")]
pub fn shroud_item(engine: &crate::render::DrawItem, model: String) -> crate::render::DrawItem {
    let mut it = engine.companion(model);
    if let Some((base, _)) = engine.paint {
        for (c, b) in it.color.iter_mut().zip(base) {
            *c *= b;
        }
    }
    it
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamshell_panels_split_the_wall_and_face_outward() {
        let d = ose_data::fairing::def("fairing_1v_procedural").unwrap();
        let s = FairingShape::cylinder(d, 2.0);
        assert_eq!(piece_count(&s), 4);
        let (pos, _, tris) = piece_mesh(&s, d, 1);
        assert!(!tris.is_empty() && tris.len() % 3 == 0);
        // every vertex of panel 1 lies in its quarter
        let dir = piece_direction(&s, 1);
        assert!(pos
            .iter()
            .filter(|p| p[0] != 0.0 || p[2] != 0.0)
            .all(|p| glam::Vec3::new(p[0], 0.0, p[2]).dot(dir) >= -1e-4));
        let shroud = FairingShape { pieces: 0, ..s };
        assert_eq!(piece_count(&shroud), 1);
        assert_eq!(piece_direction(&shroud, 0), glam::Vec3::Y);
    }

    #[test]
    fn swivel_shroud_mesh_wraps_the_engine_between_its_nodes() {
        let d = ose_data::engine_shroud::def("engine_1v_methalox_swivel").unwrap();
        let (shape, def) = shroud_shape(d, 0.625);
        let (pos, _, tris) = piece_mesh(&shape, &def, 0);
        assert!(!tris.is_empty());
        let ys: Vec<f32> = pos.iter().map(|p| p[1]).collect();
        let lo = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        // KSP2's saved Swivel shroud: y 0.680 down to -1.000 (the bottom node). The prefab's static mesh,
        // drawn before, spanned -1.868..-0.133 and left the engine's top bare.
        assert!(
            (hi - 0.680).abs() < 2e-3 && (lo + 1.0).abs() < 2e-3,
            "{lo}..{hi}"
        );
        let r = pos
            .iter()
            .map(|p| (p[0] * p[0] + p[2] * p[2]).sqrt())
            .fold(0.0, f32::max);
        assert!((r - 0.625).abs() < 1e-3);
    }
}
