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
| SBS-P2 | No KSP2 trace exists for any flight, so ascent, SAS, wheel and gimbal feel are unproven | `tools/ksp2-control-capture` records time, inputs, attitude, rates, mass | `tools/compare_control_traces.py` is ready but has nothing to compare | `tools/ksp2-control-capture/README.md` | this thread, **blocked on Josh** (BepInEx install approval) |
| SBS-P3 | Pad 01 stands 6 m higher | Craft sits at 111.9 m above sea level | 118.1 m | ascent traces | planets/terrain thread |

### Visual fidelity

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-V1 | KSC campus has no trees or shrubs | Dense tree lines along the rivers and around the pad ring | Bare, evenly lit green ground | `side-by-side/ksc.jpg` | planets/trees thread |
| SBS-V2 | KSC geography differs: no ocean behind the campus, mountains where KSP2 has the coast | The campus sits on a peninsula with open ocean behind the VAB | Inland lakes and a mountain range on the horizon | `side-by-side/ksc.jpg` | planets/trees + 3D campus threads |
| SBS-V3 | A bright yellow rectangle floats on the river while the KSC camera moves | Water shows sun glints, no hard-edged quad | Sharp-edged yellow streak on the river, visible in the moving-camera frame | `ose-shots/02b-ksc-camera-moving.png` (left of centre, on the river) | 3D world thread: **done** in PR 34 |
| SBS-V4 | KSC ground is flat-lit with no ambient occlusion or contact shadows | Buildings and trees cast soft shadows, ground has colour variation | Uniform albedo, buildings float visually | `side-by-side/ksc.jpg` | 3D world thread: **done** in PR 34 (ambient occlusion) |
| SBS-V5 | Exhaust plume and launch smoke | Volumetric smoke, bright plume | Present and comparable at liftoff; particles not audited here | `ose-shots/07-liftoff.png` | **Report for Josh** (GPT agent owns particles/effects) |

### Camera, input and controls

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-C1 | The flight camera starts far from the craft on the pad | Camera starts a few craft-lengths behind the vessel, which fills a good part of the screen | Camera frames the whole pad structure; a 3-part rocket is about 20 px tall at 1080p | `side-by-side/pad.jpg` | this thread (flight camera) |
| SBS-C2 | The in-flight map opens at system scale | M opens on the active vessel: Kerbin fills the view with the trajectory and AP/PE markers on it | Opens on the full Kerbin system (Mun and Minmus orbits); Kerbin and the trajectory are a few pixels in the centre | `side-by-side/map.jpg` | this thread (map camera), confirm with UI thread |
| SBS-C3 | VAB camera starts far from a small craft | Craft centred and filling the hall's lower third | Craft small at the right edge of the hall | `side-by-side/vab.jpg` | UI/VAB thread: **done** in PR 32 (`8874924`) |
| SBS-C4 | Input latency and control response not measured against KSP2 | Needs a trace | Needs a trace | n/a | **blocked on Josh** |

### Features

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-F1 | Losing the vessel ends the flight with no way back | The crash report offers Revert to Launch / Revert to VAB as well as returning to the KSC | "The vessel was lost" report with only Continue, which goes to the Tracking Station; the revert points still exist (Esc menu) but are not offered | `ose-shots/12-map.png` | this thread (flight report) |
| SBS-F2 | Debris and the crash are not seen | The camera stays in flight on the wreck and debris | The view leaves flight as soon as the vessel is lost | `ose-shots/12-map.png` | this thread, together with SBS-F1 |

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

**Rotation delta (T5/T7), run 2.** Same craft in a 200 km orbit, SAS off, full input for 1 s on pitch, then yaw,
then roll (4 s apart), then SAS on in stability mode.

| Channel | KSP2 | OSE | Delta |
|---|---|---|---|
| Pitch rate after 1 s of full pitch | 0.301 rad/s | 0.479 rad/s | **+59%** |
| Yaw rate after 1 s of full yaw | 0.299 rad/s | about 0.49 rad/s | **+64%** |
| Time for SAS to stop the tumble | about 2.0 s, then a 0.05-0.07 rad/s wobble that dies out over about 10 s | under 0.5 s, then exactly 0 | **SAS stops instantly** |

## Parity checklist (trace-verified)

Status: **verified** = a KSP2 trace and an OSE trace of the same scenario agree within the stated tolerance;
**failing** = they differ beyond it (row links the fix); **not traced** = no KSP2 trace yet. Nothing is verified
until a delta table is attached here. Priority: P0 changes how every flight feels; P1 blocks a whole activity;
P2 completeness.

| # | System | Pri | KSP2 scenario to trace | Compared channels and tolerance | Status |
|---|---|---|---|---|---|
| T1 | Thrust, Isp vs pressure, fuel burn | P0 | Run 1 craft, full throttle from the pad | mass(t), thrust, burn time; burnout altitude/speed within 2% | **verified** (mass flow and burn time within 0.3%, peak speed -0.2%) |
| T2 | Drag and Mach model (surface area, drag cubes) | P0 | Run 1 coast and fall; a blunt pod falling from 30 km | speed vs altitude on descent, terminal speed at 5 km within 5% | **verified at high Mach** (apoapsis -0.3%, 8.5 km fall speed +2.0%); subsonic terminal speed still to trace |
| T3 | Mass and CoM shift as fuel drains | P0 | Run 1 with CoM logged | CoM along the stack vs fuel fraction within 2 cm | not traced |
| T4 | Crossfeed and fuel-flow order, staging, decouplers | P0 | Two-stage stack with a decoupler, radial tanks with fuel lines | per-tank amounts vs time, separation impulse | not traced |
| T5 | SAS hold, reaction wheels, torque | P0 | Pod with wheel: step pitch input 1 s, release, SAS on | attitude and angular rate vs time within 5% | **failing**: rate +59-64% per second of input; SAS stops the spin instantly instead of in about 2 s |
| T6 | Gimbal response | P0 | Engine gimbal step under SAS | gimbal angle and body rate vs time | not traced |
| T7 | Input response and precision mode | P0 | Keyboard pitch hold, CapsLock precision, trim | rate build-up and latency per tick | full-input step traced with T5 (failing with it); precision and trim not traced |
| T8 | Orbits, SOI changes, time warp (rails and physics) | P0 | 100 km orbit, warp x100, Mun encounter | apsides after warp, SOI exit time within 1 s | not traced |
| T9 | Maneuver node execution | P1 | Prograde 100 m/s node, burn by node timer | resulting apsides within 1% | not traced |
| T10 | Control surfaces and lift | P1 | Stock jet takeoff roll and pitch-up | lift vs AoA and speed, rotation speed | not traced |
| T11 | Heating and reentry | P1 | Pod reentering from 100 km x 30 km orbit | skin temperature vs time, ablator use | not traced |
| T12 | Parachutes | P1 | Mk16 deploy at 2.5 km over water | deploy timing, descent speed at sea level within 3% | not traced |
| T13 | Joints, flex, wobble, struts | P1 | Tall 6-tank stack under full thrust and roll | top-part deflection vs time, break threshold | not traced |
| T14 | Collisions, crash tolerance, part breakage | P1 | Drop pod on pad at 5, 10, 15 m/s | survive/destroy per part matches | not traced |
| T15 | Landing legs and ground contact | P1 | Lander touching down at 3 m/s | suspension travel, bounce, settle time | not traced |
| T16 | Wheels and driving | P1 | Stock rover: full throttle 20 s, brake, steer | speed vs time, stopping distance, turn radius | not traced |
| T17 | RCS translation and rotation | P1 | Probe with RCS: 2 s translate, 2 s rotate | delta-v, monoprop use, rate | not traced |
| T18 | Docking | P1 | Two ports approaching at 0.3 m/s | capture distance, merged mass and CoM | not traced |
| T19 | EVA exit, ladders | P1 | Kerbal exits Tin Can on the pad, climbs ladder | exit position, climb speed | not traced |
| T20 | EVA walk, run, jump | P1 | Walk 10 s, run 10 s, jump | speeds (0.8 / 2.2 m/s documented), jump height | not traced |
| T21 | EVA jetpack | P1 | 2 s up, 2 s forward in orbit | delta-v and fuel per second | not traced |
| T22 | Electric charge and solar | P2 | Probe with panels in sun, wheel on | EC rate in/out | not traced |
| T23 | Comms (CommNet) | P2 | Probe behind Kerbin from KSC | link loss timing | not traced |
| T24 | Science collection and transmit | P2 | Crew report on the pad, transmit | science value, transmit time and EC | not traced |
| T25 | Flags | P2 | Plant a flag next to the pad | placement, collision | not traced |
| T26 | Camera modes and input bindings | P2 | Cycle V modes, default binding table | mode list, key map identical | not traced |

## Next steps

1. Build the OSE side of the trace runner: an `ose-bot` example that imports the same workspace JSON, flies the same
   scenario steps and writes the same CSV columns; a comparer that prints the delta table above.
2. Add scenarios T1-T8 to the KSP2 pilot first (P0), then P1, closing failing rows with fix PRs that attach
   before/after deltas.
3. Earlier items: SBS-C1, SBS-C2, SBS-F1/F2 fix PRs are in progress; SBS-V1..V4 are with planets/campus; SBS-V5 is
   for Josh.
