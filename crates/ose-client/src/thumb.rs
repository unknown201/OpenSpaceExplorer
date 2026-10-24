//! Craft thumbnails: a side-view silhouette of a craft's part tree, from each part's local position and
//! rotation (`CraftDocument`) and a per-part size. Pure geometry, unit-tested natively; `lib.rs` paints the
//! boxes on a small canvas. Until the server sends part geometry, every part is a `DEFAULT_SIZE_M` cube,
//! which still gives a recognisable stack from the positions alone.

use ose_data::craft::{Craft, Quat};
use ose_math::DVec3;

/// Edge of the cube used for parts whose size is unknown (a small tank is about this big).
pub const DEFAULT_SIZE_M: f64 = 1.25;

/// One part as an axis-aligned box in the craft frame after rotation: the side-view rectangle is
/// `(min.x, min.y)` to `(max.x, max.y)` with the stack axis `y` up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartBox {
    pub part: usize,
    pub stage: i32,
    pub min: DVec3,
    pub max: DVec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: DVec3,
    pub max: DVec3,
}

impl Bounds {
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
}

fn q_mul(a: Quat, b: Quat) -> Quat {
    Quat {
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    }
}

fn q_rotate(q: Quat, v: DVec3) -> DVec3 {
    // v' = v + 2w(q x v) + 2(q x (q x v))
    let u = DVec3::new(q.x, q.y, q.z);
    let t = u.cross(v) * 2.0;
    v + t * q.w + u.cross(t)
}

fn q_norm(q: Quat) -> Quat {
    let n = (q.w * q.w + q.x * q.x + q.y * q.y + q.z * q.z).sqrt();
    if n == 0.0 {
        return Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
    }
    Quat {
        x: q.x / n,
        y: q.y / n,
        z: q.z / n,
        w: q.w / n,
    }
}

/// Size and centre (metres, part frame) of a part, or `None` to use the default cube.
pub type SizeOf<'a> = &'a dyn Fn(&str) -> Option<(DVec3, DVec3)>;

/// World (craft-frame) pose of every part: parent pose composed with the part's local position and rotation.
pub fn poses(craft: &Craft) -> Vec<(DVec3, Quat)> {
    let n = craft.parts.len();
    let mut out: Vec<Option<(DVec3, Quat)>> = vec![None; n];
    // Parts are not guaranteed parent-first; iterate until everything is placed (tree depth passes).
    for _ in 0..n.max(1) {
        let mut progressed = false;
        for (i, p) in craft.parts.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            let pose = match p.parent {
                None => Some((p.local_position, q_norm(p.local_rotation))),
                Some(pi) => out.get(pi).copied().flatten().map(|(pp, pq)| {
                    (
                        pp + q_rotate(pq, p.local_position),
                        q_norm(q_mul(pq, p.local_rotation)),
                    )
                }),
            };
            if pose.is_some() {
                out[i] = pose;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    out.into_iter()
        .map(|p| {
            p.unwrap_or((
                DVec3::ZERO,
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            ))
        })
        .collect()
}

/// Boxes for every part plus the overall bounds. Empty crafts give `None`.
pub fn layout(craft: &Craft, size_of: SizeOf) -> Option<(Vec<PartBox>, Bounds)> {
    if craft.parts.is_empty() {
        return None;
    }
    let poses = poses(craft);
    let mut boxes = Vec::with_capacity(craft.parts.len());
    let mut bmin = DVec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut bmax = DVec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (i, p) in craft.parts.iter().enumerate() {
        let (size, centre) = size_of(&p.part_id).unwrap_or((
            DVec3::new(DEFAULT_SIZE_M, DEFAULT_SIZE_M, DEFAULT_SIZE_M),
            DVec3::ZERO,
        ));
        let (pos, rot) = poses[i];
        let half = size * 0.5;
        let mut min = DVec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = DVec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let corner = centre + DVec3::new(half.x * sx, half.y * sy, half.z * sz);
                    let w = pos + q_rotate(rot, corner);
                    min = DVec3::new(min.x.min(w.x), min.y.min(w.y), min.z.min(w.z));
                    max = DVec3::new(max.x.max(w.x), max.y.max(w.y), max.z.max(w.z));
                }
            }
        }
        bmin = DVec3::new(bmin.x.min(min.x), bmin.y.min(min.y), bmin.z.min(min.z));
        bmax = DVec3::new(bmax.x.max(max.x), bmax.y.max(max.y), bmax.z.max(max.z));
        boxes.push(PartBox {
            part: i,
            stage: p.stage_index,
            min,
            max,
        });
    }
    // Draw from the back (large z first) so nearer parts paint over farther ones.
    boxes.sort_by(|a, b| {
        let az = (a.min.z + a.max.z) * 0.5;
        let bz = (b.min.z + b.max.z) * 0.5;
        bz.partial_cmp(&az).unwrap_or(std::cmp::Ordering::Equal)
    });
    Some((
        boxes,
        Bounds {
            min: bmin,
            max: bmax,
        },
    ))
}

/// Map a craft-frame x,y to pixels in a `w` x `h` canvas with a margin, stack axis up.
pub fn fit(bounds: &Bounds, w: f64, h: f64, margin: f64) -> impl Fn(f64, f64) -> (f64, f64) {
    let bw = bounds.width().max(1e-6);
    let bh = bounds.height().max(1e-6);
    let scale = ((w - 2.0 * margin) / bw).min((h - 2.0 * margin) / bh);
    let cx = (bounds.min.x + bounds.max.x) * 0.5;
    let cy = (bounds.min.y + bounds.max.y) * 0.5;
    move |x, y| (w * 0.5 + (x - cx) * scale, h * 0.5 - (y - cy) * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_data::craft::CraftPart;

    fn part(part_id: &str, parent: Option<usize>, pos: DVec3, stage: i32) -> CraftPart {
        CraftPart {
            guid: part_id.into(),
            part_id: part_id.into(),
            parent,
            own_node: None,
            parent_node: None,
            local_position: pos,
            local_rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            stage_index: stage,
            fuel_crossfeed: false,
            resources: vec![],
        }
    }

    fn stack() -> Craft {
        Craft {
            name: "stack".into(),
            description: String::new(),
            parts: vec![
                part("pod", None, DVec3::ZERO, -1),
                part("tank", Some(0), DVec3::new(0.0, -2.0, 0.0), 0),
                part("engine", Some(1), DVec3::new(0.0, -1.5, 0.0), 0),
            ],
            stages: vec![vec![2]],
        }
    }

    #[test]
    fn default_cubes_stack_downwards() {
        let (boxes, b) = layout(&stack(), &|_| None).unwrap();
        assert_eq!(boxes.len(), 3);
        assert!((b.width() - DEFAULT_SIZE_M).abs() < 1e-9);
        // pod top at +0.625, engine bottom at -3.5 - 0.625
        assert!((b.max.y - 0.625).abs() < 1e-9);
        assert!((b.min.y + 4.125).abs() < 1e-9);
    }

    #[test]
    fn sizes_and_rotations_apply() {
        let mut c = stack();
        // Rotate the tank 90 degrees about z: its 1x3 box becomes 3x1 in the side view.
        let s = (std::f64::consts::FRAC_PI_4).sin();
        c.parts[1].local_rotation = Quat {
            x: 0.0,
            y: 0.0,
            z: s,
            w: s,
        };
        let size_of = |id: &str| match id {
            "tank" => Some((DVec3::new(1.0, 3.0, 1.0), DVec3::ZERO)),
            _ => None,
        };
        let (boxes, _) = layout(&c, &size_of).unwrap();
        let tank = boxes.iter().find(|b| b.part == 1).unwrap();
        assert!(((tank.max.x - tank.min.x) - 3.0).abs() < 1e-9);
        assert!(((tank.max.y - tank.min.y) - 1.0).abs() < 1e-9);
        // The engine hangs off the rotated tank, so its offset turned sideways too.
        let engine = boxes.iter().find(|b| b.part == 2).unwrap();
        assert!(((engine.min.x + engine.max.x) * 0.5 - 1.5).abs() < 1e-9);
    }

    #[test]
    fn fit_centres_and_scales() {
        let (_, b) = layout(&stack(), &|_| None).unwrap();
        let f = fit(&b, 96.0, 96.0, 4.0);
        let (x, y_top) = f(0.0, b.max.y);
        let (_, y_bottom) = f(0.0, b.min.y);
        assert!((x - 48.0).abs() < 1e-9);
        assert!((y_top - 4.0).abs() < 1e-9 && (y_bottom - 92.0).abs() < 1e-9);
    }
}
