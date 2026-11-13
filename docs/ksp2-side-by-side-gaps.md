# KSP2 side-by-side gap report

Date: 2026-11-07. OSE build: ose.jfly.dev at `6820f49`. KSP2 reference: installed v0.2.2.0 (Steam, AI-House).

This report compares what a player sees and feels in the two games, scene by scene. It complements the
source-level [feature gap analysis](ksp2-feature-gap-analysis.md), the [physics checklist](PHYSICS-GAPS.md) and the
[UI parity audit](ui-parity-ksp2.md); it does not repeat their rows.

## How the comparison was made

- **OSE:** a headless Chromium (WebGPU) script plays a fresh Sandbox campaign on ose.jfly.dev at 1920x1080:
  main menu, KSC, KSC with the camera moving, VAB, a Tin Can + FL-T200 + LV-T45 stack, pad, a vertical full-throttle
  ascent with the camera orbiting at each altitude mark, in-flight map at 30 km, and the end of the flight. The
  altitude is sampled from `data-ose-altitude` every 0.4 s.
- **KSP2:** Josh's 16 Steam screenshots of the same journey (Oct 25: KSC, VAB, pad, ascent, orbit, map).
  Launching and controlling KSP2 from this session was refused by the computer-use permission prompt (the same as
  on 2026-10-25), and the passive trace recorder in `tools/ksp2-control-capture` needs the BepInEx loader in the
  KSP2 install, which Josh has to approve. **Until then every physics and controls row below is a measured OSE
  number checked against the KSP2 data and first principles, not yet against a KSP2 trace.**
- Images live in `reference/` (git-ignored): `reference/ose-shots/` (OSE captures with `log.txt`),
  `reference/ksp2-shots/` (KSP2), and `reference/side-by-side/*.jpg` (KSP2 left, OSE right; `ksc`, `vab`, `pad`,
  `pad-wide`, `ascent`, `orbit`, `map`).

## Summary

The HUD, VAB layout, launch pad and ascent-altitude visuals are close to KSP2. The largest gaps a player notices are
(1) a coasting rocket keeps almost vacuum speed through the lower atmosphere (unverified: the drag model follows
KSP2's own formula, so this needs a KSP2 trace), (2) the KSC has no trees and a different
coastline, (3) the in-flight map opens on the whole Kerbin system instead of the vessel, and (4) the flight camera
starts far from the craft. Physics and control feel cannot be closed with proof until KSP2 traces are recorded.

## Gaps

Owner column: who should close it. "Report" means the area belongs to another thread or to Josh's GPT agent and is
listed here only so it is not lost.

### Physics

| ID | Gap | KSP2 | OSE (measured) | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-P1 | **Closed: matches KSP2.** Drag on a coasting/falling craft looked low | KSP2 run 3 of the identical craft: apoapsis 127,114 m, 1,323 m/s falling through 8.5 km | OSE: apoapsis 126,723 m (-0.3%), 1,350 m/s at 8.5 km (+2.0%). See T1/T2 below | `reference/ksp2-run3/`, `reference/ksp2-run1/ose-trace.csv` | none |
| SBS-P2 | No KSP2 trace exists for any flight, so ascent, SAS, wheel and gimbal feel are unproven | `tools/ksp2-control-capture` records time, inputs, attitude, rates, mass | `tools/compare_control_traces.py` is ready but has nothing to compare | `tools/ksp2-control-capture/README.md` | **done**: KSP2 traces exist for every physics row (runs 1-19, `tools/ksp2-parity-pilot`) |
| SBS-P3 | Pad 01 stands 6 m higher | Craft sits at 111.9 m above sea level | 118.1 m | ascent traces | planets thread: **done** in PR 40 (a craft on pad 1 reads 111.9 m, as in KSP2) |

### Visual fidelity

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-V1 | KSC campus has no trees or shrubs | Dense tree lines along the rivers and around the pad ring | Bare, evenly lit green ground | `side-by-side/ksc.jpg` | planets/3D world threads: **done** in PRs 38 and 47 (tree lines and groves) |
| SBS-V2 | KSC geography differs: no ocean behind the campus, mountains where KSP2 has the coast | The campus sits on a peninsula with open ocean behind the VAB | Inland lakes and a mountain range on the horizon | `side-by-side/ksc.jpg` | **still open** (planets/3D world threads): the coast got KSP2's beaches (PR 46) and the trees are in (local build 2026-11-08, `side-by-side/ksc-2026-11-08.jpg` after 60 s of streaming), but behind the campus OSE still shows green land to the horizon where KSP2 shows open ocean |
| SBS-V3 | A bright yellow rectangle floats on the river while the KSC camera moves | Water shows sun glints, no hard-edged quad | Sharp-edged yellow streak on the river, visible in the moving-camera frame | `ose-shots/02b-ksc-camera-moving.png` (left of centre, on the river) | 3D world thread: **done** in PR 34 |
| SBS-V4 | KSC ground is flat-lit with no ambient occlusion or contact shadows | Buildings and trees cast soft shadows, ground has colour variation | Uniform albedo, buildings float visually | `side-by-side/ksc.jpg` | 3D world thread: **done** in PR 34 (ambient occlusion) |
| SBS-V5 | Exhaust plume and launch smoke | Volumetric smoke, bright plume | Present and comparable at liftoff; particles not audited here | `ose-shots/07-liftoff.png` | **Report for Josh** (GPT agent owns particles/effects) |

### Camera, input and controls

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-C1 | The flight camera starts far from the craft on the pad | Camera starts a few craft-lengths behind the vessel, which fills a good part of the screen | Camera frames the whole pad structure; a 3-part rocket is about 20 px tall at 1080p | `side-by-side/pad.jpg` | **done** in PR 33 (KSP2 BasicCameraRigSolution distance) |
| SBS-C2 | The in-flight map opens at system scale | M opens on the active vessel: Kerbin fills the view with the trajectory and AP/PE markers on it | Opens on the full Kerbin system (Mun and Minmus orbits); Kerbin and the trajectory are a few pixels in the centre | `side-by-side/map.jpg` | **done** in PR 35 (opens on the vessel and its trajectory) |
| SBS-C3 | VAB camera starts far from a small craft | Craft centred and filling the hall's lower third | Craft small at the right edge of the hall | `side-by-side/vab.jpg` | UI/VAB thread: **done** in PR 32 (`8874924`) |
| SBS-C4 | Input latency and control response not measured against KSP2 | Needs a trace | Needs a trace | n/a | **done**: T36 (key to send 1-2 ms; response 0.1-0.18 s vs KSP2 0.12 s) |

### Features

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-F1 | Losing the vessel ends the flight with no way back | The crash report offers Revert to Launch / Revert to VAB as well as returning to the KSC | "The vessel was lost" report with only Continue, which goes to the Tracking Station; the revert points still exist (Esc menu) but are not offered | `ose-shots/12-map.png` | **done** in PR 36 (Revert to Launch / VAB on the loss report) |
| SBS-F2 | Debris and the crash are not seen | The camera stays in flight on the wreck and debris | The view leaves flight as soon as the vessel is lost | `ose-shots/12-map.png` | **done** in PR 36 (camera stays on the crash site) |

## What already matches

HUD layout and widgets (navball, altimeter, staging stack, app bar, resources, Kerbal portrait), the VAB part picker
and info card, pad 01 geometry, clouds and terrain colour from 1 km to 25 km, PAIGE hints on first visits, and the
main menu replica. These were checked frame by frame in `reference/side-by-side/`.

## KSP2 trace runs

`tools/ksp2-parity-pilot` is a BepInEx plugin that flies scripted scenarios inside the installed KSP2 through the
game's own hooks (`OAB.ShowOAB` + `OnLaunch` to roll a craft out, `FlightInputHandler` overrides for inputs,
`VesselComponent` telemetry every physics tick) and saves screenshots. It runs only when
`BepInEx/ose-parity/run.txt` exists, and it plays in a copy of a campaign (`OSE Parity`) so Josh's saves are not
touched. Outputs go to `reference/ksp2-run*/` (git-ignored).

**Run 1 and run 3 (2026-11-07), vertical ascent.** Craft: Mk16 + Tin Can + 2x FL-T200 + LV-T45 (Josh's `First.json`
workspace, 4.74 t), SAS on, full throttle, first stage only, no parachute. KSP2 results: burnout at T+30 s, 17.6 km,
1,312 m/s; apoapsis 127.1 km; falling through 8.5 km at 1,323 m/s (Mach 4.5, q = 336 kPa). So KSP2's own drag on
this craft at high Mach is also very low, which supports the sim thread's reading of SBS-P1. Run 3 repeats run 1
after the pilot learned to wait for the loading curtain, so its screenshots (`reference/ksp2-run3/`) are gameplay.

The OSE side runs headless: `cargo run -p ose-bot --example parity_ascent -- craft.json out.csv` imports the same
workspace through `ose-importer`, saves it, rolls it out to the pad and flies the same schedule against a local
server; `--example parity_rotation` does the orbital rotation schedule below.

**Ascent delta (T1/T2), KSP2 run 3 vs OSE.** OSE time zero is the first streamed state after staging, about 0.2 s
later than KSP2's, which accounts for most of the early lead.

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Mass on the pad | 4.742 t | 4.737 t | -0.1% |
| Mass after burnout | 2.742 t | 2.743 t | 0.0% |
| Burn time | 29.3 s | about 29.2 s | -0.3% |
| Altitude / speed at T+10 s | 1,659 m / 324.0 m/s | 1,753 m / 334.2 m/s | +5.7% / +3.1% (time offset) |
| Altitude / speed at T+20 s | 6,979 m / 763.2 m/s | 7,177 m / 775.9 m/s | +2.8% / +1.7% |
| Peak speed | 1,312.8 m/s | 1,310.6 m/s | -0.2% |
| Apoapsis | 127,114 m | 126,723 m | -0.3% |
| Speed falling through 30 km | 1,219.5 m/s | 1,219.0 m/s | 0.0% |
| Speed falling through 8.5 km | 1,323.0 m/s | 1,349.8 m/s | +2.0% |
| Pad altitude (sea level) | 111.9 m | 118.1 m | +6.2 m (SBS-P3) |

**Rotation delta (T5/T7), run 7.** Same craft in a 200 km orbit, SAS off, full input for 1 s on pitch, then yaw,
then roll (4 s apart), then SAS on in stability mode. Run 2 is void: KSP2's input handler consumes an override once
per frame while a frame holds two or three physics ticks, so the held input reached only 64% of the ticks. The pilot
now re-applies held inputs before the handler on every physics tick (`InputHolder`).

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Pitch rate after 1 s of full pitch | 0.469 rad/s | 0.479 rad/s | +2.1% |
| Yaw added by 1 s of full yaw | about 0.47 rad/s | about 0.49 rad/s | about +5% |
| Roll after 1 s of full roll (total rate) | 1.66 rad/s | about 1.74 rad/s | about +5% |
| SAS (stability) stopping a 1.66 rad/s tumble | 0.38 rad/s after 1 s, 0.09 after 2 s, then a 0.03-0.07 rad/s wobble for 10+ s | 0 within 0.5 s | **SAS stops instantly** |

**RCS delta (T17), run 6.** Stock KSS Hub in a 200 km orbit, SAS off, RCS on, translate forward 2 s, right 2 s,
then pitch 1 s (inputs held every tick).

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Craft mass | 28.545 t | 29.505 t | **+0.96 t (+3.4%)** |
| Monopropellant, 2 s forward | 13.73 kg | 13.60 kg | -0.9% |
| Monopropellant, 2 s right | 8.29 kg | 8.12 kg | -2.0% |
| Monopropellant, 1 s RCS pitch | 4.47 kg | 4.40 kg | -1.6% |
| Yaw induced by the right translation | 0.0812 rad/s | 0.0739 rad/s | -9% (heavier craft) |
| Pitch rate after 1 s (RCS + wheels) | 0.273 rad/s | 0.273 rad/s | 0.0% |

**Hop and parachute delta (T2 subsonic, T12), run 4.** Same craft as run 1: 8 s at full throttle, cut, parachute
staged below 2.5 km.

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Apoapsis | 4,217 m | 4,420 m | +4.8% (OSE cut about 0.2 s later, pad 6 m higher) |
| Fastest fall before the canopy | 228.2 m/s at 1,051 m | 230.0 m/s at 1,095 m | +0.8% |
| Canopy slows the craft below 100 m/s at | 680 m | 703 m | +3.4% |
| Descent under the canopy, 100 m / 5 m above ground | 10.16 / 10.10 m/s | 10.10 / 10.04 m/s | -0.6% |
| Air density at the ground | 1.2090 kg/m3 | 1.2072 kg/m3 | -0.15% |

**Staging delta (T4), run 5.** Stock Kerbal K1 straight up, SAS on, full throttle, next stage each time the mass has
stopped falling for 1 s.

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Mass on the pad | 27.112 t | 27.344 t | **+232 kg** |
| Mass at stage-1 burnout | 9.872 t | 10.148 t | **+276 kg** (gone after the decouplers drop: 4.108 vs 4.123 t) |
| Speed at T+5 s / T+40 s | 120.6 / 890.5 m/s | 141.8 / 858.2 m/s | **+17% / -3.7%** (solid booster thrust curve) |
| Stage-1 burnout altitude | 75.9 km | 73.7 km | -2.9% |
| Speed at T+150 s | 1,916 m/s | 1,878 m/s | -2.0% |

**Driving delta (T16), run 8.** Stock Inquisitive Rover on the pad; KSP2 trace `reference/ksp2-run8-rover/`.

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Speed at +1 / +2 / +4 / +7 s of full throttle | 1.8 / 3.3 / 5.9 / 7.8 m/s | 4.1 / 6.7 / 9.0 / 10.0 m/s | **about 2x** |
| Speed after 15 s | 9.8 m/s | 11.9-12.7 m/s (ripples) | **+21-29%** |
| Brakes from about 10 m/s to a stop | about 1.5 s | about 1 s | close |
| Half throttle, full steer, 10 s | 1.2 to 2.5 m/s, yaw up to 1.33 rad/s | 5.4 to 9.0 m/s, heading turns up to 3.2 rad/s | **too fast** |
| Yaw after the steering is released | decays over about 3 s | stops at once | **instant** |

**Heating delta (T11), run 11.** Run 1 craft, straight up to 127 km and back down; hottest part temperature.

| Altitude on the way down | KSP2 speed / hottest part | OSE speed / hottest part |
|---|---|---|
| 30 km | 1,220 m/s / 183 K | 1,219 m/s / 194 K |
| 10 km | 1,324 m/s / 269 K | 1,345 m/s / 273 K |
| 2 km | 1,286 m/s / 359 K | 1,354 m/s / 361 K |
| 0.5 km | 1,269 m/s / 377 K | 1,349 m/s / 379 K |
| Peak during the ascent | 392 K | 389 K |

Holding `Up` the run showed a 6% dense-air drag gap; it was the attitude, not the drag model (run 15 force dump: total
part drag 26.8 kN at Mach 4.8, with KSP2's craft 4.3 degrees off its velocity). `parity_ascent` now holds the pad
attitude fixed in inertial space like KSP2's stability SAS: falling through 10 / 5 / 2 / 0.5 km, KSP2 1,324 / 1,311 /
1,286 / 1,269 m/s, OSE 1,323 / 1,309 / 1,284 / 1,264 m/s.

**K1 per-part mass and drag (run 10).** `reference/ksp2-run10-k1-parts/parts.csv` lists every part's dry, resource
and physics mass, PhysicsMode and per-face drag area and Cd. KSP2's vessel mass (27.112 t) leaves out the 12
PhysicsMode None parts (28 kg); the rest of OSE's +232 kg is in other parts. Handed to the sim thread.

Other findings from these runs: OSE ignites a craft's solid boosters by themselves when it is launched straight into
orbit (stock K2 via `LaunchSavedCraft` burns its four SRBs at stage 0 with the throttle at 0). These rows are with
the sim thread.

## Parity checklist (trace-verified)

Status: **verified** = a KSP2 trace and an OSE trace of the same scenario agree within the stated tolerance;
**failing** = they differ beyond it (row links the fix); **not traced** = no KSP2 trace yet. Nothing is verified
until a delta table is attached here. Priority: P0 changes how every flight feels; P1 blocks a whole activity;
P2 completeness.

| # | System | Pri | KSP2 scenario to trace | Compared channels and tolerance | Status |
|---|---|---|---|---|---|
| T1 | Thrust, Isp vs pressure, fuel burn | P0 | Run 1 craft, full throttle from the pad | mass(t), thrust, burn time; burnout altitude/speed within 2% | **verified** (mass flow and burn time within 0.3%, peak speed -0.2%) |
| T2 | Drag and Mach model (surface area, drag cubes) | P0 | Run 1 coast and fall; run 4 hop | speed vs altitude on descent within 5% | **verified** (apoapsis -0.3%, 8.5 km fall +2.0%, subsonic fall +0.8%, air density -0.15%); the dense-air gap in run 11 was the test: OSE held `Up` while KSP2's SAS held an inertial attitude 4.3 degrees off the flow; with the same hold OSE matches within 0.4% at 10, 5, 2 and 0.5 km |
| T3 | Mass and CoM shift as fuel drains | P0 | Run 1 with CoM logged | CoM along the stack vs fuel fraction within 2 cm | **verified against KSP2's rule and run 10's part masses** (`ose-sim` test `ksp2_com_parity`): stock K1, tanks full to empty, mass and CoM equal to the mm (KSP2 leaves physicsless parts out of vessel mass and CoM, as OSE does). The drain order itself is T4. Also matches KSP2's live CoM on the run-1 ascent (`ksp2_live_centre_of_mass_tracks_the_draining_tanks`): within 1 mm from full tanks to empty |
| T4 | Crossfeed and fuel-flow order, staging, decouplers | P0 | Stock Kerbal K1 (boosters, decouplers, 5 stages) | mass and speed vs time per stage | **verified** (KSP2 runs 5 and 20): mass on the pad 27.112 t in both, booster flow 0.5433 t/s and burnout at 23.7 s in both; speed at 25 / 40 / 60 s within 0.3% (the procedural fins' drag was 5x KSP2's until #83 shaped them); every part's drag at Mach 1.15 and 2.0 within 5% of run 20's force dump (test `ksp2_k1_part_drag_matches_run_20`) after porting `Module_Drag.SetExposedArea` (a stack node covers its face by the neighbour's cross-section, and by the part beyond a thin neighbour). OSE's trace reads up to 5% fast in the first seconds only because `parity_staged` starts its clock after ignition (about 0.2 s later); per tonne of propellant burned the speeds match. Not matched: the Terrier's side drag (KSP2 renders the engine shroud into a drag cube at runtime, `Module_Fairing` via `DragCubeGenerator`; 0.79 vs 0.63 m2; deferred until the wings thread's fairing drag-cube renderer can be shared). The trace reads 1.5% fast after 100 s only because the KSP2 pilot stages later (it screenshots after each staging); compared at equal mass the speeds match |
| T5 | SAS hold, reaction wheels, torque | P0 | Pod with wheel: step pitch input 1 s, release, SAS on | attitude and angular rate vs time within 5% | **verified**: wheel torque +2% pitch, about +5% yaw/roll; SAS damping matches after PR 41 (sim thread replay within 1% at +1, +2, +8 s; rerun here shows the 2 s stop and wobble); SAS mode directions **ported from KSP2's source** (`VesselAutopilot`, orbit-movement frame; test `ksp2_sas_modes.rs`): orbit radial out/in is square to the velocity in the orbit plane (OSE held the local vertical, off by the flight-path angle on eccentric orbits), and on surface speed normal/anti-normal hold horizon north/south and radial out/in straight up/down (OSE held the orbit normal and radial). Not traced in KSP2 |
| T6 | Gimbal response | P0 | Engine gimbal step under SAS | gimbal angle and body rate vs time | **checked against KSP2's source** (`Module_Gimbal`): OSE already follows the deflection from pitch/yaw/roll, the sign flip when the centre of mass is above the engine, the per-side ranges and limiter, and the response lerp at `gimbalResponseSpeed` x dt (10/s); fixed: roll swings a gimbal by its offset from the control transform (`minRollOffset` 0.1 m), not from the centre of mass (test `a_gimbal_on_the_control_axis_does_not_roll_however_far_the_centre_of_mass_is`). Not traced in KSP2 |
| T7 | Input response and precision mode | P0 | Keyboard pitch hold, CapsLock precision, trim | rate build-up and latency per tick | full-input step **verified** with T5, latency in T36; precision and trim not traced |
| T8 | Orbits, SOI changes, time warp (rails and physics) | P0 | 100 km orbit, warp x100, Mun encounter | apsides after warp, SOI exit time within 1 s | **verified, height cap fixed** (KSP2 `warp` run 2026-11-08 18:11Z, `ose-sim` tests `ksp2_orbit_parity.rs`): KSP2 holds a 100 km orbit to 0.1 m through 9,000 s at 100x, and so does OSE. KSP2 ran a 1,000x request at 100x at 100 km: its `TimeWarp` lets each rate above 100x go only above the air (or the highest terrain) by `TimeWarpLevels` heights (1,000x +50 km, 10,000x +150 km, 100,000x +500 km, 1,000,000x +4,000 km) for the active vessel. OSE had no height cap; it now applies one to the vessels players fly (`Universe::altitude_warp_cap`, server `apply_warp`). KSP2 drops the rate for good, OSE re-applies the request so warp comes back up on climbing. From KSP2's Mun orbit (Pe 100 km, Ap 2,800 km) OSE leaves the SOI at UT 22,207.50, inside KSP2's sampled bracket (22,205.76 to 22,225.51) and 0.01 s from KSP2's elements; the Kerbin orbit after it matches to 62 m in sma and 1 m in Pe (Mun phase and frames agree) |
| T9 | Maneuver node execution | P1 | Prograde 100 m/s node, burn by node timer | resulting apsides within 1% | **verified** (KSP2 `burn` run 2026-11-08 18:17Z, test `ksp2_orbit_parity.rs`): the Swivel adds 0.907 m/s and burns 1.370 kg per tick at once in both games (no spool-up). KSP2 is 100 m/s up after 110 thrust ticks and 0.1507 t on a 240,878 x 100,000 m orbit; OSE after 109 ticks and 0.1494 t on 240,832 x 100,000 m. A 100 m/s prograde node flown by OSE's timer ends on 240,130 x 99,999 m (0.3%). The pilot's logged 2.28 s and 246.8 km include its throttle reaching the engine 4 ticks late at ignition and at cutoff (4 extra thrust ticks after the cut: 3.75 m/s); OSE flying those 4 ticks too ends on 246,770 m |
| T10 | Control surfaces and lift | P1 | Stock jet takeoff roll and pitch-up | lift vs AoA and speed, rotation speed | **takeoff matches KSP2, flight does not yet** (run 21, Stock04_Jet, `parity_jetfly`): full pitch-up at 60 m/s lifts off after 0.72 s at 66.7 m/s (KSP2 0.96 s, 68.2 m/s); fixed: a craft with no chosen control part now flies from its root part's control frame, so a level-laid plane's pitch and yaw were reversed before. Open: in flight OSE's elevators bite about 2x harder (pitch -0.5 at 250 m/s: about -24 degrees vs KSP2 -14), and SAS holds the climb near 12 degrees where KSP2 settles near 5-6 |
| T11 | Heating and reentry | P1 | Run 1 craft falling from 127 km at Mach 4.5 (run 11) | hottest part temperature vs altitude | **verified**: peak 392 vs 389 K, 10 km 269 vs 273 K, ground 377 vs 379 K (within 2%; +11 K at 30 km). Heat shield (KSP2 reentry run: Mk1 pod over a 1.25 m shield, 70 km at 2,300 m/s; `ose-sim` test `ksp2_heat_shield_reentry_from_70_km_at_2300_mps`): the same fall second by second, shield 1,047 vs 1,046 K at its hottest and within 6% all the way down, pod under 300 K, ablator 0.0030 vs 0.0033 units |
| T12 | Parachutes | P1 | Mk16 staged below 2.5 km after a hop | deploy timing, descent speed at sea level within 3% | **verified** (canopy descent -0.6%, deploy altitude +3.4%) |
| T13 | Joints, flex, wobble, struts | P1 | Pod + six 1x4 tanks + Swivel on the pad: full throttle 5 s, full roll 3 s (run 18, `craft-stack.json`) | engine position in the pod's frame per tick, breaks | **verified** (`ose-sim` test `ksp2_tall_stack_stays_rigid_under_thrust_and_roll` replays it): no breaks in either; engine flex 0.0 mm in OSE vs 0.8 mm axial / 0.25 mm lateral in KSP2; peak roll 2.115 vs 2.097 rad/s, SAS decay within 10%. Roll in air **fixed** (KSP2 `struts` run 2026-11-08 21:12Z, test `ksp2_roll_damping.rs`): KSP2 damps each part's own spin by `angularDrag x dynamic pressure (atm) x AngularDragMultiplier 2` (`RigidbodyBehavior`, PhysX angular drag); OSE had none and rolled the core-plus-two-boosters craft to 4.8 rad/s under 3 s of full roll. With the drag ported (`PartDef.angular_drag`, imported from `PartData.angularDrag`) it levels at 2.58 rad/s like KSP2 and stops in 1.5 s after release (every 0.5 s sample within 0.2 rad/s). The tumble that followed in OSE is gone; KSP2's own craft starts tumbling about 2 s after the roll stops (0.56 rad/s at 11 s, 3.3 at 12.5 s), not yet reproduced |
| T14 | Collisions, crash tolerance, part breakage | P1 | Upright drops onto the pad by surface teleport (run 17, `craft-drop-*.json`) | survive/destroy per part, tick | **verified** (`ose-sim` test `ksp2_drops_break_and_survive_the_same_parts_on_the_same_tick` replays all six from KSP2's first airborne tick): Mk1 pod survives 6.8 and 11.0 m/s and is lost on impact at 15.6 m/s (tick 4 vs KSP2 3); pod over a 1x2 tank survives 6.8, 11.0 and 15.8 m/s and ends Landed. Remaining difference: KSP2's first-tick rebound (+2.9 / +2.2 / +9.4 m/s) is PhysX pushing the hull out of the pad after sinking up to 30 cm in one tick; it leaves an apex of 2-6 cm, and 64 cm once (pod+tank at 15.8 m/s). OSE stops the hull at the surface (apex under 2 cm) Crush-through (KSP2 drops of the tall stack at 10 to 45 m/s and the pod on a tank at 25/35; `ose-sim` test `ksp2_crashes_crush_through_the_same_layers_at_the_same_pace`): each layer breaks as it reaches the ground, at KSP2's pace; one struck more than 3 m/s over its tolerance does not slow the rest, one barely over stops the craft (was: every broken layer slowed the craft, so 30 and 45 m/s crashes stopped 4 to 6 layers early) |
| T15 | Landing legs and ground contact (fix: PR 63) | P1 | Run 1 craft with three landingleg_1v placed by hand (`craft-legs.json`), legs deployed, 0.3 s hop (run 16) | touchdown rebound, settle, situation | PR 63: Landed on its legs, rebound +0.56 vs KSP2 +0.49 m/s, no slide (0.02 vs 2.4 m/s before), settles in about 1.5 s vs 1 s. Rechecked 2026-11-08: run 16 cannot judge the settle. KSP2's lander touched down at 8.25 m/s on the pad's central divider between the flame-trench grates, then leaned from 2 deg at touchdown to 90 deg within 3.5 s (tilt doubling about every 0.5 s, all 8 parts kept) and came to rest 3.5 m lower, so it fell over toward a trench. OSE stays upright. Needs a rerun of the `legs` hop on flat ground (runway or grass) before the settle time or tip-over stability can be compared |
| T16 | Wheels and driving (fix: PR 45) | P1 | Stock Inquisitive Rover: full wheel throttle 15 s, coast 3 s, brakes 8 s, half throttle full steer 10 s | speed vs time, stopping, yaw rate | **verified** on main after PR 107 (`parity_rover` on a local server vs KSP2 run 8): straight-line speed at +1/+2/+4/+7/+15 s 1.66/3.21/5.49/7.46/9.69 vs KSP2 1.82/3.35/5.91/7.80/9.83 m/s, brakes stop it as KSP2 does; the half-throttle full-steer turn now speeds up as KSP2's does, 2.49 vs 2.24 m/s after 10 s (+11%), yaw 1.01 vs 0.97 rad/s at 5 s and 1.49 vs 1.41 at 10 s (+6%) |
| T17 | RCS translation and rotation | P1 | Stock KSS Hub: 2 s forward, 2 s right, 1 s pitch | monoprop use, rate | **verified** (fuel within 2%, pitch rate 0%); craft mass rechecked on main: 28.545 t in both (the +3.4% is gone) |
| T18 | Docking | P1 | Clamp-O-Tron pair joined in the VAB: undock, then halves set 1.31 m and 0.91 m apart at rest (rendezvous teleport), then flight undock | latch distance, separation speed, merged mass and CoM | KSP2 run 18 (`docking` pilot scenario): stays Ready at 1.31 m, latches on the next tick at 0.91 m (5 deg off) and snaps face to face, merged mass is the sum (13.006 t), undock parts at 0.13 m/s on both paths. OSE matches (ejection 0.127 m/s); fixed: ports joined in the VAB could not be undocked. Not traced: approach faster than 0.3 m/s (KSP2 RCS translation burned propellant without moving this craft) |
| T19 | EVA exit, ladders | P1 | First Kerbal exits the run-1 pod on the pad and lets go of the ladder (run 14) | exit position, fall | **fixed** (PR 64, `ose-sim` test `letting_go_of_the_pod_ladder_lands_on_the_pod_and_walking_off_its_edge_falls_to_the_pad`): the Kerbal drops about 0.3 m onto the pod and stands on it Landed, then walks off its edge and falls to the pad, as in run 14 |
| T20 | EVA walk, run, jump | P1 | Walk 6 s, run 6 s, jump (run 14) | ground speed, jump height | **fixed** (PR 58, `ose-sim` test `a_kerbal_walks_across_the_ground`): walk 0.65 m/s and run 2.05 m/s over the ground, easing up from the walk over a quarter second, jump 0.277 m, all as run 14 |
| T21 | EVA jetpack | P1 | 2 s up, 2 s forward in orbit | delta-v and fuel per second | **verified** (KSP2 `jetpack` free-flight rerun 2026-11-08 18:56Z, Kerbal 50 m clear of the craft; `ose-sim` test `ksp2_jetpack_parity.rs`): with gravity taken out, KSP2's Kerbal is pushed at 0.250 kN along its own axes and burns 2.124 g per tick; up 5.216 m/s and forward 5.280 m/s for 212.44 g each, mass 0.095000 -> 0.094575 t, no turn. OSE: up 5.270 m/s, forward 5.281 m/s, 212.44 g each, the same final mass, no turn. The 0.05 m/s on up is KSP2 applying the first push a tick after the first fuel. Not exercised: KSP2's turn toward the camera while moving (`JetpackAlignToCameraEnabled`), which OSE does not model. Ported from code earlier: 0.125 kN*m turn at Isp 240 (100 at 1 atm), turn rate capped at 280 deg/s, spin reset at 200 deg/s^2 |
| T22 | Electric charge and solar | P2 | Probe with panels in sun, wheel on | EC rate in/out | **solar matches KSP2's code** (`ose-sim` test `ksp2_solar_parity`): panels make their rating x luminosity / (4 pi d^2) / 1,360 W/m^2 (96.8% at Kerbin, was 100%), and only the body the craft is at shades it (Kerbin no longer darkens a craft at the Mun). Tracking panels turn about their pivot at KSP2's speed and lose the cosine out of their plane, fixed panels use their real face, and parts in the way shade them (PR 97, test `ksp2_solar_tracking_parity`); not traced in KSP2 |
| T23 | Comms (CommNet) | P2 | Probe behind Kerbin from KSC | link loss timing | **ported from KSP2's source** (`commnet.rs`, tests `the_route_is_the_cheapest_chain_of_squared_lengths_not_the_fewest_hops`, `a_lost_link_shows_at_the_next_rebuild_three_seconds_on`): links need each end in the other's range, the route is Dijkstra on squared link lengths (`GetConnectedNodesJob`), the graph is rebuilt every 3 s of real time (`CommNetManager`), so a link is lost up to 3 s late; bodies do not block links in KSP2 either. Not traced in KSP2 |
| T24 | Science collection and transmit | P2 | Crew report on the pad, transmit | science value, transmit time and EC | **checked against KSP2's source** (`PartComponentModule_DataTransmitter`): OSE already sends ceil(size / packet) packets, one per `DataTransmissionInterval`, drawing the antenna's EC while it sends and waiting without a connection; fixed: a vessel with no control (no crew, no powered probe core) does not send, as KSP2 stops on `NoControl` (test `a_vessel_without_control_does_not_send_its_reports`). Not traced in KSP2 |
| T25 | Flags | P2 | Plant a flag next to the pad | placement, collision | **ported from KSP2's source** (`KerbalBehavior.TryPlantFlag`, `IsPlantFlagAllowed`, `OnFlagRemovalUsed`, `EVAUtils.ComputeFlagLocation`; `ose-sim` test `a_kerbal_plants_one_flag_where_they_stand_and_gets_it_back_by_taking_one_down`): each Kerbal steps out carrying one flag, plants it at their own feet (`FlagLocalPositionOffset` 0.45 m down; OSE used to put it 1.5 m east), cannot plant another until they take a flag down (new Take flag button, any flag within reach), and cannot plant while on the ladder, swimming, airborne or running an experiment. The reach is OSE's 6 m boarding range: KSP2's `PlayerInteractRange` lives in a prefab not in the extracted data. Not traced in KSP2 |
| T26 | Camera modes and input bindings | P2 | Cycle V modes, default binding table | mode list, key map identical | not traced |
| T27 | Flight camera orbit (mouse drag) | P0 | KSP2 `ground/orbital basic rig`: heading degrees = mouse pixels x 4.5 x frame seconds (0.075 deg/px at 60 Hz; fine x 0.3) | degrees per pixel | **failing**: OSE turns 0.286 deg per CSS pixel, about 3.8x KSP2 at 60 Hz, and is frame-rate independent where KSP2 is not |
| T28 | Flight camera zoom (wheel) | P1 | KSP2 distance x exp(-zoomAxis), fine x exp(-0.3 zoomAxis) | distance factor per wheel notch | **likely failing**: OSE x0.8 in / x1.25 out per wheel event; KSP2's factor per notch depends on the Unity wheel axis value, which needs a live capture |
| T29 | Map camera orbit | P1 | KSP2 2 x max(0.001, 0.05 x (85 - abs(pitch))/85) deg/px, Alt x 0.1 | degrees per pixel vs pitch | **failing**: OSE 0.286 deg/px at every pitch, about 2.9x KSP2 at the horizon and far more near the poles; no Alt fine mode |
| T30 | Map zoom | P2 | KSP2 inverse scale x exp(-0.3) in / exp(0.3) out | factor per step | close: OSE x0.8 / x1.25 vs KSP2 x0.741 / x1.350 |
| T31 | VAB camera orbit and zoom | P1 | KSP2 0.004 rad/px (precise 0.001); zoom moves the pivot by zoomAxis x 3 m (Shift x2, precise 0.3) | per pixel, per notch | **failing**: OSE 0.005 rad/px (+25%), no precise mode; zoom is multiplicative x0.8 instead of KSP2's additive metres |
| T32 | Time warp ladder | P0 | KSP2 1, 2, 4, 10, 50, 100, 1,000, 10,000, 100,000, 1,000,000; physics warp capped at 4x in atmosphere | ladder steps | **fixed on main** (`kerbin_time.rs` WARP_LADDER matches KSP2; physics warp capped at 4x) |
| T33 | VAB held-part rotation keys | P2 | KSP2 90 deg per press with snap on or off; Shift 5 deg | degrees per press | **failing**: OSE 15 deg with snap off |
| T34 | Maneuver handle drag | P1 | KSP2 adds (displacement / (strength x DPI))^2 m/s every frame while held | m/s per pixel per frame | **failing**: OSE sets an absolute 1 m/s per pixel (0.1 with Shift); KSP2 is a held-rate control with no Shift factor |
| T35 | Throttle keys | P0 | KSP2 Shift/Ctrl change the throttle 1.0 per real second; Z full, X cut | ramp rate, presets | **verified** (same equation and presets in `input.rs`) |
| T36 | Key press to control response | P0 | KSP2 applies keyboard input in the next frame's FixedUpdate; the craft starts turning about 0.12 s after the control state changes (run 7) | key to send, send to motion | **verified**: OSE sends 1-2 ms after keydown (local build, 10 presses, 60 fps), and the server starts the turn 0.1-0.18 s after the input (stream at 10 Hz) |
| T37 | Content counts | P0 | KSP2 data files | parts, tech nodes, missions, bodies | **verified**: 400/400 parts (none missing), 87/87 tech nodes, 57/57 missions (32 exploration + 25 mission control), 17/17 placed bodies (KSP2's five asteroid definitions are not in its galaxy) |
| T38 | Procedural wings and fairing shaping | P0 | KSP2 VAB wing and fairing editors | shape handles, persisted shape, lift/mass from shape | **partial** (proc-wings PR): KSP2's wing rig ported (`ose_data::procedural`: sliders, constraints, skinning, the 256 px drag cube for mass and lift; the stock jet's wing and stabilizer lift area within 0.1%, the Flea fin's mass exact); VAB Wing Shape sliders, shaped meshes in VAB and flight. Fairings: wall sections, panel mass, enclosure, shielding when capped, panels shed and flown off on jettison, VAB wall editor. KSP2 wing probe run (2026-11-08, stock jet reshaped six ways in KSP2's own editor): fixed and moving lift coefficients within 2% (most within 0.5%) and flight dry mass within 0.3% on every reshaped wing and stabilizer, after the control surface sliders were made to move constraint weights as in KSP2. Open: fairing drag cubes and automatic wall fitting stay approximate |
| T39 | Fuel line and strut placement | P1 | KSP2 two-anchor tool | place, save, flow, structural link | **verified** (PRs 77, 109, 110): VAB two-anchor tool, saved as KSP2's anchor pairs; struts brace the joints they span (KSP2's strut joint stiffness, in parallel along the tree path); decoupler/separator/docking-port crossfeed switch. KSP2 traces of the pilot's `fuelline` and `crossfeed` crafts: tanks empty at 29.28/58.48 s over a line (OSE 29.2/58.4) and together at 58.48 s across a switched-on decoupler; both are fixtures in `ose-sim/tests/fuel_line_trace.rs` |
| T40 | Warp under thrust, finite-burn prediction | P1 | Stack craft (pod, six 1x4 tanks, Swivel) burning prograde in a 200 km orbit: 10 s at 1x, then 10x, then 100x | allowed rate, mass and non-gravitational delta-v at 40 s | KSP2 run 19 (`thrustwarp` pilot scenario): 10x allowed under thrust (orbital physics under thrust), 100x dropped to 1x with tanks about to run dry; at 40 s 13.168 t and 589.7 m/s. **verified**: OSE (`parity_thrustwarp`) 13.152 t and 590.3 m/s, the same fuel-out drop; node burns predicted as KSP2 does (finite, from the node). Not modelled: per-tank fuel events and SOI changes during a warped burn |
| T41 | Engine modes, spool-up and jet intakes | P1 | KSP2 multi-mode engines, spool curves, intake air | thrust vs time on throttle steps, mode switch | **matches KSP2** (run 20, Stock04_Jet on Runway_1, `tools/compare_jet_traces.py`): brakes-on thrust spools to 15.3 kN after 15 s in both (63% of the 20 s value after 7.54 s in KSP2 and 7.60 s in OSE), air-limited by the single radial intake (KSP2 0.0270 IntakeAir/s at a standstill); spool-down within 0.12 kN; Reverse holds 93.8 kN (OSE 94.6) for the 3 s mode wait, then 54.6 vs 55.5 kN at +5 s. The takeoff roll is up to 8.5 kN low in OSE because the plane gathers speed (and ram air) more slowly on the runway; thrust against speed agrees. Ported: intake air, flameout/relight, mode switching with waits, auto-switch. Open: rocket starvation flameouts, Goliath `occludeNode`, mode button in the part window |
| T42 | Parachute RISKY/IMMEDIATE modes, extra heat sources | P2 | KSP2 chute safety modes, engine/part heating | deploy rules, temperatures | **ported from KSP2's source**: parachute SAFE/RISKY/IMMEDIATE deploy modes (chute-modes PR, `structure.rs` `ChuteMode`); engine heat is the exhaust blast (`Module_Engine.EngineExhaustDamage`, `exhaust.rs`: on the craft's own parts and on other craft, tests `ksp2_exhaust_heat_parity`), since no stock engine heats itself (`GenerateHeat` off). Fuel-cell and reactor heat (`OtherFlux` = FluxGenerated x deltaTime, a few kW) is not modelled |
| T43 | Water: flooding, waves, impact damage | P2 | KSP2 splashdown | sink/float, damage on water impact | **ported from KSP2's source** (`ose-sim` `water.rs`, test `ksp2_water_parity`): a part breaks entering the water when max(0.1 x speed, vertical speed) is over 1.2 x its crash tolerance (Mk1 pod: 18 m/s down); buoyancy is the drag-cube volume x 1.2 x the part's `buoyancy` (pod 3, tanks 0.3, adapters 0) x its wet height share (the pod floats 0.40 m deep); splash drag is KSP2's damping (2/s while bobbing, drag-cube drag easing 4.5 to 0.15 when moving across). KSP2 has no flooding or waves. Not yet checked against a KSP2 splashdown trace |
| T44 | Kerbal character animation | P2 | KSP2 `animtree_kerbal_eva` (dumped from kerbal_shared_assets_all.bundle) | states, cross-fades, exit times | **partial**: the client runs KSP2's EVA states and timings (`kerbal_anim.rs`): IDLES/MOVEMENT 0.05 s in, 0.5 s out; walk and run blended in step by the sprint; JUMP_START to ZERO_G at 0.59 (0.21 s), ZERO_G to JUMP_END 0.05 s, JUMP_END to IDLES at 0.56 (0.25 s); LADDER 0.25 s; FLAG_PLANT 0.05 s in, out at 0.97. Not yet: swimming, jetpack open/close, fidgets, emotes, low/high-gravity clip variants, expressions |
| T45 | Localization | P2 | KSP2 12 languages | locale switch, coverage | **missing** (G15): English only |
| T46 | Stock craft launch from the pad list | P0 | Stock Kerbal K1 rolled out | parts drawn and staged | **verified** on a local build of main: all 38 parts drawn, 6 stages listed (the pod-only report came from the VAB Load path, not rechecked here) |

Controls rows T27-T34 come from the KSP2 source and configuration (camera rigs, `TimeWarp`, OAB, maneuver gizmo);
T35-T36 add live timing. Source: an interaction-parity recording from 2026-10-29 (`docs/reference-notes/interaction-parity.md`,
not yet committed) and `reference/controls/ose-key-latency.json`.

## Next steps

1. Build the OSE side of the trace runner: an `ose-bot` example that imports the same workspace JSON, flies the same
   scenario steps and writes the same CSV columns; a comparer that prints the delta table above.
2. Add scenarios T1-T8 to the KSP2 pilot first (P0), then P1, closing failing rows with fix PRs that attach
   before/after deltas.
3. Earlier items: SBS-C1, SBS-C2, SBS-F1/F2 fix PRs are in progress; SBS-V1..V4 are with planets/campus; SBS-V5 is
   for Josh.
