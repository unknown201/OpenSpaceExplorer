# ADR-0002: Thin WASM client, logic stays native

Date: 2026-10-23. Status: accepted (Josh, project thread).

## Decision

Game logic lives in native Rust (the server). The browser WASM client contains only rendering, input, UI and
the minimum simulation needed for smooth presentation: interpolation of received snapshots and short-horizon
prediction of the player's own vessel. The full `ose-sim` (resource flow, staging, joints, physics bubble,
patched-conic solver, missions) is never compiled into the client.

## Consequences

- The workspace is split so the dependency direction is enforced by Cargo:
  - `ose-math` (pure math, compiles anywhere) and `ose-data` (schemas) are shared.
  - `ose-sim` is native only. `ose-client` does not depend on it; CI checks the wasm target without it.
  - `ose-net` carries the protocol plus a small `ose-net::predict` module (Keplerian extrapolation and
    snapshot interpolation over `ose-math::Orbit`) that both sides use; this is the only "logic" in the client.
- Single-player is the same binary as multiplayer: the client always talks to an `ose-server` process. For a
  solo session the player runs `ose-server` locally (one command, or a launcher later).
- Trajectory prediction (patched conics for the map view) is computed on the server and sent as a list of
  patches; the client only draws them. Maneuver-node editing sends intents and receives recomputed patches.
- The client-side ECS (if any) is a render scene graph, not game state.
- `02-architecture-and-plan.md` is updated: Phase 3 (map view) now means "server streams body and vessel
  states, client renders them", and `ose-client` drops its `ose-sim` dependency in Phase 0 scaffolding.
