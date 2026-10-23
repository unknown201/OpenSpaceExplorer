//! f64 math for orbital mechanics: vectors, Kepler solvers, reference frames.

#![forbid(unsafe_code)]

pub mod kepler;
pub mod orbit;
pub mod vec3;

pub use vec3::DVec3;

pub use orbit::{Elements, Frame, Orbit};
