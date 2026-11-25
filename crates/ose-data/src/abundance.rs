//! Where Ore lies (`docs/colonies-design.md`, in-situ resources). KSP2 planned resource abundance by body,
//! latitude, longitude and biome (`AbundanceRequest`, `PlanetaryResource`) but shipped no data for it, so
//! OSE makes its own: a smooth field over each body's surface, the same everywhere for everyone, made from a
//! few waves seeded by the body's name. Rich patches cover about a fifth of a rocky world; gas giants and
//! the sun have none, and Kerbin, mined out near home, has little.

/// The bodies with no ground to drill.
const NO_GROUND: [&str; 2] = ["Kerbol", "Jool"];
/// Kerbin's share of what the field would give.
const HOME_SHARE: f64 = 0.25;

fn seed(body: &str) -> u64 {
    // FNV-1a: stable across builds and platforms
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in body.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn next(state: &mut u64) -> f64 {
    // splitmix64
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Ore in the ground at a place on a body, 0 (none) to 1 (richest).
pub fn ore(body: &str, lat_deg: f64, lon_deg: f64) -> f64 {
    if NO_GROUND.contains(&body) {
        return 0.0;
    }
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let p = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
    let mut state = seed(body);
    const WAVES: usize = 8;
    let mut sum = 0.0;
    for _ in 0..WAVES {
        // a random direction, frequency and phase per wave
        let z = 2.0 * next(&mut state) - 1.0;
        let a = std::f64::consts::TAU * next(&mut state);
        let r = (1.0 - z * z).sqrt();
        let d = [r * a.cos(), r * a.sin(), z];
        let k = 2.0 + 6.0 * next(&mut state);
        let phase = std::f64::consts::TAU * next(&mut state);
        sum += (k * (p[0] * d[0] + p[1] * d[1] + p[2] * d[2]) + phase).sin();
    }
    // the sum spreads about ±2 (sigma = 2); only its upper reach is rich ground
    let x = sum / 2.0;
    let field = ((x - 0.4) / 1.6).clamp(0.0, 1.0);
    if body == "Kerbin" {
        field * HOME_SHARE
    } else {
        field
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ore_is_patchy_on_rocky_worlds_and_absent_on_gas_giants() {
        assert_eq!(ore("Jool", 10.0, 20.0), 0.0);
        let mut rich = 0;
        let mut any = 0;
        let mut max: f64 = 0.0;
        let n = 4000;
        for i in 0..n {
            let lat = -80.0 + 160.0 * ((i * 37) % n) as f64 / n as f64;
            let lon = -180.0 + 360.0 * i as f64 / n as f64;
            let a = ore("Mun", lat, lon);
            assert!((0.0..=1.0).contains(&a));
            any += (a > 0.0) as usize;
            rich += (a > 0.3) as usize;
            max = max.max(a);
        }
        let share = any as f64 / n as f64;
        assert!(
            (0.1..0.6).contains(&share),
            "ore under {share:.2} of the Mun"
        );
        assert!(rich > 0 && max > 0.5, "rich {rich}, max {max}");
        // smooth: a step of 100 m on the Mun changes little
        let (a, b) = (ore("Mun", 3.0, 40.0), ore("Mun", 3.0, 40.0 + 0.03));
        assert!((a - b).abs() < 0.05);
        // the same for everyone, different between bodies, and poor at home
        assert_eq!(ore("Minmus", 1.0, 2.0), ore("Minmus", 1.0, 2.0));
        assert_ne!(ore("Minmus", 1.0, 2.0), ore("Mun", 1.0, 2.0));
        assert!((0..400).all(|i| ore("Kerbin", 0.0, i as f64 * 0.9) <= HOME_SHARE));
    }
}
