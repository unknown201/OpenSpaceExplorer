//! Headless check: load the converted game data and print where the bodies are at a few universal times.
//!
//! cargo run -p ose-sim --example kerbol -- assets/ksp2/converted/game_data.json

use bevy_ecs::world::World;
use ose_sim::bodies::{relative_position, spawn_bodies, star_space_state};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/ksp2/converted/game_data.json".into());
    let text =
        std::fs::read_to_string(&path).expect("read game_data.json (run ose-importer first)");
    let game: ose_data::GameData = serde_json::from_str(&text).expect("parse game data");
    let mut world = World::new();
    let index = spawn_bodies(&mut world, &game.bodies);
    let kerbin = index.by_id["Kerbin"];
    for ut in [0.0, 3600.0, 86_400.0, 9_203_545.0] {
        println!("UT {ut:>12.0} s");
        let mut ids: Vec<_> = index.by_id.keys().collect();
        ids.sort();
        for id in ids {
            let e = index.by_id[id];
            let (p, v) = star_space_state(&world, e, ut);
            let rk = relative_position(&world, e, kerbin, ut).length();
            println!(
                "  {id:<18} r_star={:>14.0} m  v={:>9.1} m/s  dist_from_kerbin={:>14.0} m",
                p.length(),
                v.length(),
                rk
            );
        }
    }
}
