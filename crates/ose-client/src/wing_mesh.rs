//! Shaped procedural wings drawn with the part's own materials: the meshes `ose_data::procedural` bakes for a
//! shape, written as a small GLB that borrows the materials and images of `assets/parts/<id>.glb`, so the
//! renderer loads it like any part. The moving surface sits under its own node (`FLAP_NODE`) placed on the
//! hinge, so flight can turn it about `WingResult::ctrl_hinge`.

use ose_data::procedural::WingResult;
use serde_json::{json, Value};

/// The node holding a shaped wing's control surface, origin on the hinge, axes the part's.
pub const FLAP_NODE: &str = "shaped_control_surface";

/// Unity part frame -> glTF (X mirrored), as the part exports are written.
fn gltf(v: [f64; 3]) -> [f32; 3] {
    [-v[0] as f32, v[1] as f32, v[2] as f32]
}

/// The glTF hinge axis of a shaped wing's control surface (unit), for `Quat::from_axis_angle`.
pub fn hinge_axis(r: &WingResult) -> glam::Vec3 {
    glam::Vec3::from_array(gltf(r.ctrl_hinge)).normalize_or_zero()
}

/// A stable model name for `part_id` shaped as `key` (a shape's debug text), so equal shapes share a model.
pub fn model_name(part_id: &str, key: &str) -> String {
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    format!("wing:{part_id}:{hash:016x}")
}

fn floats<'a>(v: impl Iterator<Item = &'a f32>) -> Vec<u8> {
    v.flat_map(|f| f.to_le_bytes()).collect()
}

fn read_json(glb: &[u8]) -> Result<Value, String> {
    if glb.len() < 20 || &glb[0..4] != b"glTF" {
        return Err("not a GLB".into());
    }
    let len = u32::from_le_bytes(glb[12..16].try_into().map_err(|_| "header")?) as usize;
    serde_json::from_slice(glb.get(20..20 + len).ok_or("truncated GLB JSON")?)
        .map_err(|e| e.to_string())
}

/// Build the GLB of a shaped wing from the part's exported GLB `base` (for its materials).
pub fn shaped_glb(base: &[u8], r: &WingResult) -> Result<Vec<u8>, String> {
    let src = read_json(base)?;
    let material_of = |name: &str| -> Option<Value> {
        let meshes = src["meshes"].as_array()?;
        let m = meshes
            .iter()
            .find(|m| m["name"].as_str() == Some(name))
            .or_else(|| meshes.first())?;
        Some(m["primitives"][0]["material"].clone()).filter(|v| !v.is_null())
    };
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut push = |bytes: &[u8], target: u32, acc: Value, bin: &mut Vec<u8>| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len(), "target": target}));
        bin.extend_from_slice(bytes);
        let mut acc = acc;
        acc["bufferView"] = json!(views.len() - 1);
        accessors.push(acc);
        accessors.len() - 1
    };
    let mut meshes: Vec<Value> = Vec::new();
    let mut nodes: Vec<Value> = vec![json!({"name": "shaped_wing", "children": []})];
    let flap_origin = gltf(r.ctrl_pivot_m);
    let mut flap_children: Vec<usize> = Vec::new();
    for m in &r.meshes {
        if m.positions.is_empty() || m.triangles.len() < 3 {
            continue;
        }
        let flap = m.group == "CONTROLSURFACE";
        let origin = if flap { flap_origin } else { [0.0; 3] };
        let pos: Vec<[f32; 3]> = m
            .positions
            .iter()
            .map(|p| {
                let g = gltf(*p);
                [g[0] - origin[0], g[1] - origin[1], g[2] - origin[2]]
            })
            .collect();
        // Mirroring X turns Unity's clockwise faces counter-clockwise: reverse each triangle.
        let tris: Vec<u32> = m
            .triangles
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|t| [t[0], t[2], t[1]])
            .filter(|i| (*i as usize) < pos.len())
            .collect();
        let tris = &tris[..tris.len() / 3 * 3];
        // `RecalculateNormals` after the bake: area-weighted face normals summed per vertex.
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
        let uvs: Vec<[f32; 2]> = (0..pos.len())
            .map(|i| m.uvs.get(i).map_or([0.0, 0.0], |u| [u[0], 1.0 - u[1]]))
            .collect();
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let p_acc = push(
            &floats(pos.iter().flatten()),
            34962,
            json!({"componentType": 5126, "count": pos.len(), "type": "VEC3", "min": lo, "max": hi}),
            &mut bin,
        );
        let n_acc = push(
            &floats(normals.iter().flatten()),
            34962,
            json!({"componentType": 5126, "count": normals.len(), "type": "VEC3"}),
            &mut bin,
        );
        let t_acc = push(
            &floats(uvs.iter().flatten()),
            34962,
            json!({"componentType": 5126, "count": uvs.len(), "type": "VEC2"}),
            &mut bin,
        );
        let i_acc = push(
            &tris
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<u8>>(),
            34963,
            json!({"componentType": 5125, "count": tris.len(), "type": "SCALAR"}),
            &mut bin,
        );
        let mut prim = json!({"attributes": {"POSITION": p_acc, "NORMAL": n_acc, "TEXCOORD_0": t_acc}, "indices": i_acc});
        if let Some(mat) = material_of(&m.name) {
            prim["material"] = mat;
        }
        meshes.push(json!({"name": m.name, "primitives": [prim]}));
        nodes.push(json!({"name": m.name, "mesh": meshes.len() - 1}));
        let node = nodes.len() - 1;
        if flap {
            flap_children.push(node);
        } else {
            nodes[0]["children"]
                .as_array_mut()
                .expect("children")
                .push(json!(node));
        }
    }
    if !flap_children.is_empty() {
        nodes.push(
            json!({"name": FLAP_NODE, "translation": flap_origin, "children": flap_children}),
        );
        let flap = nodes.len() - 1;
        nodes[0]["children"]
            .as_array_mut()
            .expect("children")
            .push(json!(flap));
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "ose wing_mesh"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": nodes,
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    for key in ["materials", "textures", "images", "samplers"] {
        if let Some(v) = src.get(key) {
            doc[key] = v.clone();
        }
    }
    let mut text = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    Ok(out)
}

/// The shape a procedural wing part draws with: its own, or the authored start shape.
pub fn shape_of(p: &ose_data::craft::CraftPart) -> Option<ose_data::procedural::WingShape> {
    let rig = ose_data::procedural::rig(&p.part_id)?;
    Some(p.shape.clone().unwrap_or(ose_data::procedural::WingShape {
        option: rig.options.len().saturating_sub(1) as u32,
        ..Default::default()
    }))
}

/// The model to draw part `p` with: a procedural wing's shaped model once built (asked for here), else the
/// part's own export. Shaped models' names go into `used` so stale shapes can be dropped.
#[cfg(target_arch = "wasm32")]
pub fn model_for(
    gpu: &mut crate::render::Renderer,
    p: &ose_data::craft::CraftPart,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let part = format!("part:{}", p.part_id);
    gpu.request_model(&part, &format!("assets/parts/{}.glb", p.part_id));
    let Some(shape) = shape_of(p) else {
        return part;
    };
    let mirrored = p.flight.as_ref().is_some_and(|f| f.mirrored);
    let name = model_name(&p.part_id, &format!("{shape:?}{mirrored}"));
    if let Some(r) = ose_data::procedural::evaluate_cached(&p.part_id, &shape, mirrored) {
        gpu.request_built_model(
            &name,
            &format!("assets/parts/{}.glb", p.part_id),
            move |base| shaped_glb(base, &r),
        );
    }
    used.insert(name.clone());
    if gpu.has_model(&name) {
        name
    } else {
        part
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shaped_wing_glb_parses_with_its_flap_under_the_hinge_node() {
        let base = shaped_glb(&minimal_base(), &evaluate()).unwrap();
        let doc = read_json(&base).unwrap();
        let names: Vec<&str> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(names.contains(&FLAP_NODE));
        assert!(names.contains(&"mesh_wing_main"));
        assert_eq!(doc["materials"][0]["name"], "wing");
        assert_eq!(doc["meshes"][0]["primitives"][0]["material"], 0);
    }

    fn evaluate() -> WingResult {
        ose_data::procedural::evaluate(
            "wing_1v_procedural_light",
            &ose_data::procedural::WingShape {
                option: 1,
                ..Default::default()
            },
            false,
        )
        .unwrap()
    }

    fn minimal_base() -> Vec<u8> {
        let doc = json!({"asset": {"version": "2.0"}, "materials": [{"name": "wing"}],
            "meshes": [{"name": "mesh_wing_main", "primitives": [{"attributes": {}, "material": 0}]}]});
        let mut text = serde_json::to_vec(&doc).unwrap();
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let mut out = b"glTF".to_vec();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((20 + text.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        out
    }
}
