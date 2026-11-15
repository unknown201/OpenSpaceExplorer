//! Delivery routes (`docs/delivery-routes-design.md`). Step 1: recording the proving flight.
//!
//! Server-free: everything here reads the simulation (`ose_sim::Universe`) and keeps plain data that
//! serializes into the campaign save. The server owns when a recording starts, which vessels are
//! depots, and when a player finishes or reverts.
pub mod recording;
