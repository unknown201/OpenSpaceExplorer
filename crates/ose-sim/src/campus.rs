//! A flat built site on a body (the KSC campus): the client draws it rigidly from its scene, anchored on a
//! point of the planet and turned to fit, and the planet's terrain is sunk under it. Craft standing there
//! stand on the scene's surfaces: each area is a rectangle of the scene's horizontal plane at its own
//! height, and the ground under a body-fixed direction is where that direction meets it.

use ose_math::DVec3;

/// One flat area of the site, in scene coordinates (x, z ranges, surface height y), with an optional apron:
/// `apron_m` beyond its edges the surface slopes on down to `apron_y` (a launch pad's raised deck meets the
/// campus ground that way, as KSP2's does, instead of ending in a drop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
    pub y: f64,
    pub apron_m: f64,
    pub apron_y: f64,
}

impl Area {
    /// A flat area without an apron.
    pub fn flat(min_x: f64, max_x: f64, min_z: f64, max_z: f64, y: f64) -> Area {
        Area {
            min_x,
            max_x,
            min_z,
            max_z,
            y,
            apron_m: 0.0,
            apron_y: y,
        }
    }

    /// The surface height at scene (x, z), or `None` off the area and its apron.
    pub fn height(&self, x: f64, z: f64) -> Option<f64> {
        let dx = (self.min_x - x).max(x - self.max_x).max(0.0);
        let dz = (self.min_z - z).max(z - self.max_z).max(0.0);
        let d = dx.max(dz);
        if d <= 0.0 {
            return Some(self.y);
        }
        if d > self.apron_m {
            return None;
        }
        // a smooth shoulder at the deck and the foot, so wheels roll over both
        let t = d / self.apron_m;
        Some(self.y + (self.apron_y - self.y) * t * t * (3.0 - 2.0 * t))
    }
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
    /// Conservative outer radius of every flat site surface, used to skip distant contact sweeps.
    pub fn surface_radius_bound(&self, radius_m: f64) -> f64 {
        let (origin, east, up, north) = self.frame(radius_m);
        self.areas
            .iter()
            .flat_map(|a| {
                [a.min_x, a.max_x].into_iter().flat_map(move |x| {
                    [a.min_z, a.max_z].into_iter().map(move |z| {
                        let d = DVec3::new(x, a.y, z) - self.anchor;
                        (origin + east * d.x + up * d.y - north * d.z).length()
                    })
                })
            })
            .fold(radius_m, f64::max)
    }
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
            // where `u` meets the plane at scene height `y`: the area's, then on an apron the slope's there
            let at = |y: f64| {
                let r = (o.dot(up) + (y - self.anchor.y)) / cos;
                let d = u * r - o;
                (r, self.anchor.x + d.dot(ex), self.anchor.z - d.dot(en))
            };
            let (r, x, z) = at(a.y);
            match a.height(x, z) {
                Some(h) if h == a.y => return Some(r),
                Some(h) => {
                    let (r2, x2, z2) = at(h);
                    if let Some(h2) = a.height(x2, z2) {
                        return Some(at(h2).0);
                    }
                    return Some(r2);
                }
                None => {}
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
                    apron_m: 40.0,
                    apron_y: 0.0,
                    ..Area::flat(-10.0, 10.0, -10.0, 10.0, 5.0)
                },
                Area::flat(-1000.0, 1000.0, -1000.0, 1000.0, 0.0),
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
        // off the pad's edge the apron slopes down to the ground, without a step
        let mut prev = on_pad;
        for x in (0..=21).map(|k| 10.5 + k as f64 * 2.0) {
            let (la, lo, _) = site.place(r, DVec3::new(x, 0.0, 0.0));
            let g = site.ground_radius(r, direction(la, lo)).unwrap();
            assert!(
                g <= prev + 0.01 && prev - g < 0.5,
                "{x}: {} after {}",
                g - r,
                prev - r
            );
            prev = g;
        }
        assert!((prev - (r + 100.0)).abs() < 0.01, "{}", prev - r);
        let (la, lo, _) = site.place(r, DVec3::new(900.0, 0.0, 0.0));
        let far = site.ground_radius(r, direction(la, lo)).unwrap();
        assert!(far > r + 100.0 && far < r + 101.0, "{}", far - r);
        assert!(
            site.ground_radius(r, direction(1.0, 0.0)).is_none(),
            "off the site"
        );
    }
}
