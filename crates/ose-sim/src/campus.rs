//! A flat built site on a body (the KSC campus): the client draws it rigidly from its scene, anchored on a
//! point of the planet and turned to fit, and the planet's terrain is sunk under it. Craft standing there
//! stand on the scene's surfaces: each area is a rectangle of the scene's horizontal plane at its own
//! height, and the ground under a body-fixed direction is where that direction meets it.

use ose_math::DVec3;

/// One flat area of the site, in scene coordinates (x, z ranges, surface height y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
    pub y: f64,
}

/// A site placed on a body: scene point `anchor` sits at (`lat_deg`, `lon_deg`, `alt_m`), the scene's +X
/// turned `yaw_rad` from east toward north, its +Y up, its -Z north of +X (glTF axes).
#[derive(Clone, Debug, PartialEq)]
pub struct Site {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
    pub yaw_rad: f64,
    pub anchor: DVec3,
    /// Earlier areas win where they overlap.
    pub areas: Vec<Area>,
}

/// Unit body-fixed direction of a latitude/longitude (Z the spin axis).
pub fn direction(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (la, lo) = (lat_deg.to_radians(), lon_deg.to_radians());
    DVec3::new(la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin())
}

impl Site {
    /// The site's frame in body-fixed axes: origin (the anchor), scene +X, up, and scene -Z.
    fn frame(&self, radius_m: f64) -> (DVec3, DVec3, DVec3, DVec3) {
        let up = direction(self.lat_deg, self.lon_deg);
        let (la, lo) = (self.lat_deg.to_radians(), self.lon_deg.to_radians());
        let east = DVec3::new(-lo.sin(), lo.cos(), 0.0);
        let north = DVec3::new(-la.sin() * lo.cos(), -la.sin() * lo.sin(), la.cos());
        let (s, c) = self.yaw_rad.sin_cos();
        let ex = east * c + north * s;
        let en = -east * s + north * c;
        (up * (radius_m + self.alt_m), ex, up, en)
    }

    /// Distance from the body centre to the site's ground along body-fixed direction `u` (unit), when `u`
    /// passes over one of its areas.
    pub fn ground_radius(&self, radius_m: f64, u: DVec3) -> Option<f64> {
        let (o, ex, up, en) = self.frame(radius_m);
        let cos = u.dot(up);
        if cos <= 0.5 {
            return None;
        }
        for a in &self.areas {
            // where `u` meets the plane at scene height `a.y`
            let r = (o.dot(up) + (a.y - self.anchor.y)) / cos;
            let d = u * r - o;
            let x = self.anchor.x + d.dot(ex);
            let z = self.anchor.z - d.dot(en);
            if x >= a.min_x && x <= a.max_x && z >= a.min_z && z <= a.max_z {
                return Some(r);
            }
        }
        None
    }

    /// Latitude, longitude and altitude of scene point `p`.
    pub fn place(&self, radius_m: f64, p: DVec3) -> (f64, f64, f64) {
        let (o, ex, up, en) = self.frame(radius_m);
        let d = p - self.anchor;
        let w = o + ex * d.x + up * d.y - en * d.z;
        let r = w.length();
        let lat = (w.z / r).clamp(-1.0, 1.0).asin().to_degrees();
        let lon = w.y.atan2(w.x).to_degrees();
        (lat, lon, r - radius_m)
    }

    /// Compass heading (degrees clockwise from north) of the scene's +X.
    pub fn x_heading_deg(&self) -> f64 {
        (90.0 - self.yaw_rad.to_degrees()).rem_euclid(360.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_places_its_points_and_stands_craft_on_its_areas() {
        let site = Site {
            lat_deg: 0.0,
            lon_deg: 0.0,
            alt_m: 100.0,
            yaw_rad: 0.0,
            anchor: DVec3::ZERO,
            areas: vec![
                Area {
                    min_x: -10.0,
                    max_x: 10.0,
                    min_z: -10.0,
                    max_z: 10.0,
                    y: 5.0,
                },
                Area {
                    min_x: -1000.0,
                    max_x: 1000.0,
                    min_z: -1000.0,
                    max_z: 1000.0,
                    y: 0.0,
                },
            ],
        };
        let r = 600_000.0;
        // the anchor sits at its place; scene +X is east, -Z north
        let (lat, lon, alt) = site.place(r, DVec3::ZERO);
        assert!(lat.abs() < 1e-9 && lon.abs() < 1e-9 && (alt - 100.0).abs() < 1e-6);
        let (_, lon_e, _) = site.place(r, DVec3::new(500.0, 0.0, 0.0));
        assert!(lon_e > 0.0);
        let (lat_n, _, _) = site.place(r, DVec3::new(0.0, 0.0, -500.0));
        assert!(lat_n > 0.0);
        assert!((site.x_heading_deg() - 90.0).abs() < 1e-9);
        // straight down at the anchor: the raised pad; further out the flat ground, which rises off the
        // sphere with distance (a flat site on a round world)
        let on_pad = site.ground_radius(r, direction(0.0, 0.0)).unwrap();
        assert!((on_pad - (r + 105.0)).abs() < 1e-6);
        let (la, lo, _) = site.place(r, DVec3::new(900.0, 0.0, 0.0));
        let far = site.ground_radius(r, direction(la, lo)).unwrap();
        assert!(far > r + 100.0 && far < r + 101.0, "{}", far - r);
        assert!(
            site.ground_radius(r, direction(1.0, 0.0)).is_none(),
            "off the site"
        );
    }
}
