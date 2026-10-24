# Status

Updated 2026-10-23.

## What works

- Cargo workspace with seven crates (`ose-math`, `ose-data`, `ose-sim`, `ose-net`, `ose-server`, `ose-client`, `ose-importer`). `cargo test --workspace`, `cargo clippy -D warnings` and the wasm check all pass. `trunk build` in `crates/ose-client` produces a loadable page (no game logic in the client, see ADR-0002).
- `ose-math::Orbit`: Keplerian elements <-> state vectors, the original's anomaly solvers and tolerances, propagation, time to true anomaly, true anomaly at radius; 7 tests including elliptic/hyperbolic round trips.
- `ose-sim::bodies`: bodies as ECS entities with `OnRails` orbits, star-space position by walking the parent chain, `main_body_at` (SOI containment). `cargo run -p ose-sim --example kerbol` loads the converted data and prints every body; Kerbin comes out at 13,599,840 km and 9284.5 m/s, Mun at 12,000 km, Minmus at 47,000 km.
- `ose-sim::trajectory`: patched-conic predictor (`predict`): collision, encounter (sampled closest approach refined with the original's halving-step and SOI bisection solvers, 0.01 s tolerance), escape and final patches with handoff via state vectors. Parity: a 100 km Hohmann transfer to the Mun enters its SOI 19,941 s after the burn (the bot's independent estimate: 19,950 s) and, with no lead error, impacts the Mun as the bot predicted.
- `ose-sim::vessels`: vessels on rails as entities; `update_reference_body` re-expresses the orbit around whichever body's SOI contains the vessel (escape and encounter), with tests showing the handoff keeps the star-space position continuous.
- `ose-server` streams the universe: `/ws` session (`Hello` -> `Welcome` + `BodyCatalog`, then `TimeState` + `Snapshot` at 10 Hz with star-space position and velocity of every body, `RequestWarp` sets the warp). The Kerbol system (17 bodies, `assets/own/kerbol-system.json`) is embedded in the binary; `--bodies PATH` overrides. `crates/ose-server/tests/ws_session.rs` spawns the binary and checks the whole session. Protocol version 2 in `ose-net`.
- `ose-sim::Universe` drives vessels: each step advances up to `warp` ticks but never past a vessel's next predicted SOI transition (the original's timescale override), then runs the SOI handoff for every vessel. `spawn_vessel`, `spawn_vessel_in_circular_orbit`, `vessel`, `predict`.
- Guest sweep: players who never joined the campaign and have been away for `--guest-ttl-s` (default one day) are removed with their vessels and saved crafts every `--guest-sweep-s` (60 s); connected players and campaign members are never swept. Keeps the live server tidy after e2e runs. Tested.
- Craft library (protocol v7): `CraftList` lists the 9 stock crafts (converted by the importer into `assets/own/stock-craft.json`, ids 1..9) plus the player's saved crafts (ids from 1000) with parts, stages, dry/wet mass and delta-v; `SaveCraft`, `DeleteCraft`, `GetCraft` (full document) and `LaunchSavedCraft { id, altitude_m }`. Saved crafts persist in the save file.
- Sessions, difficulty and agency (protocol v6): `ListSessions` -> one shared session (id `shared`, name, mode, UT, last played, players); `SelectSession` joins it; `CreateSession { name, mode, difficulty }` configures it while nobody has joined; `SetAgency { flag_id, colours }` is echoed in `CampaignMember.agency`. `DifficultyOptions` (11 definitions from the imported `DifficultyOptionDefinitions` plus the Easy/Normal/Hard/RocketScientist presets, `assets/own/difficulty.json`) is sent once after `BodyCatalog`. `CampaignState` now carries id, mode, difficulty and `created`.
- Campaign (protocol v5): `JoinCampaign` adds the player to the one shared campaign (idempotent) and `CampaignInfo` reads it; `CampaignState` returns name, start tick, members with join ticks, membership flag and vessel count. Membership lives in the save file.
- Per-stage delta-v (`ose_data::staging::delta_v_by_stage`): stages fire highest id first, decouplers jettison their subtree, engines keep running until jettisoned, each stage burns until the first propellant its engines can reach over crossfeed runs out; Tsiolkovsky with thrust-weighted Isp per stage. Kerbal K2 reads 1375 + 1744 + 3081 = 6200 m/s vacuum over its three burning stages. `CraftLaunched` carries the readout (protocol v4).
- Craft launch: `ClientMessage::LaunchCraft { craft, altitude_m }` validates the craft against the embedded part catalog (`assets/own/parts-catalog.json`, 400 parts without drag cubes and raw modules, plus `resources.json`), answers `CraftLaunched` with part count, dry and wet mass and a whole-craft vacuum delta-v estimate, and spawns the craft as an owned vessel in a circular orbit (a launch pad replaces this in Phase 6). Session test launches a pod plus Terrier.
- Universe save: time, warp and every vessel (owner, orbit elements, scheduled burn) are written to `--save ose-save.json` every `--save-interval-s` (30 s) and restored at startup, so a server restart keeps the campaign; `--save none` disables. Tested: restart with the same db and save file keeps the player, their single probe, the advanced time and the scheduled burn.
- Accounts for resume continuity: `Welcome.token` identifies the player; a later `Hello` with that token resumes the same player id and keeps their vessels (no duplicate probe). Stored hashed in a small JSON file (`--db ose-accounts.json`, `:memory:` for tests); JSON instead of SQLite so the static Linux server cross-compiles without a C toolchain. Session test covers resume.
- Warp arbitration: each player's `RequestWarp` is a request; the universe runs at the lowest request among connected players, and `TimeState.warp_limited_by` names the player holding it down. Leaving releases the hold. Tested with two clients.
- Burns on rails: `ClientMessage::ScheduleBurn { vessel, ut, prograde, normal, radial }` schedules an impulsive burn in the vessel's orbital frame (`ClearBurn` removes it; `BurnState` answers). The step clamps warp so the burn lands exactly on its tick, applies it, and the streamed `Trajectory` shows the pre-burn patch ending in `Maneuver` followed by the post-burn patches. Test: 841.6 m/s prograde from 100 km gives a 12,000 km apoapsis.
- Protocol v3: on join every player gets a probe in a 100 km Kerbin orbit; the server streams `Vessels` (star-space state, reference body, orbit elements, owner) with every snapshot and `Trajectory` (patched-conic patches with end kind) once a second per vessel. The session test checks both.
- `ose-bot` (ADR-0003): an end-to-end bot that plays the Mun campaign as a client over the protocol. Eleven named steps from server health to Mun landing, each `PASSED`, `BLOCKED` (feature missing, named), `SKIPPED` (no KSP2 data) or `FAILED`. Today 9 pass through the server: connect, join the campaign, launch the Kerbal K2 (48 parts, 6200 m/s), plan the transfer from the streamed orbit, schedule and tune the burn against the server's prediction (845.6 m/s, Mun periapsis 41 km), warp into the Mun's SOI as predicted, and capture with a 264 m/s retrograde burn into a 41 x 41 km Mun orbit. Blocked: powered ascent, landing. `cargo test --workspace` spawns a server and checks the frontier in `crates/ose-bot/tests/mun_run.rs`.
- `ose-client` (ADR-0002, ADR-0005): the browser map view. Connects to `/ws`, keeps the `BodyCatalog`, places every body from its Keplerian elements at the client's time estimate (resynced on each `TimeState`, extrapolated at the warp factor), and draws orbits, bodies, SOI circles and labels on a Canvas 2D map with pan, zoom, click-to-focus and `F` to reframe. HUD with Kerbin-calendar UT and warp buttons that send `RequestWarp`. Vessels from protocol v3 are drawn with their predicted trajectory patches, the camera follows the player's probe, and a maneuver-node panel plans one burn (`ScheduleBurn`, prograde/normal/radial) with the post-burn conics drawn from the server's reply. A main menu takes the player's name and Play, the resume token is kept in the browser so a reload gets the same player and probe, the header shows the player, the campaign members and who holds warp, and breadcrumbs name star > body > vessel. After Play the menu lists sessions (join, create with mode and difficulty presets) and the agency flag and colours; joining lands in a 2D KSC hub whose launch pad lists the craft library (stock and saved) and launches a craft into a chosen orbit; the Tracking Station button opens the map and the KSC button in the HUD returns to the hub. About 100 KB of WASM; native unit tests for the scene, camera and time formatting.
- `e2e/` (ADR-0004): Playwright drives the trunk-built WASM client in Chromium and Firefox against `ose-server --static-dir`. Eleven player steps (page, WASM start, clean console, WebGPU adapter, canvas, WebSocket, time, map bodies, warp, vessel, maneuver node) with the same `PASSED`/`BLOCKED`/`SKIPPED`/`FAILED` vocabulary and a frontier test. Today every player step passes: 15 in Chromium and 14 in Firefox (its WebGPU adapter is skipped headless), none blocked; the warp step is skipped when another connected player holds warp. The client exposes features to the test through `data-ose="..."` hooks listed in `e2e/README.md`. CI job `browser-e2e`; `OSE_BASE_URL=https://ose.jfly.dev npm test` checks the live site from the house (passes, 2026-10-23).
- `tools/ksp2_catalog.py` parses the Addressables catalog (keys, buckets, entries) and resolves any key to its bundle.
- `tools/extract_bundles.py` extracts every JSON data asset from the install into `assets/ksp2/data/` (gitignored): 798 files, including 400 part definitions, 22 celestial bodies, 14 resources, 87 tech nodes, 9 stock assemblies, missions and tutorials. Format is `{version, useExternal, data: {...}}`.
- `ose_data::craft::Craft`: a craft as a part tree (parent link with the attach nodes on both sides, local position and rotation, stage index, crossfeed, stored resources) plus staging lists, with `mass()` against the part and resource catalogs. `ose_importer::craft::craft_from_value` converts a KSP2 workspace/stock assembly file; the real-data test imports Stock Kerbal-K2 as a 48-part tree whose computed wet mass matches the file's own metadata (111.69 t) within 2%.
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
- Campaign content: missions and the tech tree from the imported JSON, science.
- Phase 5/6 server side: vessels built from the craft tree in the sim (parts, containers, flow graph per `docs/reference-notes/resource-flow.md`), finite burns consuming propellant with staging, then the rapier physics bubble.
- Convert the scaled-space planet meshes and textures to glTF/KTX2 for Phase 3.

## Known problems

- The client map view is Canvas 2D (ADR-0005); the 3D flight view (WebGPU) does not exist yet. Vessels of players who disconnected stay on the server with the same name, so a busy server shows stacked "browser probe" labels.
