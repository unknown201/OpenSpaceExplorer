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

/// The clock as the original's time warp widget writes it: `T+001y,001d,00:00:00` for UT 0.
pub fn format_ut_clock(ut: f64) -> String {
    let ut = ut.max(0.0);
    let year = (ut / YEAR_S).floor();
    let rem = ut - year * YEAR_S;
    let day = (rem / DAY_S).floor();
    let rem = rem - day * DAY_S;
    let (h, m, s) = (
        (rem / 3600.0).floor(),
        (rem % 3600.0 / 60.0).floor(),
        (rem % 60.0).floor(),
    );
    format!(
        "T+{:03}y,{:03}d,{:02}:{:02}:{:02}",
        year as u64 + 1,
        day as u64 + 1,
        h as u64,
        m as u64,
        s as u64
    )
}

/// Warp factors the UI steps through: the original's `TimeWarp` ladder (the first three are physics warp,
/// which the sim caps at 4x while a vessel is under power or in the air).
pub const WARP_LADDER: [u32; 10] = [1, 2, 4, 10, 50, 100, 1_000, 10_000, 100_000, 1_000_000];

pub fn next_warp(current: u32) -> u32 {
    WARP_LADDER
        .iter()
        .copied()
        .find(|&w| w > current)
        .unwrap_or(WARP_LADDER[WARP_LADDER.len() - 1])
}

/// The warp to ask for with `remaining_s` of game time left before a warp-to point: as the original, the
/// slowest rate that gets there within ten real seconds.
pub fn warp_toward(remaining_s: f64) -> u32 {
    WARP_LADDER
        .iter()
        .copied()
        .find(|&w| remaining_s / w as f64 <= 10.0)
        .unwrap_or(WARP_LADDER[WARP_LADDER.len() - 1])
}

/// The original's automatic drop to 1x: at rate `w`, once the point is (w + 1) / 2 game seconds away.
pub fn warp_drop(rate: u32, remaining_s: f64) -> bool {
    remaining_s <= (rate as f64 + 1.0) / 2.0
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
    #[test]
    fn clock_reads_like_the_originals_widget() {
        assert_eq!(super::format_ut_clock(0.0), "T+001y,001d,00:00:00");
        assert_eq!(
            super::format_ut_clock(super::YEAR_S + 2.0 * super::DAY_S + 3723.0),
            "T+002y,003d,01:02:03"
        );
    }

    use super::*;

    #[test]
    fn warp_steps_down_toward_the_point() {
        assert_eq!(warp_toward(5.0), 1);
        assert_eq!(warp_toward(30.0), 4);
        assert_eq!(warp_toward(500.0), 50);
        assert_eq!(warp_toward(1.0e7), 1_000_000);
        assert!(warp_drop(50, 25.5) && !warp_drop(50, 25.501));
    }

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
        assert_eq!(next_warp(2), 4);
        assert_eq!(next_warp(4), 10);
        assert_eq!(next_warp(100_000), 1_000_000);
        assert_eq!(next_warp(1_000_000), 1_000_000);
        assert_eq!(prev_warp(10), 4);
        assert_eq!(prev_warp(1), 1);
        assert_eq!(next_warp(7), 10);
    }
}
