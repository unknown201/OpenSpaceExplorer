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
| SBS-P1 | **Unverified.** Drag on a coasting/falling craft looks low | Not yet flown in KSP2. Trace to capture: same craft straight up at full throttle, record apoapsis and speed passing 4 km on the way down | Burnout at 7.6 km and 760 m/s, apoapsis 39.6 km at T+104 s (close to the no-drag ballistic value of about 39 km). Still doing about 790 m/s at 4 km on the way down. The server/sim thread checked the code: it follows KSP2's Module_Drag/Data_Drag with KSP2's exported curves, and the low supersonic drag comes from KSP's Mach-power model (sea-level terminal speed for this craft is about 470 m/s) | `reference/ose-shots/log.txt` (ascent trace), `10-ascent-25km.png` | needs a KSP2 trace (this thread), then server/sim |
| SBS-P2 | No KSP2 trace exists for any flight, so ascent, SAS, wheel and gimbal feel are unproven | `tools/ksp2-control-capture` records time, inputs, attitude, rates, mass | `tools/compare_control_traces.py` is ready but has nothing to compare | `tools/ksp2-control-capture/README.md` | this thread, **blocked on Josh** (BepInEx install approval) |

### Visual fidelity

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-V1 | KSC campus has no trees or shrubs | Dense tree lines along the rivers and around the pad ring | Bare, evenly lit green ground | `side-by-side/ksc.jpg` | planets/trees thread |
| SBS-V2 | KSC geography differs: no ocean behind the campus, mountains where KSP2 has the coast | The campus sits on a peninsula with open ocean behind the VAB | Inland lakes and a mountain range on the horizon | `side-by-side/ksc.jpg` | planets/trees + 3D campus threads |
| SBS-V3 | A bright yellow rectangle floats on the river while the KSC camera moves | Water shows sun glints, no hard-edged quad | Sharp-edged yellow streak on the river, visible in the moving-camera frame | `ose-shots/02b-ksc-camera-moving.png` (left of centre, on the river) | planets thread (water) |
| SBS-V4 | KSC ground is flat-lit with no ambient occlusion or contact shadows | Buildings and trees cast soft shadows, ground has colour variation | Uniform albedo, buildings float visually | `side-by-side/ksc.jpg` | 3D campus thread |
| SBS-V5 | Exhaust plume and launch smoke | Volumetric smoke, bright plume | Present and comparable at liftoff; particles not audited here | `ose-shots/07-liftoff.png` | **Report for Josh** (GPT agent owns particles/effects) |

### Camera, input and controls

| ID | Gap | KSP2 | OSE | Evidence | Owner |
|---|---|---|---|---|---|
| SBS-C1 | The flight camera starts far from the craft on the pad | Camera starts a few craft-lengths behind the vessel, which fills a good part of the screen | Camera frames the whole pad structure; a 3-part rocket is about 20 px tall at 1080p | `side-by-side/pad.jpg` | this thread (flight camera) |
| SBS-C2 | The in-flight map opens at system scale | M opens on the active vessel: Kerbin fills the view with the trajectory and AP/PE markers on it | Opens on the full Kerbin system (Mun and Minmus orbits); Kerbin and the trajectory are a few pixels in the centre | `side-by-side/map.jpg` | this thread (map camera), confirm with UI thread |
| SBS-C3 | VAB camera starts far from a small craft | Craft centred and filling the hall's lower third | Craft small at the right edge of the hall | `side-by-side/vab.jpg` | UI/VAB thread |
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

## Next steps

1. Josh: approve the BepInEx install so the trace recorder can run (SBS-P2, SBS-C4), and either allow computer-use on
   KSP2 or take F12 shots of the same five scenes at noon on a fresh save.
2. Fix agents: SBS-C1, SBS-C2, SBS-F1/F2 (this thread), each with before/after captures from the same script.
3. Hand SBS-P1 to server/sim, SBS-V1..V4 to planets/campus, SBS-C3 to UI/VAB, SBS-V5 to Josh.
