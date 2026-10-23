# ADR-0003: An end-to-end bot that tracks progress toward the Mun

Date: 2026-10-23. Status: accepted (Josh, project thread).

## Context

The game is early: orbits, rails and SOI handoff exist, the server has a health endpoint and a ticking
universe, and networking, vessels, physics and campaign flow are still to come. We want one automated check
that plays the game the way a player will, from a fresh client connection to a Mun landing, that runs today
and keeps running as features land, so the distance to "reach the Mun" is measured rather than guessed.

## Decision

1. **`ose-bot` is a real client.** A native crate (`crates/ose-bot`, binary and library) that talks to
   `ose-server` through the `ose-net` protocol over WebSocket, exactly as the browser client will. It never
   links `ose-server`; it spawns the server as a process (`harness::spawn_server`) or is pointed at one.
2. **The campaign is a fixed chain of named steps** (`steps::STEP_NAMES`): server health, hello, start
   campaign, load a stock craft, upload and launch, ascend, plan the Mun transfer, burn, coast to the Mun's
   SOI, capture, land. Each step ends in one of four outcomes:
   - `Passed(detail)`: done, with a one-line measurement.
   - `Blocked { feature }`: the game lacks the feature; the text names what must exist and the roadmap phase.
   - `Skipped { reason }`: this machine lacks data (the gitignored KSP2 extraction); not a game problem.
   - `Failed { error }`: the feature exists (or the step is pure math) and the bot could not do it.
   The chain never aborts on a blocked step, so later steps that can run still report.
3. **Local steps stand in for missing server features.** Steps that only need orbital mechanics run in the
   bot's own process over `ose-sim` and `ose-math` and are marked `mode: local`: the bot parks itself in a
   100 km Kerbin orbit on rails, plans a Hohmann transfer (`plan::plan_hohmann`: lead angle, wait, delta-v),
   applies an impulsive prograde burn and coasts with `update_reference_body` until the Mun encounter. When
   the server gains vessels and maneuver intents, those steps switch to `mode: server` and the local code
   becomes the oracle the server's answer is checked against.
4. **The test encodes a frontier.** `tests/mun_run.rs` lists, per step, `Pass`, `PassOrSkip` or `Blocked`.
   It fails on any `Failed` outcome and on a regression (an expected pass that is now blocked or skipped). A
   blocked step that starts passing is printed as `PROGRESS:` and does not fail, so the thread landing a
   feature is not punished; it moves the frontier in the same change.
5. **Body data has a fallback.** `game_data.json` is gitignored, so the bot carries a built-in
   Kerbol/Kerbin/Mun fixture with the importer's constants and uses the converted file when present.

## Consequences

- `cargo test --workspace` now builds and runs `ose-server` once (through `cargo run`), adding a few
  seconds; CI needs nothing new.
- `cargo run -p ose-bot -- --spawn-server` prints the progress table; exit status 1 means a `Failed` step.
- New protocol features should come with the matching bot step turned from `Blocked` into a real exchange.
  The first one is the WebSocket endpoint: `client::Client::connect` already speaks `Hello`/`Welcome` at
  `/ws` and only needs the server side.
- The Hohmann numbers (841.6 m/s from 100 km, 7.4 h, 110.7 degrees lead, SOI entry 5.5 h after the burn on an
  impact course) are the first Kerbin-to-Mun figures in the repo and become the parity target for the
  patched-conic predictor.

## Update 2026-10-23 (later the same day): the server flies the run

With protocol v4 (`LaunchCraft`, `ScheduleBurn`, `Trajectory`, `BurnState`) the orbital steps moved from
`mode: local` to `mode: server`, and the local rails model is now only the fallback when no server is given:

- `connect_hello` completes `Hello` -> `Welcome` -> `BodyCatalog` and finds the probe the server hands out.
- `upload_and_launch_craft` converts the stock assembly with `ose_importer::craft::craft_from_value` and
  sends `LaunchCraft`; the bot then flies the launched craft (Kerbal K2, 48 parts, 6200 m/s) rather than
  the probe. It is `Skipped` on machines without the extracted KSP2 data.
- `plan_mun_transfer` plans from the server's own orbit elements and catalog, with the burn at least 30 s
  ahead of the server's current time.
- `execute_transfer_burn` schedules the Hohmann burn, then reads the server's prediction back and raises
  the delta-v in 0.5 m/s steps (bisecting on overshoot) until the predicted Mun periapsis is 30 to 150 km
  above the surface. That is the bot's maneuver-node editing: about nine `ScheduleBurn` round trips.
- `coast_to_mun_encounter` requests warp 10,000 and waits for `Vessels` to show the craft around the Mun;
  it fails if the observed handoff disagrees with the prediction beyond one snapshot interval of warped
  time, or if the periapsis differs from the prediction by more than 20 km.
- `mun_capture_burn` schedules a retrograde burn at the next Mun periapsis sized to circularise, checks the
  server predicts a bound post-burn patch, warps past it and verifies the resulting orbit (e below 0.05).

`ascend_to_orbit` stays `Blocked`: the server places launched craft straight into a circular orbit, which
is a stand-in, not an ascent. `mun_landing` stays `Blocked` on terrain. `start_campaign` stays `Blocked`;
joining drops the player into the one shared universe.

The test harness starts the server with `--save none --db :memory:` so runs never read or write the
developer's save and account files.
