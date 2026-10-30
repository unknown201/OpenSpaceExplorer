//! The navball, drawn from the original's own texture (`Flight-Navball-Texture`, exported from
//! `hudinstruments_assets_all.bundle` to `assets/ui/`) and its marker icons.
//!
//! The texture is an atlas, not an equirectangular map: the band between about -37 and +45 degrees of pitch
//! runs across the middle (north at x = 512, 128 px per 45 degrees of heading, horizon at y = 532, 3.15 px per
//! degree), and each pole is a disk (sky centred at (256, 250) with north to the right, ground centred at
//! (767, 818) with north to the left; 3 px per degree from the pole, 90 degrees of heading at the top). Each
//! ball pixel is turned into a direction in the craft frame, then into pitch and heading in the local
//! horizon frame, then sampled from the atlas; markers are projected onto the ball where they face forward.

use glam::DVec3;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, ImageData};

/// The craft's axes and the local horizon at the vessel, in one frame (the flight view's model frame).
#[derive(Clone, Copy, Debug)]
pub struct NavFrame {
    /// Craft nose (ball centre), the craft side shown at the top of the ball, and the right side.
    pub forward: DVec3,
    pub top: DVec3,
    pub right: DVec3,
    /// Local up, north and east.
    pub up: DVec3,
    pub north: DVec3,
    pub east: DVec3,
    /// Prograde direction for the marker (surface or orbital), if moving.
    pub prograde: Option<DVec3>,
    /// Orbit normal and radial-out, for their markers.
    pub normal: Option<DVec3>,
    pub radial: Option<DVec3>,
    /// Burn direction of the planned maneuver node, if any.
    pub maneuver: Option<DVec3>,
    /// Direction to the target vessel, if one is set.
    pub target: Option<DVec3>,
}

impl NavFrame {
    /// Instruments use the inertial craft pose in every view, independently of the renderer.
    pub fn new(
        r: DVec3,
        velocity: DVec3,
        forward: DVec3,
        top: DVec3,
        omega: f64,
        orbit_speed: bool,
    ) -> Self {
        let up = r.normalize_or_zero();
        let north = (DVec3::Z - up * up.z).normalize_or_zero();
        let east = north.cross(up);
        let v = if orbit_speed {
            velocity
        } else {
            velocity - DVec3::Z.cross(r) * omega
        };
        let moving = v.length() > 0.5;
        let normal = r.cross(velocity);
        let radial = r - velocity.normalize_or_zero() * r.dot(velocity.normalize_or_zero());
        Self {
            forward,
            top,
            right: forward.cross(top),
            up,
            north,
            east,
            prograde: moving.then(|| v.normalize_or_zero()),
            normal: (normal.length() > 1.0).then(|| normal.normalize_or_zero()),
            radial: (radial.length() > 1.0 && moving).then(|| radial.normalize_or_zero()),
            maneuver: None,
            target: None,
        }
    }
}

struct Atlas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// Loads the atlas and marker images once, then draws the ball into a canvas each frame.
pub struct Navball {
    atlas: Rc<RefCell<Option<Atlas>>>,
    icons: Vec<(&'static str, HtmlImageElement)>,
    shadow: Option<HtmlImageElement>,
    started: bool,
    buf: Vec<u8>,
}

impl Default for Navball {
    fn default() -> Self {
        Self::new()
    }
}

fn image(src: &str) -> Option<HtmlImageElement> {
    let img = HtmlImageElement::new().ok()?;
    img.set_src(src);
    Some(img)
}

impl Navball {
    pub fn new() -> Navball {
        Navball {
            atlas: Rc::new(RefCell::new(None)),
            icons: Vec::new(),
            shadow: None,
            started: false,
            buf: Vec::new(),
        }
    }

    fn start(&mut self) {
        self.started = true;
        for name in [
            "ICO-Navball-Prograde",
            "ICO-Navball-Retrograde",
            "ICO-Navball-Normal",
            "ICO-Navball-AntiNormal",
            "ICO-Navball-RadialOut",
            "ICO-Navball-RadialIn",
            "ICO-Navball-Maneuver",
            "ICO-Navball-Target",
            "ICO-Navball-AntiTarget",
        ] {
            if let Some(img) = image(&format!("assets/ui/{name}.png")) {
                self.icons.push((name, img));
            }
        }
        self.shadow = image("assets/ui/Flight-Navball-Shadow.png");
        let Some(img) = image("assets/ui/Flight-Navball-Texture.png") else {
            return;
        };
        let slot = self.atlas.clone();
        let img2 = img.clone();
        let onload = Closure::<dyn FnMut()>::new(move || {
            let (w, h) = (img2.natural_width(), img2.natural_height());
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            let Some(canvas) = doc
                .create_element("canvas")
                .ok()
                .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
            else {
                return;
            };
            canvas.set_width(w);
            canvas.set_height(h);
            let Some(ctx) = canvas
                .get_context("2d")
                .ok()
                .flatten()
                .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
            else {
                return;
            };
            if ctx
                .draw_image_with_html_image_element(&img2, 0.0, 0.0)
                .is_err()
            {
                return;
            }
            if let Ok(data) = ctx.get_image_data(0.0, 0.0, w as f64, h as f64) {
                *slot.borrow_mut() = Some(Atlas {
                    w,
                    h,
                    rgba: data.data().0,
                });
            }
        });
        img.set_onload(Some(onload.as_ref().unchecked_ref()));
        onload.forget();
    }

    /// Atlas pixel for pitch and heading (degrees).
    fn sample(a: &Atlas, pitch: f64, heading: f64) -> [u8; 3] {
        let (sx, sy) = (a.w as f64 / 1024.0, a.h as f64 / 1024.0);
        let h = heading.to_radians();
        let (x, y) = if pitch > 40.0 {
            let r = (90.0 - pitch) * 3.0;
            (256.0 + r * h.cos(), 250.0 - r * h.sin())
        } else if pitch < -35.0 {
            let r = (90.0 + pitch) * 3.0;
            (767.0 - r * h.cos(), 818.0 - r * h.sin())
        } else {
            (
                (512.0 + heading * (128.0 / 45.0)).rem_euclid(1024.0),
                532.0 - pitch * 3.15,
            )
        };
        // Bilinear between the four nearest atlas pixels: the lines and numbers stay smooth as the ball turns.
        let (fx, fy) = (x * sx - 0.5, y * sy - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let px = |dx: f64, dy: f64| {
            let cx = ((x0 + dx) as i64).clamp(0, a.w as i64 - 1) as usize;
            let cy = ((y0 + dy) as i64).clamp(0, a.h as i64 - 1) as usize;
            (cy * a.w as usize + cx) * 4
        };
        let (i00, i10, i01, i11) = (px(0.0, 0.0), px(1.0, 0.0), px(0.0, 1.0), px(1.0, 1.0));
        let mut out = [0u8; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let top = a.rgba[i00 + k] as f64 * (1.0 - tx) + a.rgba[i10 + k] as f64 * tx;
            let bot = a.rgba[i01 + k] as f64 * (1.0 - tx) + a.rgba[i11 + k] as f64 * tx;
            *o = (top * (1.0 - ty) + bot * ty).round() as u8;
        }
        out
    }

    /// Draw the ball for `nav` into `canvas` (square). Returns false until the atlas has loaded.
    pub fn draw(&mut self, canvas: &HtmlCanvasElement, nav: &NavFrame) -> bool {
        if !self.started {
            self.start();
        }
        let atlas = self.atlas.borrow();
        let Some(a) = atlas.as_ref() else {
            return false;
        };
        let size = canvas.width().max(16) as usize;
        let Some(ctx) = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
        else {
            return false;
        };
        self.buf.resize(size * size * 4, 0);
        let half = size as f64 * 0.5;
        for py in 0..size {
            for px in 0..size {
                let x = (px as f64 + 0.5 - half) / half;
                let y = (half - py as f64 - 0.5) / half;
                let r2 = x * x + y * y;
                let i = (py * size + px) * 4;
                // The rim fades over one pixel instead of stepping (coverage by distance to the edge).
                let edge = ((1.0 - r2.sqrt()) * half + 0.5).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    self.buf[i + 3] = 0;
                    continue;
                }
                let z = (1.0 - r2.min(1.0)).sqrt();
                let d = nav.right * x + nav.top * y + nav.forward * z;
                let pitch = d.dot(nav.up).clamp(-1.0, 1.0).asin().to_degrees();
                let heading = d
                    .dot(nav.east)
                    .atan2(d.dot(nav.north))
                    .to_degrees()
                    .rem_euclid(360.0);
                let c = Self::sample(a, pitch, heading);
                // Gentle shading towards the rim, as the original's lit sphere.
                let shade = 0.55 + 0.45 * z;
                self.buf[i] = (c[0] as f64 * shade) as u8;
                self.buf[i + 1] = (c[1] as f64 * shade) as u8;
                self.buf[i + 2] = (c[2] as f64 * shade) as u8;
                self.buf[i + 3] = (edge * 255.0).round() as u8;
            }
        }
        if let Ok(img) = ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&self.buf),
            size as u32,
            size as u32,
        ) {
            ctx.put_image_data(&img, 0.0, 0.0).ok();
        }
        if let Some(sh) = &self.shadow {
            ctx.draw_image_with_html_image_element_and_dw_and_dh(
                sh,
                0.0,
                0.0,
                size as f64,
                size as f64,
            )
            .ok();
        }
        // Markers where they face the viewer.
        let icon = size as f64 * 0.17;
        let put = |name: &str, d: DVec3| {
            let z = d.dot(nav.forward);
            if z <= 0.0 {
                return;
            }
            let (x, y) = (d.dot(nav.right), d.dot(nav.top));
            if let Some((_, img)) = self.icons.iter().find(|(n, _)| *n == name) {
                ctx.draw_image_with_html_image_element_and_dw_and_dh(
                    img,
                    half + x * half - icon * 0.5,
                    half - y * half - icon * 0.5,
                    icon,
                    icon,
                )
                .ok();
            }
        };
        if let Some(p) = nav.prograde {
            put("ICO-Navball-Prograde", p);
            put("ICO-Navball-Retrograde", -p);
        }
        if let Some(n) = nav.normal {
            put("ICO-Navball-Normal", n);
            put("ICO-Navball-AntiNormal", -n);
        }
        if let Some(r) = nav.radial {
            put("ICO-Navball-RadialOut", r);
            put("ICO-Navball-RadialIn", -r);
        }
        if let Some(m) = nav.maneuver {
            put("ICO-Navball-Maneuver", m);
        }
        if let Some(t) = nav.target {
            put("ICO-Navball-Target", t);
            put("ICO-Navball-AntiTarget", -t);
        }
        // The craft's own marker: a fixed chevron at the centre.
        ctx.set_stroke_style_str("#ffcc33");
        ctx.set_line_width((size as f64 / 60.0).max(2.0));
        ctx.begin_path();
        let s = size as f64 * 0.1;
        ctx.move_to(half - s, half + s * 0.15);
        ctx.line_to(half - s * 0.35, half + s * 0.15);
        ctx.line_to(half, half + s * 0.5);
        ctx.line_to(half + s * 0.35, half + s * 0.15);
        ctx.line_to(half + s, half + s * 0.15);
        ctx.stroke();
        true
    }
}
