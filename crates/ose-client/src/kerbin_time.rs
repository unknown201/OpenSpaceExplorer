//! Universal time as the original game shows it: Kerbin days of 6 hours, years of 426 days, both 1-based.

pub const DAY_S: f64 = 6.0 * 3600.0;
pub const YEAR_S: f64 = 426.0 * DAY_S;

/// `Y1 D1 00:00:00` for UT 0.
pub fn format_ut(ut: f64) -> String {
    let ut = ut.max(0.0);
    let year = (ut / YEAR_S).floor();
    let rem = ut - year * YEAR_S;
    let day = (rem / DAY_S).floor();
    let rem = rem - day * DAY_S;
    let h = (rem / 3600.0).floor();
    let rem = rem - h * 3600.0;
    let m = (rem / 60.0).floor();
    let s = (rem - m * 60.0).floor();
    format!(
        "Y{} D{} {:02}:{:02}:{:02}",
        year as u64 + 1,
        day as u64 + 1,
        h as u64,
        m as u64,
        s as u64
    )
}

/// Warp factors the UI steps through; the same ladder as the original's rails warp.
pub const WARP_LADDER: [u32; 11] = [1, 2, 3, 4, 5, 10, 50, 100, 1_000, 10_000, 100_000];

pub fn next_warp(current: u32) -> u32 {
    WARP_LADDER
        .iter()
        .copied()
        .find(|&w| w > current)
        .unwrap_or(WARP_LADDER[WARP_LADDER.len() - 1])
}

pub fn prev_warp(current: u32) -> u32 {
    WARP_LADDER
        .iter()
        .rev()
        .copied()
        .find(|&w| w < current)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_original() {
        assert_eq!(format_ut(0.0), "Y1 D1 00:00:00");
        assert_eq!(format_ut(61.0), "Y1 D1 00:01:01");
        assert_eq!(format_ut(DAY_S), "Y1 D2 00:00:00");
        assert_eq!(format_ut(YEAR_S + 3.0 * 3600.0), "Y2 D1 03:00:00");
    }

    #[test]
    fn warp_ladder_steps() {
        assert_eq!(next_warp(1), 2);
        assert_eq!(next_warp(5), 10);
        assert_eq!(next_warp(100_000), 100_000);
        assert_eq!(prev_warp(10), 5);
        assert_eq!(prev_warp(1), 1);
        assert_eq!(next_warp(7), 10);
    }
}
