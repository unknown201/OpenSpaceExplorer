# Status

Updated 2026-10-23.

## What works

- Cargo workspace with seven crates (`ose-math`, `ose-data`, `ose-sim`, `ose-net`, `ose-server`, `ose-client`, `ose-importer`). `cargo test --workspace`, `cargo clippy -D warnings` and the wasm check all pass. `trunk build` in `crates/ose-client` produces a loadable page that ticks the shared sim.
- `ose-math::Orbit`: Keplerian elements <-> state vectors, the original's anomaly solvers and tolerances, propagation, time to true anomaly, true anomaly at radius; 7 tests including elliptic/hyperbolic round trips.
- `ose-sim::bodies`: bodies as ECS entities with `OnRails` orbits, star-space position by walking the parent chain, `main_body_at` (SOI containment). `cargo run -p ose-sim --example kerbol` loads the converted data and prints every body; Kerbin comes out at 13,599,840 km and 9284.5 m/s, Mun at 12,000 km, Minmus at 47,000 km.
- `ose-sim` has an ECS `Universe` with integer `Tick` time and a `WarpFactor` resource. `ose-net` has a first protocol enum with postcard round-trip tests.
- `tools/ksp2_catalog.py` parses the Addressables catalog (keys, buckets, entries) and resolves any key to its bundle.
- `tools/extract_bundles.py` extracts every JSON data asset from the install into `assets/ksp2/data/` (gitignored): 798 files, including 400 part definitions, 22 celestial bodies, 14 resources, 87 tech nodes, 9 stock assemblies, missions and tutorials. Format is `{version, useExternal, data: {...}}`.
- `ose-importer` converts the extracted JSON into our `ose-data` schema: 22 bodies (mu from `gravityASL * g0 * r^2`, SOI from `a * (m/M)^0.4`, orbits from `GalaxyDefinition_Default`, atmosphere pressure/temperature curves), 14 resources (basic and recipe), 400 parts (mass, attach rules/nodes, containers, 41 engine modes with Isp curves and gimbal, drag cubes, remaining modules kept raw). `cargo run -p ose-importer` writes `assets/ksp2/converted/game_data.json` (2.9 MB) and `coverage.txt` listing every source field not yet mapped. The real-data test runs when the extracted data is present and skips otherwise.
- `docs/reference-notes/`: our notes on the original algorithms, written as specs for the Rust code. `orbits.md` (frames, anomaly solvers, state vectors, SOI handoff, patched conics, warp clamping), `physics-joints-drag.md` (gravity application, joint stiffness and break formulas with defaults, atmosphere properties, drag-cube aero with the default curves, engine thrust and mass flow), `resource-flow.md` (flow graph edge kinds, priority flood fills per flow mode, request processing, proportional draining).
- `reference/ksp2/` (gitignored) holds the ILSpy decompile of `Assembly-CSharp` (6414 files) and the in-house support assemblies.

## How to run

```
cargo test --workspace
cargo run -p ose-server            # http://127.0.0.1:7777/health
cargo run -p ose-importer          # assets/ksp2/data -> assets/ksp2/converted
cd crates/ose-client && trunk serve
.venv/Scripts/python tools/extract_bundles.py
```

Tooling installed locally: rust 1.99 with `wasm32-unknown-unknown`, trunk 0.22, wasm-bindgen-cli, dotnet 9, ilspycmd 11.1, Python venv in `.venv` with UnityPy.

## Documents

- `docs/01-ksp2-install-survey.md`: what is in the Steam install and how to read it.
- `docs/02-architecture-and-plan.md`: architecture and nine-phase roadmap.
- `docs/ADR-0001-initial-decisions.md`: scope decisions.

## Next (Phase 1, remaining)

- Extract the shipped `PhysicsSettings` asset (drag curves, joint constants) so the sim loads them from data.
- Phase 2 remaining: vessels on rails as entities, SOI transition (escape/encounter) handoff with `Orbit::from_state_vectors`, the patched-conic trajectory predictor, warp clamping at SOI crossings, and parity tests (Kerbin to Mun transfer).
- Convert the scaled-space planet meshes and textures to glTF/KTX2 for Phase 3.

## Known problems

- None blocking. The server has no WebSocket endpoint yet and the client has no renderer; both are Phase 3 and 4 work.
