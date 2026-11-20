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

/// KSP2's `FairingsProceduralMat` (proceduralparts-shared_assets_all, shader OAB_Procedural_Fairings) as a
/// glTF material: near-white albedo, fully metallic with smoothness from its gloss map, flat normals. Its
/// paint mask is full on both channels; the base colour tint comes from `shroud_item`.
fn procedural_fairing_material() -> Value {
    json!({
        "materials": [{
            "name": "FairingsProceduralMat",
            "pbrMetallicRoughness": {
                "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
                "baseColorTexture": {"index": 0},
                "metallicFactor": 1.0,
                "roughnessFactor": 0.8,
                "metallicRoughnessTexture": {"index": 1},
            },
            "normalTexture": {"index": 2},
        }, {
            "name": "FairingsProceduralMat_gutter",
            "pbrMetallicRoughness": {
                "baseColorFactor": [GUTTER_GREY, GUTTER_GREY, GUTTER_GREY, 1.0],
                "metallicFactor": 0.0,
                "roughnessFactor": 0.8,
            },
        }],
        "textures": [
            {"sampler": 0, "source": 0},
            {"sampler": 0, "source": 1},
            {"sampler": 0, "source": 2},
        ],
        "images": [
            {"uri": "../textures/fairing_procedural_d.png", "name": "fairing_procedural_d"},
            {"uri": "../textures/fairing_procedural_m.png", "name": "fairing_procedural_m"},
            {"uri": "../textures/fairing_procedural_n.png", "name": "fairing_procedural_n"},
        ],
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
    })
}

/// `_GutterColor` of `FairingsProceduralMat`: the seam lines along a panel's edges.
const GUTTER_GREY: f64 = 0.36792;
/// `_FairingGutterWidth`, in texture v (3 m of slant per unit): the seam band's width up the wall.
const GUTTER_WIDTH_V: f64 = 0.008;

/// Slant length up the wall at each section, m, from the base ring.
fn section_slant(shape: &FairingShape) -> Vec<f64> {
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(shape.sections.len());
    for (i, (h, r)) in shape.sections.iter().enumerate() {
        if i > 0 {
            let (h0, r0) = shape.sections[i - 1];
            acc += ((h - h0).powi(2) + (r - r0).powi(2)).sqrt();
        }
        out.push(acc);
    }
    out
}

/// Panel `k`'s texture coordinates as `FairingPanel.SetArcPoints` lays them: u runs a ninth of the texture
/// across each panel (`(panel + t) * 0.11111`), v is the slant length up the wall in metres over 3; a capped
/// wall's apex sits a last ring's radius further up.
pub fn panel_uvs(shape: &FairingShape, def: &FairingDef, k: usize) -> Vec<[f32; 2]> {
    let (pos, uv, _) = piece_mesh(shape, def, k);
    let slant = section_slant(shape);
    let w = uv.len() / shape.sections.len().max(1);
    let apex = slant.last().copied().unwrap_or(0.0) + shape.sections.last().map_or(0.0, |s| s.1);
    uv.iter()
        .enumerate()
        .map(|(i, t)| {
            let row = i / w.max(1);
            let v = slant.get(row).copied().unwrap_or(apex);
            [((k as f32) + t[0]) / 9.0, (v / 3.0) as f32]
        })
        .take(pos.len())
        .collect()
}

/// A thin strip a hair outside the wall over `rows` (height above the base, radius), between the angles
/// `arc(radius)` gives, in the part's glTF frame as `piece_mesh` lays the wall.
fn strip(
    rows: &[(f64, f64)],
    def: &FairingDef,
    arc: impl Fn(f64) -> (f64, f64),
    steps: usize,
) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    for (h, r) in rows {
        let (a0, a1) = arc(*r);
        let r = r + 0.004;
        for s in 0..=steps {
            let a = a0 + (a1 - a0) * s as f64 / steps as f64;
            pos.push([
                -(r * a.cos()) as f32,
                (def.base_height + h) as f32,
                (r * a.sin()) as f32,
            ]);
        }
    }
    let w = (steps + 1) as u32;
    for row in 0..rows.len().saturating_sub(1) as u32 {
        for s in 0..steps as u32 {
            let (a, b) = (row * w + s, row * w + s + 1);
            let (c, d) = (a + w, b + w);
            tris.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    if let Some(t) = tris.get(0..3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| glam::Vec3::from_array(pos[i as usize]));
        let n = (b - a).cross(c - a);
        if n.dot(glam::Vec3::new(a.x, 0.0, a.z)) < 0.0 {
            for t in tris.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
    }
    (pos, tris)
}

/// Panel `k`'s seam lines (`UpdateMeshGutters`): rings at the bottom edge, at the top edge of an open wall
/// and every 3 m of slant (each whole v), and on a clamshell the two vertical edges where it parts. A shroud
/// is unbounded in u, so it has no vertical seams.
pub fn panel_gutters(
    shape: &FairingShape,
    def: &FairingDef,
    k: usize,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    let n = piece_count(shape);
    let (a0, a1) = (
        k as f64 / n as f64 * std::f64::consts::TAU,
        (k + 1) as f64 / n as f64 * std::f64::consts::TAU,
    );
    let width = GUTTER_WIDTH_V * 3.0;
    let slant = section_slant(shape);
    let total = slant.last().copied().unwrap_or(0.0);
    // the wall's (height, radius) at slant length `t`
    let at = |t: f64| -> (f64, f64) {
        let t = t.clamp(0.0, total);
        let i = slant
            .partition_point(|s| *s < t)
            .clamp(1, slant.len().max(2) - 1);
        let (s0, s1) = (slant[i - 1], slant[i]);
        let f = if s1 > s0 { (t - s0) / (s1 - s0) } else { 0.0 };
        let (p, q) = (shape.sections[i - 1], shape.sections[i]);
        (p.0 + (q.0 - p.0) * f, p.1 + (q.1 - p.1) * f)
    };
    let steps = (def.side_count as usize).max(8) / n.max(1);
    let mut bands: Vec<(f64, f64)> = vec![(0.0, width)];
    let mut m = 3.0;
    while m < total - width {
        bands.push((m - width * 0.5, m + width * 0.5));
        m += 3.0;
    }
    if !shape.capped {
        bands.push((total - width, total));
    }
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    let mut add = |(p, t): (Vec<[f32; 3]>, Vec<u32>)| {
        let off = pos.len() as u32;
        pos.extend(p);
        tris.extend(t.iter().map(|i| i + off));
    };
    for (lo, hi) in bands {
        add(strip(&[at(lo), at(hi)], def, |_| (a0, a1), steps.max(2)));
    }
    if shape.pieces > 0 {
        let rows: Vec<(f64, f64)> = shape.sections.clone();
        add(strip(&rows, def, |r| (a0, a0 + width / r.max(0.05)), 1));
        add(strip(&rows, def, |r| (a1 - width / r.max(0.05), a1), 1));
    }
    let uv = vec![[0.0, 0.0]; pos.len()];
    (pos, uv, tris)
}

/// Panel `k` as a GLB in KSP2's procedural fairing material, its seams in the gutter grey.
pub fn panel_glb(shape: &FairingShape, def: &FairingDef, k: usize) -> Vec<u8> {
    let (pos, _, tris) = piece_mesh(shape, def, k);
    let uv = panel_uvs(shape, def, k);
    let (gp, gu, gt) = panel_gutters(shape, def, k);
    prims_glb(
        &[(&pos, &uv, &tris, Some(0)), (&gp, &gu, &gt, Some(1))],
        k,
        &procedural_fairing_material(),
    )
}

/// The shroud's seam rings at its top and bottom edges.
pub fn shroud_gutters(
    shape: &FairingShape,
    def: &FairingDef,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    panel_gutters(shape, def, 0)
}

/// The shroud's texture coordinates (`panel_uvs` of its one panel).
pub fn shroud_uvs(shape: &FairingShape, def: &FairingDef) -> Vec<[f32; 2]> {
    panel_uvs(shape, def, 0)
}

/// An engine's shroud as a GLB in KSP2's procedural fairing material.
pub fn shroud_glb(shape: &FairingShape, def: &FairingDef) -> Vec<u8> {
    panel_glb(shape, def, 0)
}

/// A GLB of one node whose mesh holds `prims` (positions, uvs, triangles, material index).
type Prim<'a> = (&'a [[f32; 3]], &'a [[f32; 2]], &'a [u32], Option<usize>);

fn prims_glb(prims: &[Prim], k: usize, src: &Value) -> Vec<u8> {
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
    let mut primitives = Vec::new();
    for (pos, uv, tris, material) in prims {
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
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in pos.iter() {
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
        let mut prim =
            json!({"attributes": {"POSITION": p, "NORMAL": nn, "TEXCOORD_0": t}, "indices": i});
        if let Some(m) = material {
            prim["material"] = json!(m);
        }
        primitives.push(prim);
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "ose fairing_mesh"},
        "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"name": format!("fairing_panel_{k}"), "mesh": 0}],
        "meshes": [{"name": format!("fairing_panel_{k}"), "primitives": primitives}],
        "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": bin.len()}],
    });
    for key in ["materials", "textures", "images", "samplers"] {
        if let Some(v) = src.get(key) {
            doc[key] = v.clone();
        }
    }
    crate::wing_mesh::glb_bytes(&doc, &bin)
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
        gpu.request_built_model(name, BASE_URL, move |_| Ok(panel_glb(&s, &d, k)));
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
        start_height: 0.0,
        nose_tip: 0.5,
        base_cube: None,
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
    // The base GLB is only fetched to drive the build; the shroud carries its own material.
    gpu.request_built_model(&name, BASE_URL, move |_| Ok(shroud_glb(&shape, &def)));
    gpu.has_model(&name).then_some(name)
}

/// The shroud drawn beside its engine's draw item: the engine's pose and look, the wall tinted in the vessel's
/// base colour (`Module_Color` paints `FairingsProceduralMat`, whose paint mask covers the whole wall).
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

    #[test]
    fn shroud_uses_ksp2_procedural_fairing_material_and_uvs() {
        let d = ose_data::engine_shroud::def("engine_1v_methalox_swivel").unwrap();
        let (shape, def) = shroud_shape(d, 0.625);
        let uv = shroud_uvs(&shape, &def);
        // u spans a ninth of the texture round the shroud, v the 1.68 m slant over 3
        let umax = uv.iter().map(|t| t[0]).fold(0.0, f32::max);
        let vmax = uv.iter().map(|t| t[1]).fold(0.0, f32::max);
        assert!((umax - 1.0 / 9.0).abs() < 1e-5, "{umax}");
        assert!((vmax - 1.6801 / 3.0).abs() < 1e-3, "{vmax}");
        let glb = shroud_glb(&shape, &def);
        let doc = crate::wing_mesh::read_json(&glb).unwrap();
        assert_eq!(doc["materials"][0]["name"], "FairingsProceduralMat");
        assert_eq!(
            doc["images"][0]["uri"],
            "../textures/fairing_procedural_d.png"
        );
        assert_eq!(doc["meshes"][0]["primitives"][0]["material"], 0);
        // seam rings at the top and bottom edges, in the gutter grey
        assert_eq!(doc["meshes"][0]["primitives"][1]["material"], 1);
        let (gp, _, gt) = shroud_gutters(&shape, &def);
        assert!(!gt.is_empty());
        let ys: Vec<f32> = gp.iter().map(|p| p[1]).collect();
        assert!(
            ys.iter().any(|y| (y + 1.0).abs() < 1e-3)
                && ys.iter().any(|y| (y - 0.6801).abs() < 1e-3)
        );
        assert!(ys
            .iter()
            .all(|y| (y + 1.0).abs() < 0.03 || (y - 0.6801).abs() < 0.03));
    }

    #[test]
    fn payload_panels_use_ksp2_procedural_fairing_material_and_seams() {
        let d = ose_data::fairing::def("fairing_1v_procedural").unwrap();
        let s = FairingShape::cylinder(d, d.height_max.min(7.0));
        assert!(s.pieces > 0 && s.capped);
        let k = 1;
        // u runs a ninth of the texture per panel, offset by the panel's index (FairingPanel.SetArcPoints)
        let uv = panel_uvs(&s, d, k);
        let umin = uv.iter().map(|t| t[0]).fold(f32::INFINITY, f32::min);
        let umax = uv.iter().map(|t| t[0]).fold(0.0, f32::max);
        assert!((umin - 1.0 / 9.0).abs() < 1e-5 && (umax - 2.0 / 9.0).abs() < 1e-5);
        // v is the slant up the wall over 3, the apex a last ring's radius past the top section
        let slant = section_slant(&s);
        let vmax = uv.iter().map(|t| t[1]).fold(0.0, f32::max);
        let apex = slant.last().unwrap() + s.sections.last().unwrap().1;
        assert!((vmax as f64 - apex / 3.0).abs() < 1e-4, "{vmax}");
        let glb = panel_glb(&s, d, k);
        let doc = crate::wing_mesh::read_json(&glb).unwrap();
        assert_eq!(doc["materials"][0]["name"], "FairingsProceduralMat");
        assert_eq!(doc["meshes"][0]["primitives"][0]["material"], 0);
        assert_eq!(doc["meshes"][0]["primitives"][1]["material"], 1);
        // seams: a ring at the base, one every 3 m of slant, and the panel's two parting edges
        let (gp, _, _) = panel_gutters(&s, d, k);
        let ys: Vec<f32> = gp.iter().map(|p| p[1] - def_base(d)).collect();
        assert!(ys.iter().any(|y| y.abs() < 1e-3));
        if *slant.last().unwrap() > 3.1 {
            assert!(ys.iter().any(|y| (y - 3.0).abs() < 0.02), "no 3 m seam");
        }
        let n = piece_count(&s) as f32;
        let ang =
            |p: &[f32; 3]| (p[2].atan2(-p[0]) + std::f32::consts::TAU) % std::f32::consts::TAU;
        let (a0, a1) = (
            k as f32 / n * std::f32::consts::TAU,
            (k + 1) as f32 / n * std::f32::consts::TAU,
        );
        assert!(gp.iter().any(|p| (ang(p) - a0).abs() < 1e-3));
        assert!(gp.iter().any(|p| (ang(p) - a1).abs() < 1e-3));
        // every seam sits a hair outside the wall and inside the panel's arc
        assert!(gp
            .iter()
            .all(|p| ang(p) >= a0 - 1e-3 && ang(p) <= a1 + 1e-3));
    }

    fn def_base(d: &FairingDef) -> f32 {
        d.base_height as f32
    }
}
