# Status

Updated 2026-10-23.

## What works

- Cargo workspace with seven crates (`ose-math`, `ose-data`, `ose-sim`, `ose-net`, `ose-server`, `ose-client`, `ose-importer`). `cargo test --workspace`, `cargo clippy -D warnings` and the wasm check all pass. `trunk build` in `crates/ose-client` produces a loadable page (no game logic in the client, see ADR-0002).
- `ose-math::Orbit`: Keplerian elements <-> state vectors, the original's anomaly solvers and tolerances, propagation, time to true anomaly, true anomaly at radius; 7 tests including elliptic/hyperbolic round trips.
- `ose-sim::bodies`: bodies as ECS entities with `OnRails` orbits, star-space position by walking the parent chain, `main_body_at` (SOI containment). `cargo run -p ose-sim --example kerbol` loads the converted data and prints every body; Kerbin comes out at 13,599,840 km and 9284.5 m/s, Mun at 12,000 km, Minmus at 47,000 km.
- `ose-sim::vessels`: vessels on rails as entities; `update_reference_body` re-expresses the orbit around whichever body's SOI contains the vessel (escape and encounter), with tests showing the handoff keeps the star-space position continuous.
- `ose-server` streams the universe: `/ws` session (`Hello` -> `Welcome` + `BodyCatalog`, then `TimeState` + `Snapshot` at 10 Hz with star-space position and velocity of every body, `RequestWarp` sets the warp). The Kerbol system (17 bodies, `assets/own/kerbol-system.json`) is embedded in the binary; `--bodies PATH` overrides. `crates/ose-server/tests/ws_session.rs` spawns the binary and checks the whole session. Protocol version 2 in `ose-net`.
- `ose-sim` has an ECS `Universe` with integer `Tick` time, a `WarpFactor` resource, `with_bodies`, `set_warp` and `body_state`.
- `ose-bot` (ADR-0003): an end-to-end bot that plays the Mun campaign as a client. Eleven named steps from server health to Mun landing, each `PASSED`, `BLOCKED` (feature missing, named), `SKIPPED` (no KSP2 data) or `FAILED`. Today 4 pass: health, stock craft (Kerbal K2, 48 parts), the Mun transfer plan (841.6 m/s from 100 km, 7.4 h coast, 110.7 deg lead) and the on-rails coast into the Mun's SOI (5.5 h after the burn); 7 are blocked on WebSocket, campaign, craft, physics and maneuver features. `cargo test --workspace` spawns a server and checks the frontier in `crates/ose-bot/tests/mun_run.rs`.
- `e2e/` (ADR-0004): Playwright drives the trunk-built WASM client in Chromium and Firefox against `ose-server --static-dir`. Eleven player steps (page, WASM start, clean console, WebGPU adapter, canvas, WebSocket, time, map bodies, warp, vessel, maneuver node) with the same `PASSED`/`BLOCKED`/`SKIPPED`/`FAILED` vocabulary and a frontier test. Today in both browsers: 3 pass, WebGPU adapter is skipped headless (Chromium full build has it headless on a GPU machine; Firefox 142 only headed), 7 blocked on the renderer, WebSocket client and HUD. The client exposes features to the test through `data-ose="..."` hooks listed in `e2e/README.md`. CI job `browser-e2e`; `OSE_BASE_URL=https://ose.jfly.dev npm test` checks the live site from the house (passes, 2026-10-23).
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
cargo run -p ose-bot -- --spawn-server   # play the Mun run against a fresh server, print the step table
cd crates/ose-client && trunk serve
.venv/Scripts/python tools/extract_bundles.py
```

Tooling installed locally: rust 1.99 with `wasm32-unknown-unknown`, trunk 0.22, wasm-bindgen-cli, dotnet 9, ilspycmd 11.1, Python venv in `.venv` with UnityPy.

## Deploy

Live at https://ose.jfly.dev/ (home network and Tailscale). A push to `main` builds the static Linux server and the WASM client in GitHub Actions and ships them by scp to the `ose` container on piper, where `ose.service` runs them (`docs/DEPLOY.md`); `python tools/deploy.py` does the same from a developer machine. `ose-server --static-dir DIR` serves the built client.

## Documents

- `docs/DEPLOY.md`: how ose.jfly.dev is built and hosted, and the one-time setup on docker.lan, prod-prox and Cloudflare.
- `docs/01-ksp2-install-survey.md`: what is in the Steam install and how to read it.
- `docs/02-architecture-and-plan.md`: architecture and nine-phase roadmap.
- `docs/ADR-0001-initial-decisions.md`: scope decisions.
- `docs/ADR-0003-e2e-bot.md`: the end-to-end bot, its step outcomes and the frontier test.
- `docs/ADR-0002-thin-wasm-client.md`: logic stays native, the WASM client only renders, interpolates and predicts its own vessel.

## Next (Phase 1, remaining)

- Extract the shipped `PhysicsSettings` asset (drag curves, joint constants) so the sim loads them from data.
- Phase 2 remaining: the patched-conic trajectory predictor (collision, encounter, escape patches), driving vessels from the `Universe` schedule, warp clamping at SOI crossings, and a Kerbin to Mun transfer parity test.
- Convert the scaled-space planet meshes and textures to glTF/KTX2 for Phase 3.

## Known problems

- The client has no renderer yet (owned by the "Web client rendering" thread); the server side of Phase 3 is done.
