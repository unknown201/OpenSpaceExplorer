//! KSP2's procedural fairings (`Module_Fairing`, `Data_Fairing`): a wall of cross sections built up from a
//! fairing base, split into clamshell panels or a single shroud that come off with `EjectionForce`.
//!
//! - Panel mass: `MassAreaRatio` x the wall's area, each section a frustum
//!   (`FairingPanel.GetArea`, `Module_Fairing.UpdatePanelMass`); it leaves the craft with the panels.
//! - Shielding: parts inside the wall are out of the airflow, but only once the fairing is capped
//!   (`Module_Fairing.UpdateEnclosedParts` -> `ShieldEnclosedParts(IsCapped)`).
//! - Ejection: each panel gets `EjectionForce` x its mass for one physics step (`ForceMode.Force`), outward
//!   for clamshells and along the base's up axis for a shroud; nothing pushes back on the craft.
//!
//! Definitions come from `assets/own/procedural-fairings.json` (tools/export_procedural_fairings.py).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The original's physics step, s: one step of `EjectionForce` per panel tonne.
pub const PHYSICS_STEP_S: f64 = 0.02;

/// A fairing's saved shape (`Data_Fairing.CrossSections`, `IsCapped`, `DeployType`, `EjectionForce`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FairingShape {
    /// Wall cross sections from the base up: (height above the fairing node, radius), m. The first is the
    /// base ring at height 0.
    pub sections: Vec<(f64, f64)>,
    pub capped: bool,
    /// Clamshell panels (2, 3 or 4), or 0 for a shroud.
    pub pieces: u8,
    /// `EjectionForce`, kN per tonne of panel for one physics step.
    pub ejection_force: f64,
}

/// What a fairing base allows (`Data_Fairing` definition fields).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FairingDef {
    pub mass_t: f64,
    pub base_radius: f64,
    pub cap_radius: f64,
    pub max_radius: f64,
    pub mass_area_ratio: f64,
    pub height_min: f64,
    pub height_max: f64,
    pub side_count: u32,
    pub length_snap: f64,
    pub radius_snap: f64,
    pub ejection_force: f64,
    pub pieces: u8,
    /// Height of the fairing node (where the wall starts) on the part, m.
    pub base_height: f64,
}

static DEFS: OnceLock<BTreeMap<String, FairingDef>> = OnceLock::new();

pub fn def(part_id: &str) -> Option<&'static FairingDef> {
    DEFS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/own/procedural-fairings.json"))
            .expect("assets/own/procedural-fairings.json")
    })
    .get(part_id)
}

impl FairingShape {
    /// A straight wall of `height` at the base radius, closed with a cap (what a player gets on the first
    /// drag up from the base).
    pub fn cylinder(def: &FairingDef, height: f64) -> Self {
        let height = height.clamp(def.height_min, def.height_max);
        Self {
            sections: vec![
                (0.0, def.base_radius),
                (height, def.base_radius),
                (height + def.base_radius.min(def.height_max), def.cap_radius),
            ],
            capped: true,
            pieces: def.pieces,
            ejection_force: def.ejection_force,
        }
    }

    /// Sections climbing by at least the minimum and at most the maximum section height, radii within the
    /// base's limits, a known way to come apart.
    pub fn valid_for(&self, part_id: &str) -> bool {
        let Some(d) = def(part_id) else {
            return false;
        };
        let finite = self
            .sections
            .iter()
            .all(|(h, r)| h.is_finite() && r.is_finite() && *r >= 0.0 && *r <= d.max_radius + 1e-6);
        let first = self.sections.first().is_some_and(|s| s.0.abs() < 1e-6);
        let climbs = self.sections.windows(2).all(|w| {
            let step = w[1].0 - w[0].0;
            step >= d.height_min * 0.999 && step <= d.height_max * 1.001
        });
        (2..=32).contains(&self.sections.len())
            && finite
            && first
            && climbs
            && matches!(self.pieces, 0 | 2 | 3 | 4)
            && self.ejection_force.is_finite()
            && (0.0..=10_000.0).contains(&self.ejection_force)
    }

    /// The wall's area, m^2: a frustum per section (`FairingPanel.GetArea`, summed over the panels).
    pub fn area_m2(&self) -> f64 {
        self.sections
            .windows(2)
            .map(|w| {
                let (h, r0, r1) = (w[1].0 - w[0].0, w[0].1, w[1].1);
                std::f64::consts::PI * (r0 + r1) * ((r0 - r1).powi(2) + h * h).sqrt()
            })
            .sum()
    }

    /// The panels' mass, t (`MassModifierAmount`).
    pub fn panel_mass_t(&self, def: &FairingDef) -> f64 {
        self.area_m2() * def.mass_area_ratio
    }

    pub fn height(&self) -> f64 {
        self.sections.last().map_or(0.0, |s| s.0)
    }

    pub fn max_radius(&self) -> f64 {
        self.sections.iter().map(|s| s.1).fold(0.0, f64::max)
    }

    /// The wall's radius `h` above the base, `None` beyond either end.
    pub fn radius_at(&self, h: f64) -> Option<f64> {
        if h < 0.0 {
            return None;
        }
        self.sections.windows(2).find_map(|w| {
            (h >= w[0].0 && h <= w[1].0).then(|| {
                let t = (h - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
                w[0].1 + (w[1].1 - w[0].1) * t
            })
        })
    }

    /// Whether a box (centre and half extents in the fairing part's frame, the wall's axis +Y) fits inside.
    pub fn encloses(&self, def: &FairingDef, centre: [f64; 3], half: [f64; 3]) -> bool {
        let lo = centre[1] - half[1] - def.base_height;
        let hi = centre[1] + half[1] - def.base_height;
        let side = (centre[0] * centre[0] + centre[2] * centre[2]).sqrt() + half[0].max(half[2]);
        [lo.max(0.0), (lo + hi) * 0.5, hi]
            .iter()
            .all(|h| self.radius_at(*h).is_some_and(|r| side <= r + 0.05))
            && hi > 0.0
    }

    /// Frontal (end on) and side on areas of the closed shell, m^2, for its drag.
    pub fn drag_areas_m2(&self) -> (f64, f64) {
        let r = self.max_radius();
        let side: f64 = self
            .sections
            .windows(2)
            .map(|w| (w[1].0 - w[0].0) * (w[0].1 + w[1].1))
            .sum();
        (std::f64::consts::PI * r * r, side)
    }

    /// How fast each panel leaves, m/s: one physics step of `EjectionForce` per tonne.
    pub fn ejection_speed_mps(&self) -> f64 {
        self.ejection_force * PHYSICS_STEP_S
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_mass_is_the_wall_area_at_the_mass_area_ratio() {
        let d = def("fairing_1v_procedural").unwrap();
        // A 2 m cylinder at the 0.625 m base: 2 pi r h = 7.854 m^2 -> 0.0785 t at 0.01 t/m^2.
        let s = FairingShape {
            sections: vec![(0.0, 0.625), (2.0, 0.625)],
            capped: false,
            pieces: 2,
            ejection_force: 100.0,
        };
        assert!(s.valid_for("fairing_1v_procedural"));
        assert!((s.panel_mass_t(d) - 0.0785398).abs() < 1e-6);
        assert!((s.ejection_speed_mps() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_capped_cylinder_encloses_what_fits_and_not_what_sticks_out() {
        let d = def("fairing_2v_procedural").unwrap();
        let s = FairingShape::cylinder(d, 4.0);
        assert!(s.valid_for("fairing_2v_procedural"));
        let y = d.base_height + 1.0;
        assert!(s.encloses(d, [0.0, y, 0.0], [1.0, 0.8, 1.0]));
        assert!(
            !s.encloses(d, [0.0, y, 0.0], [1.4, 0.8, 1.4]),
            "wider than the wall"
        );
        assert!(
            !s.encloses(d, [0.0, y + 5.0, 0.0], [0.5, 0.5, 0.5]),
            "above the cap"
        );
    }

    #[test]
    fn shapes_outside_the_bases_limits_are_refused() {
        let d = def("fairing_1v_procedural").unwrap();
        let mut s = FairingShape::cylinder(d, 2.0);
        s.sections[1].1 = d.max_radius * 2.0;
        assert!(!s.valid_for("fairing_1v_procedural"));
        let mut s = FairingShape::cylinder(d, 2.0);
        s.sections[1].0 = d.height_max * 3.0;
        assert!(!s.valid_for("fairing_1v_procedural"));
        assert!(!FairingShape::cylinder(d, 2.0).valid_for("wing_1v_procedural_light"));
    }
}
