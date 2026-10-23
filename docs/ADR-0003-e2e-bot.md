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
