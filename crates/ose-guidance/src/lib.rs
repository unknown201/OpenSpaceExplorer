//! Rendezvous guidance shared by the browser client (advisory cues) and the server (route pilot).
//!
//! No I/O and no platform code: it builds for native and wasm32 alike, so a player's guidance and an
//! unattended pilot always plan with the same code. See `docs/rendezvous-v2-plan.md`.
pub mod plan;
