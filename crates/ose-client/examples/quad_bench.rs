//! Times quad builds against the real Kerbin terrain pack (assets/web/terrain, not in git).
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/terrain");
    let text = std::fs::read_to_string(dir.join("Kerbin.json")).expect("pack");
    let blob = std::fs::read(dir.join("Kerbin.bin")).expect("blob");
    let pack: ose_data::terrain::TerrainPack = serde_json::from_str(&text).unwrap();
    let map = ose_data::terrain::TerrainMap::from_pack(&pack, &blob).unwrap();
    let r = 600_000.0;
    let cam =
        ose_client::planet_lod::scale(ose_client::planet_lod::direction(-0.68, -74.8), r + 500.0);
    let (_, wanted) = ose_client::planet_lod::select(cam, r, -2000.0, 6000.0, &|_| false);
    let t0 = std::time::Instant::now();
    let n = wanted.len().min(40);
    for (_, k) in wanted.iter().take(n) {
        let _ = ose_client::planet_lod::build_quad(k, r, true, &|d| {
            ose_client::planet_lod::map_height(Some(&map), d)
        });
    }
    println!("{} quads, {:?} per quad", n, t0.elapsed() / n as u32);
}
