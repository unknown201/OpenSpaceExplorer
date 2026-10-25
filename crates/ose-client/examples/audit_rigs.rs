//! Audit locally exported (gitignored) rigs without a browser or GPU.
use glam::Vec3;
use ose_client::animation::{skin_vertex, Accessors, Rig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .ok_or("usage: audit_rigs <parts-directory>")?;
    let mut count = 0;
    for file in std::fs::read_dir(directory)? {
        let path = file?.path();
        if path.extension().is_none_or(|e| e != "glb") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let len = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
        let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + len])?;
        let bin = &bytes[28 + len..];
        let rig = Rig::from_gltf(&json, bin).map_err(|e| format!("{}: {e}", path.display()))?;
        let read = Accessors { json: &json, bin };
        let bounds = |fraction| -> Result<(Vec3, Vec3), String> {
            let world = rig.world_matrices(&rig.pose(rig.deployment_sample(fraction)));
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for (i, node) in rig
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.visible && n.mesh.is_some())
            {
                let palette = rig.palette(i, &world);
                for p in json["meshes"][node.mesh.unwrap()]["primitives"]
                    .as_array()
                    .unwrap()
                {
                    let a = &p["attributes"];
                    let positions = read.floats(a["POSITION"].as_u64().unwrap() as usize, 3)?;
                    let skin = if let Some(j) = a["JOINTS_0"].as_u64() {
                        Some((
                            read.floats(j as usize, 4)?,
                            read.floats(a["WEIGHTS_0"].as_u64().unwrap() as usize, 4)?,
                        ))
                    } else {
                        None
                    };
                    for (v, p) in positions.as_chunks::<3>().0.iter().enumerate() {
                        let mut pos = Vec3::from_slice(p);
                        if let (Some(palette), Some((j, w))) = (&palette, &skin) {
                            pos = skin_vertex(
                                pos,
                                Vec3::Y,
                                [
                                    j[v * 4] as u16,
                                    j[v * 4 + 1] as u16,
                                    j[v * 4 + 2] as u16,
                                    j[v * 4 + 3] as u16,
                                ],
                                w[v * 4..v * 4 + 4].try_into().unwrap(),
                                palette,
                            )
                            .0;
                        }
                        let pos = world[i].transform_point3(pos);
                        if !pos.is_finite() {
                            return Err("nonfinite skinned position".into());
                        }
                        lo = lo.min(pos);
                        hi = hi.max(pos);
                    }
                }
            }
            Ok((lo, hi))
        };
        let (a, b) = bounds(0.0)?;
        let (c, d) = bounds(1.0)?;
        let (mid_lo, mid_hi) = bounds(0.5)?;
        if !mid_lo.is_finite() || !mid_hi.is_finite() {
            return Err("invalid bounds".into());
        }
        println!("{}: nodes={} skins={} clips={} deployment={:?} bounds_delta={:.4} closed={:?} open={:?}",
            path.file_stem().unwrap().to_string_lossy(), rig.nodes.len(), rig.skins.len(), rig.clips.len(),rig.deployment,
            (a-c).length()+(b-d).length(),b-a,d-c);
        count += 1;
    }
    println!("Audited {count} rigs");
    Ok(())
}
