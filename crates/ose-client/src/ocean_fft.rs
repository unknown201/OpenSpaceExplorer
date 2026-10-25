//! FFT ocean waves from the original's Crest `OceanWaveSpectrum` (planet.json `ocean.spectrum`).
//!
//! Two periodic patches (cascades) are synthesised on the CPU each frame with Tessendorf's method: the long
//! cascade (256 m, waves 4 m and up) gives height, horizontal (choppy) displacement, slopes and the Jacobian
//! that marks breaking crests for foam; the short one (32 m, waves 0.5 to 4 m) gives slopes only. The result is
//! packed into a two-layer RGBA texture with mips (`OceanFft::texels`) that `planet.wgsl` samples by
//! planet-fixed tile coordinates. Pure Rust so the spectrum and transforms are tested natively.

use serde::Deserialize;
use std::f64::consts::PI;

/// Crest's octaves: the first starts at 2^-4 m, 14 of them.
const SMALLEST_WL_POW_2: f64 = -4.0;
const OCTAVES: usize = 14;
/// Grid size of each cascade.
pub const N: usize = 128;
/// Patch sizes (m); both divide the shader's TILE_PERIOD (4096 m).
pub const PATCH_LONG: f64 = 256.0;
pub const PATCH_SHORT: f64 = 32.0;
/// Where the cascades meet (m).
const SPLIT_WL: f64 = 4.0;
/// Horizontal displacement scale (Tessendorf's lambda).
pub const CHOP: f64 = 1.0;
/// Mip levels of the packed texture (128 down to 4).
pub const MIPS: u32 = 6;

#[derive(Clone, Debug, Deserialize)]
pub struct Spectrum {
    #[serde(default = "default_wind")]
    pub wind_speed: f64,
    #[serde(default = "default_variance")]
    pub direction_variance_deg: f64,
    pub power_log: Vec<f64>,
    #[serde(default = "one")]
    pub multiplier: f64,
}

fn default_wind() -> f64 {
    10.0
}
fn default_variance() -> f64 {
    90.0
}
fn one() -> f64 {
    1.0
}

impl Spectrum {
    /// Variance density (m² per rad/s) at a wavelength: the octave's log10 power, interpolated towards the
    /// next octave across it, as Crest's `GetAmplitude`.
    pub fn density(&self, wavelength: f64) -> f64 {
        let p2 = wavelength
            .log2()
            .clamp(SMALLEST_WL_POW_2, SMALLEST_WL_POW_2 + OCTAVES as f64 - 1.0);
        let lower = p2.floor();
        let i = (lower - SMALLEST_WL_POW_2) as usize;
        let get = |i: usize| self.power_log.get(i).copied().unwrap_or(-8.0);
        let this = get(i);
        let next = if i + 1 < OCTAVES { get(i + 1) } else { this };
        let alpha = (wavelength / 2f64.powf(lower) - 1.0).clamp(0.0, 1.0);
        let p = this + (next - this) * alpha;
        if p <= -7.99 {
            0.0
        } else {
            10f64.powf(p) * self.multiplier
        }
    }

    /// Height variance of all waves between two wavelengths (m²): the density integrated over frequency.
    pub fn variance(&self, wl_min: f64, wl_max: f64, g: f64) -> f64 {
        let steps = 2000;
        let (w0, w1) = (omega(2.0 * PI / wl_max, g), omega(2.0 * PI / wl_min, g));
        let dw = (w1 - w0) / steps as f64;
        (0..steps)
            .map(|i| {
                let w = w0 + (i as f64 + 0.5) * dw;
                let k = w * w / g;
                self.density(2.0 * PI / k) * dw
            })
            .sum()
    }
}

fn omega(k: f64, g: f64) -> f64 {
    (g * k).sqrt()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn new(re: f64, im: f64) -> Self {
        C { re, im }
    }
    fn mul(self, o: C) -> C {
        C::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    fn conj(self) -> C {
        C::new(self.re, -self.im)
    }
    fn scale(self, s: f64) -> C {
        C::new(self.re * s, self.im * s)
    }
    /// times i
    fn i(self) -> C {
        C::new(-self.im, self.re)
    }
}

/// In-place inverse transform (sum of F(k) e^{+ikx}, no 1/N) of rows then columns of an N x N grid.
struct Fft {
    twiddle: Vec<C>,
    rev: Vec<usize>,
    line: Vec<C>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let twiddle = (0..n / 2)
            .map(|i| {
                let a = 2.0 * PI * i as f64 / n as f64;
                C::new(a.cos(), a.sin())
            })
            .collect();
        Fft {
            twiddle,
            rev,
            line: vec![C::default(); n],
        }
    }

    fn line(&mut self) {
        let n = self.line.len();
        for i in 0..n {
            let j = self.rev[i];
            if j > i {
                self.line.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let w = self.twiddle[k * step];
                    let a = self.line[start + k];
                    let b = self.line[start + k + half].mul(w);
                    self.line[start + k] = a.add(b);
                    self.line[start + k + half] = a.sub(b);
                }
            }
            size *= 2;
        }
    }

    fn grid(&mut self, g: &mut [C], n: usize) {
        for r in 0..n {
            self.line.copy_from_slice(&g[r * n..(r + 1) * n]);
            self.line();
            g[r * n..(r + 1) * n].copy_from_slice(&self.line);
        }
        for c in 0..n {
            for r in 0..n {
                self.line[r] = g[r * n + c];
            }
            self.line();
            for r in 0..n {
                g[r * n + c] = self.line[r];
            }
        }
    }
}

/// One periodic patch: initial amplitudes h0(k) and frequencies.
struct Cascade {
    patch: f64,
    h0: Vec<C>,
    omega: Vec<f64>,
    kx: Vec<f64>,
    kz: Vec<f64>,
}

/// Deterministic Gaussian pairs (xorshift + Box-Muller), so every client draws the same sea.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> (f64, f64) {
        let (u, v) = (self.next(), self.next());
        let r = (-2.0 * u.ln()).sqrt();
        (r * (2.0 * PI * v).cos(), r * (2.0 * PI * v).sin())
    }
}

impl Cascade {
    fn new(spec: &Spectrum, patch: f64, wl_min: f64, wl_max: f64, g: f64, seed: u64) -> Self {
        let dk = 2.0 * PI / patch;
        let spread = spec.direction_variance_deg.to_radians().max(0.1);
        let mut rng = Rng(seed | 1);
        let mut c = Cascade {
            patch,
            h0: vec![C::default(); N * N],
            omega: vec![0.0; N * N],
            kx: vec![0.0; N * N],
            kz: vec![0.0; N * N],
        };
        let freq = |i: usize| {
            if i < N / 2 {
                i as f64
            } else {
                i as f64 - N as f64
            }
        };
        for j in 0..N {
            for i in 0..N {
                let idx = j * N + i;
                let (kx, kz) = (freq(i) * dk, freq(j) * dk);
                let (a, b) = rng.gauss();
                c.kx[idx] = kx;
                c.kz[idx] = kz;
                let k = (kx * kx + kz * kz).sqrt();
                if k <= 0.0 {
                    continue;
                }
                let w = omega(k, g);
                c.omega[idx] = w;
                let wl = 2.0 * PI / k;
                if wl < wl_min || wl >= wl_max {
                    continue;
                }
                // Directional spread about the wind (+x): cos² over the variance, integrating to 1.
                let theta = kz.atan2(kx);
                let d = if theta.abs() < spread / 2.0 {
                    (PI * theta / spread).cos().powi(2) * 2.0 / spread
                } else {
                    0.0
                };
                // E(k) dkx dkz = S(w) dw D(theta) dtheta, with dw = g / (2w) dk and k dk dtheta = dkx dkz.
                let e = spec.density(wl) * g / (2.0 * w) * d / k;
                let amp = (e * dk * dk / 2.0).sqrt();
                c.h0[idx] = C::new(a, b).scale(amp / std::f64::consts::SQRT_2);
            }
        }
        c
    }

    /// h~(k, t) = h0(k) e^{iwt} + conj(h0(-k)) e^{-iwt}
    fn at(&self, t: f64) -> Vec<C> {
        let mut out = vec![C::default(); N * N];
        for j in 0..N {
            for i in 0..N {
                let idx = j * N + i;
                let m = ((N - j) % N) * N + (N - i) % N;
                let ph = self.omega[idx] * t;
                let e = C::new(ph.cos(), ph.sin());
                out[idx] = self.h0[idx].mul(e).add(self.h0[m].conj().mul(e.conj()));
            }
        }
        out
    }
}

pub struct OceanFft {
    long: Cascade,
    short: Cascade,
    fft: Fft,
    /// Layer 0: x displacement, height, z displacement, Jacobian; layer 1: long slopes x/z, short slopes x/z.
    pub layers: [Vec<[f32; 4]>; 2],
}

impl OceanFft {
    pub fn new(spec: &Spectrum, gravity: f64) -> Self {
        let g = gravity.max(0.1);
        OceanFft {
            long: Cascade::new(spec, PATCH_LONG, SPLIT_WL, f64::INFINITY, g, 0x5eed_0ce4),
            short: Cascade::new(
                spec,
                PATCH_SHORT,
                2.0 * PATCH_SHORT / N as f64,
                SPLIT_WL,
                g,
                0x0ce4_5eed,
            ),
            fft: Fft::new(N),
            layers: [vec![[0.0; 4]; N * N], vec![[0.0; 4]; N * N]],
        }
    }

    /// Synthesise the sea at time `t` (s).
    pub fn update(&mut self, t: f64) {
        let h = self.long.at(t);
        let mut f1 = vec![C::default(); N * N];
        let mut f2 = vec![C::default(); N * N];
        let mut f3 = vec![C::default(); N * N];
        for idx in 0..N * N {
            let (kx, kz) = (self.long.kx[idx], self.long.kz[idx]);
            let k = (kx * kx + kz * kz).sqrt().max(1e-9);
            let hk = h[idx];
            // D = -i k/|k| h (crests pinch), slopes = i k h
            let dx = hk.i().scale(-kx / k);
            let dz = hk.i().scale(-kz / k);
            let sx = hk.i().scale(kx);
            let sz = hk.i().scale(kz);
            f1[idx] = hk.add(dx.i());
            f2[idx] = dz.add(sx.i());
            f3[idx] = sz;
        }
        self.fft.grid(&mut f1, N);
        self.fft.grid(&mut f2, N);
        self.fft.grid(&mut f3, N);
        let hs = self.short.at(t);
        let mut f4 = vec![C::default(); N * N];
        for idx in 0..N * N {
            let hk = hs[idx];
            f4[idx] = hk
                .i()
                .scale(self.short.kx[idx])
                .add(hk.i().scale(self.short.kz[idx]).i());
        }
        self.fft.grid(&mut f4, N);
        let cell = self.long.patch / N as f64;
        let at = |g: &[C], i: usize, j: usize, im: bool| {
            let v = g[(j % N) * N + (i % N)];
            if im {
                v.im
            } else {
                v.re
            }
        };
        for j in 0..N {
            for i in 0..N {
                let idx = j * N + i;
                // Jacobian of the horizontal displacement by central differences
                let ddx_dx = (at(&f1, i + 1, j, true) - at(&f1, i + N - 1, j, true)) / (2.0 * cell);
                let ddz_dz =
                    (at(&f2, i, j + 1, false) - at(&f2, i, j + N - 1, false)) / (2.0 * cell);
                let ddx_dz = (at(&f1, i, j + 1, true) - at(&f1, i, j + N - 1, true)) / (2.0 * cell);
                let jac =
                    (1.0 + CHOP * ddx_dx) * (1.0 + CHOP * ddz_dz) - CHOP * CHOP * ddx_dz * ddx_dz;
                self.layers[0][idx] = [
                    (f1[idx].im * CHOP) as f32,
                    f1[idx].re as f32,
                    (f2[idx].re * CHOP) as f32,
                    jac as f32,
                ];
                self.layers[1][idx] = [
                    f2[idx].im as f32,
                    f3[idx].re as f32,
                    f4[idx].re as f32,
                    f4[idx].im as f32,
                ];
            }
        }
    }

    /// Both layers with their mips (box filtered), as half floats: for each mip level, layer 0 then layer 1.
    pub fn texels(&self) -> Vec<Vec<u16>> {
        let mut out = Vec::new();
        let mut cur: [Vec<[f32; 4]>; 2] = self.layers.clone();
        let mut n = N;
        for _ in 0..MIPS {
            let mut level = Vec::with_capacity(n * n * 8);
            for layer in &cur {
                for t in layer {
                    for v in t {
                        level.push(f16_bits(*v));
                    }
                }
            }
            out.push(level);
            let m = n / 2;
            for layer in cur.iter_mut() {
                let mut next = vec![[0.0f32; 4]; m * m];
                for j in 0..m {
                    for i in 0..m {
                        let mut s = [0.0f32; 4];
                        for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let t = layer[(2 * j + b) * n + 2 * i + a];
                            for c in 0..4 {
                                s[c] += t[c] * 0.25;
                            }
                        }
                        next[j * m + i] = s;
                    }
                }
                *layer = next;
            }
            n = m;
        }
        out
    }
}

/// f32 to IEEE half (round to nearest, no NaN handling needed here).
pub fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let man = x & 0x7f_ffff;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 {
        return sign | 0x7bff;
    }
    let h = ((exp as u32) << 10) | (man >> 13);
    let round = (man >> 12) & 1;
    sign | (h + round) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerbin() -> Spectrum {
        Spectrum {
            wind_speed: 10.0,
            direction_variance_deg: 90.0,
            power_log: vec![
                -8.0, -8.0, -6.11, -5.25, -5.02, -3.0, -2.55, -1.49, -4.53, -3.83, -0.95, -8.0,
                -8.0, -8.0,
            ],
            multiplier: 1.0,
        }
    }

    #[test]
    fn transform_matches_direct_sum() {
        let n = 8;
        let mut fft = Fft::new(n);
        let mut g = vec![C::default(); n * n];
        g[1 * n + 2] = C::new(1.0, 0.5);
        g[3 * n + 7] = C::new(-0.25, 2.0);
        let src = g.clone();
        fft.grid(&mut g, n);
        for y in 0..n {
            for x in 0..n {
                let mut s = C::default();
                for (idx, v) in src.iter().enumerate() {
                    let (kx, ky) = ((idx % n) as f64, (idx / n) as f64);
                    let a = 2.0 * PI * (kx * x as f64 + ky * y as f64) / n as f64;
                    s = s.add(v.mul(C::new(a.cos(), a.sin())));
                }
                let d = g[y * n + x].sub(s);
                assert!(d.re.abs() < 1e-9 && d.im.abs() < 1e-9);
            }
        }
    }

    #[test]
    fn density_follows_octaves() {
        let s = kerbin();
        // octave 10 covers 64..128 m: its own power at the bottom
        assert!((s.density(64.0) - 10f64.powf(-0.95)).abs() < 1e-9);
        // disabled octaves give nothing
        assert_eq!(s.density(0.07), 0.0);
        assert_eq!(s.density(600.0), 0.0);
    }

    #[test]
    fn r_planet_ocean_fft_height_variance_matches_spectrum() {
        // The synthesised long cascade's height variance matches the spectrum's over the same band.
        let spec = kerbin();
        let mut o = OceanFft::new(&spec, 9.81);
        let mut var = 0.0;
        let mut samples = 0.0;
        for t in [0.0, 7.3, 19.1, 40.0] {
            o.update(t);
            for v in &o.layers[0] {
                var += (v[1] as f64).powi(2);
                samples += 1.0;
            }
        }
        var /= samples;
        let want = spec.variance(SPLIT_WL, PATCH_LONG, 9.81);
        assert!(
            var > want * 0.5 && var < want * 2.0,
            "variance {var} want {want}"
        );
        // a few decimetres of sea: Hs = 4 sigma between 0.3 and 2 m
        let hs = 4.0 * var.sqrt();
        assert!(hs > 0.3 && hs < 2.0, "Hs {hs}");
        // the field is real: the imaginary parts carry the displacement, so the Jacobian stays near 1
        let mean_j: f64 = o.layers[0].iter().map(|v| v[3] as f64).sum::<f64>() / (N * N) as f64;
        assert!((mean_j - 1.0).abs() < 0.1, "mean Jacobian {mean_j}");
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(0.5), 0x3800);
    }

    #[test]
    fn mips_halve() {
        let o = OceanFft::new(&kerbin(), 9.81);
        let t = o.texels();
        assert_eq!(t.len(), MIPS as usize);
        assert_eq!(t[0].len(), N * N * 8);
        assert_eq!(t[1].len(), N * N * 2);
    }
}
