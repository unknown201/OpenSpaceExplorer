# Rendezvous v2: launch to dock from anywhere

Status: **in progress** (2026-11-14). R1 is done (`crates/ose-guidance`). R2 and R3 are done (`crates/ose-pilot`; tests T1, T4 and T9 pass, run by CI with `--release`). R4 is done: `ose_pilot::soi` moves between spheres of influence, and T3, T5 and T6 pass. R5 is done: `ose_pilot::window` times launches from any landed site, and T2 and T7 pass. This is the prerequisite for [delivery routes](delivery-routes-design.md).
Josh decided that routes, including depot-to-depot routes, are built on a rendezvous system that
works from launch to dock, from KSC, from stations and depots, and from the Mun, including orbital
windows. That system has to be well tested before routes depend on it.

## 1. What exists (v1, 2026-11-11)

Described in [station-rendezvous-guidance-design.md](station-rendezvous-guidance-design.md) and
[reference-notes/station-rendezvous-guidance.md](reference-notes/station-rendezvous-guidance.md).

- `ose-math/src/rendezvous.rs`: a universal-variable Lambert solver with multi-revolution phasing,
  encounter checks, safe periapsis and fuel-budget rejection, midcourse corrections, relative motion,
  and **plane windows** for a rotating launch pad.
- `ose-client/src/rendezvous_plan.rs`, `rendezvous.rs`, `rendezvous_ui.rs`: a rolling advisory plan
  (transfer, correction, match velocity) that replans after each manually flown burn, plus port
  approach readouts, cues and UI. **The plan lives in the client and is advisory only.**
- `ose-sim/examples/iss_assembly.rs`: a test pilot that launched ten ISS modules from the surface and
  docked all of them with ordinary controls and RCS. **It is example code, not a library.**
- Docking physics: port capture, physics retention near a target, stable merges and splits.

## 2. Limits that block routes

| # | Limit | Why routes need it |
|---|---|---|
| L1 | The planner runs only in the WASM client, as advice | The server's route pilot has to plan and replan on its own |
| L2 | Nothing executes a plan: no automatic burns, staging or RCS approach outside the example | Every route run is flown unattended |
| L3 | One central body only (single `mu`), so no SOI changes | Kerbin to a Mun station, and Mun station to an LKO depot |
| L4 | The launch advice finds **plane** windows only. There is no launch-to-intercept forecast (when to launch so you arrive near the target), and it assumes a pad on Kerbin | A run must launch at the right moment, and ascents from the Mun |
| L5 | No departure from orbit or from a docked vessel: undock, back off, then plan | A depot-to-depot shuttle starts docked at a station |
| L6 | No return-home planning: deorbit to KSC, chutes, recovery | The Return home and Rotate modes |
| L7 | Cross-SOI predictions drift: the Mun bot's predicted periapsis is sometimes 10 to 20 km off the real one after the handoff (flaky `mun_run` CI). **Found (R4):** the handoff is accurate to about 5 km; the bot compared against the prediction for its impulsive correction node, but the engines fly the correction a few cm/s off it. The bot now compares against the server's prediction after the burn is flown | Cross-SOI routes depend on accurate encounters |
| L8 | Tests cover one case (KSC to an LKO station) | Josh asked for this to be well tested before routes depend on it |

## 3. Design

### 3.1 Shared guidance crate (L1)

Move the planner core out of `ose-client` into a new crate, `ose-guidance`. It depends on
`ose-math`, has no I/O, and builds for both native and WASM. The client keeps its UI and calls
the crate, and the server's pilot calls the same code, so a player's guidance and a route's pilot
always agree. A plan is a list of typed legs:

`Ascent → Phasing → Transfer → (SOI change → Capture) → Correction* → MatchVelocity → Approach → Dock`

The route back home is the same structure: `Undock → Depart → … → Deorbit → Descent → Landed/Splashed`.

### 3.2 Pilot library (L2)

Turn `iss_assembly.rs`'s flying into an `ose-pilot` crate that drives a vessel through the same
control inputs a player sends (attitude, throttle, stage, RCS translation, dock/undock). It covers:

- **Ascent** from any landed position on any body: a gravity turn for bodies with an atmosphere (the
  bot's `ascent.rs`), and a direct low pitch-over on airless bodies like the Mun.
- **Finite burns**: point, throttle, cut off on measured Δv, stage when a stage runs dry.
- **RCS approach and docking**: hold point, port-axis corridor, closing-speed schedule, capture.
- **Departure**: undock, back off with RCS to a safe distance, then hand over to the plan.
- **Descent and recovery** on Kerbin: deorbit burn targeting KSC, chutes, recovery.

The pilot replans with `ose-guidance` after every burn, as the v1 advisory plan already does.

### 3.3 Patched-conic transfers and windows (L3, L4, L7)

- **Between bodies.** Kerbin↔Mun and Kerbin↔Minmus, and later planet to planet. Find the window by
  searching departure time and time of flight: a phase-angle estimate first, then patched-conic
  refinement against the server's own patch propagator, so predictions match what the server will
  actually do. Then a capture burn and a same-SOI rendezvous with the target.
- **Launch-to-intercept window.** Combine the plane window with a **phase** check: the launch time
  where an ascent profile (time to orbit and downrange angle, measured per craft from its first
  flight or the route's proving flight) puts the vessel a fixed distance behind the target. Works for
  any landed site, not only the KSC pad.
- **Fix the handoff drift (L7) first.** Find out why the Mun bot's predicted periapsis differs from the
  periapsis after the SOI handoff, and fix it so cross-SOI encounters can be trusted.

### 3.4 Player-facing improvements

The client shows the same legs for any origin and destination: KSC to a Mun station, a station
to another station, the Mun surface to orbit. It also gets an "upcoming windows" list for the
selected target. Throttle stays manual for players. An optional "fly it for me" autopilot can be
added later on top of `ose-pilot`, but it is not part of this plan.

### 3.5 How cross-SOI transfers work (R4, done)

`ose_pilot::soi::SoiTransfer` searches the server's own patched-conic propagator
(`ose_sim::trajectory::predict`) over departure time and prograde Δv for the arrival periapsis
closest to the goal (the target's orbit), preferring arrivals that orbit the same way round. It flies
the burn with the engines, re-predicts from the orbit actually flown and corrects until the arrival is
within 15 km, then brakes into a circular orbit at periapsis. `Mission` hops one SOI at a time (down to
a moon, up to the parent, or up then down between sibling moons) and then runs the same-SOI
rendezvous around the target's body. The transfer floor is the top of the atmosphere, or 10 km over an
airless body.

Measured (release build): T3 KSC → Mun station, 845 m/s transfer, 5 small corrections, docked at
UT 49 946 s. T5 LKO station → Mun station, 837 + 26 m/s, docked at UT 31 974 s. T6 Mun station → LKO
station, 252 m/s out, 833 m/s capture, docked at UT 35 208 s. Low-thrust captures land a little off
circular (T6 captured 82 × 204 km), and the same-SOI phase then corrects for it.

### 3.6 How launch windows work (R5, done)

A rocket's ascent is described by a `LaunchProfile`: time to orbit and the downrange angle from the
site to the insertion point. It is measured from a real flight (`Ascent::profile`, kept by the
mission as `flown_profile`), which is exactly what a route's proving flight provides.
`window::search` rotates the site with its body over the next two days and scores each lift-off time
by how far the site is out of the target's plane (40 per degree, about the Δv cost of fixing it) and
how far the target will be from 2° ahead at insertion (0.5 per degree, since phasing only costs time),
plus an hour of waiting per point. The mission waits on the ground at warp, then the ascent steers its
heading toward the missing orbital velocity in the target's plane, which allows for the ground's
rotation. Airless bodies use `AscentParams::airless` (pitch over at 150 m, no throttle limit).

Measured: T7 (Mun surface to a 30 km station) needs 9.5 + 1.9 + 0.7 m/s of rendezvous burns and 2 m/s of
braking with the window, against 76 m/s, eight plans and 94 m/s of braking without it. T2 (KSC to a 20°
station) lifts off 0.03° from the plane and docks after two plans.

## 4. Test matrix (L8)

Every scenario runs headless on the server sim, with stock craft, ordinary controls, and no teleports.
Each one passes only if it docks, survives, and stays within a Δv margin of the ideal transfer.

| Id | From | To | Notes |
|---|---|---|---|
| T1 | KSC pad | Station in LKO, 0° | v1's case, now flown by `ose-pilot` |
| T2 | KSC pad | Station in LKO, 20° inclined | Plane window |
| T3 | KSC pad | Station in Mun orbit | Cross-SOI, launch-to-intercept |
| T4 | Docked at LKO station | LKO depot at a different altitude | Undock, depart, same SOI |
| T5 | Docked at LKO station | Mun-orbit station | Cross-SOI departure window |
| T6 | Docked at Mun station | LKO station | Return-direction transfer |
| T7 | Mun surface | Mun-orbit station | Airless ascent and phasing |
| T8 | Docked at LKO station | KSC (recover) | Deorbit and landing near KSC |
| T9 | Two pilots, same port, same window | Station | Collision behaves physically, with no crash or stuck state |

T1 to T7 and T9 run in normal CI, optimised (`cargo test --release -p ose-pilot`, about 3 min with the build); unoptimised they take far longer, so debug builds skip them. The flaky `mun_run` test is fixed as part of L7, not relaxed.

## 5. Delivery order (one PR each, each green before the next)

1. **R1**: `ose-guidance` crate. Move the v1 planner with no behavior change, and the client uses it.
2. **R2**: `ose-pilot` from the ISS example. T1 passes in CI.
3. **R3**: departures and same-SOI depot to depot. T4, T9.
4. **R4**: fix the SOI handoff drift (L7), then patched-conic transfers and windows. T3, T5, T6.
5. **R5**: launch-to-intercept windows from any site. T2, T7.
6. **R6**: deorbit and recovery at KSC. T8.
7. **R7**: client guidance for all cases, plus the windows list.

Then the delivery routes build plan (section 5 of the routes doc) runs on top of this.
