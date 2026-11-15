# KSP2 interaction reference and OSE comparison

Recorded 2026-10-29 from the local KSP2 0.2.2.0.32913 reference code and
the installed game's `Global` configuration. The committed fixture contains
130 numerical cases with source/configuration SHA-256 hashes.

**This is a source/configuration recording, not a live keyboard/mouse capture.**
No timed Shift/Ctrl holds, camera drags or maneuver drags were recorded during
this pass. The fixture explicitly sets `live_input_capture: false`. Existing
flight-control CSVs do not contain the camera, pointer, DPI and event timing
needed to establish these interaction rates empirically.

## Recorded behavior

| Interaction | KSP2 reference | Current OSE |
|---|---|---|
| Throttle Shift/Ctrl | `clamp(throttle + axis * unscaledDeltaTime, 0, 1)`: 100 percentage points per real second; Z sets 100%, X sets 0% | Same equation and presets; OSE additionally limits an input tick to 100 ms |
| Flight orbit, ground/orbital basic rig | Heading change in degrees = raw mouse delta × 4.5 × real frame seconds; fine modifier × 0.3 | 0.005 radians (0.286479°) per CSS pixel, independent of frame duration |
| Flight zoom, basic rig | Distance × `exp(-zoomAxis)`; fine movement uses `exp(-0.3 * zoomAxis)` | Distance × 0.8 in / 1.25 out per wheel event; Shift cubes that factor |
| Map orbit, mouse | Degrees per mouse pixel = `2 * max(0.001, sensitivity * (85 - abs(pitch))/85)`; sensitivity 0.05 normally, 0.005 with Alt | 0.005 radians per CSS pixel; no equivalent pitch-dependent sensitivity |
| Map zoom | Exponential mode: inverse map scale × `exp(-0.3)` in / `exp(0.3)` out, based on the sign of the zoom input | × 0.8 in / 1.25 out per wheel event |
| VAB orbit | 0.004 radians per raw mouse pixel; precise modifier 0.001 radians. The camera converts these to degrees before applying them | 0.005 radians per CSS pixel |
| VAB orbital zoom | Pivot distance decreases by `zoomAxis * speedMultiplier * 3`; precise modifier substitutes 0.3; Shift speed multiplier 2 | Multiplicative zoom: × 0.8 in / 1.25 out; Shift cubes that factor |
| Held-part rotation keys, snap on **and** off | 90° per press; Shift 5°. Snap state does not select the keyboard step | Snap on: 90°; snap off: 15°; Shift: 5° |
| Rotation gizmo | Snap on: 15°, or 5° with speed modifier; snap off: continuous | Not covered by OSE's held-part keyboard rotation; no equivalent gizmo comparison in this fixture |
| Radial symmetry | Counts 1/2/3/4/6/8; spacing 360°/180°/120°/90°/60°/45° | Matches; actual surface placement uses the same tested angle function |
| Mirror symmetry | Reflection, not a radial 180° rotation | Covered separately by the existing `a_plane_pairs_side_parts_as_mirror_images` test |
| Ordinary warp ladder | 1, 2, 4, 10, 50, 100, 1,000, 10,000, 100,000, 1,000,000; optional interstellar rate 10,000,000 | 1, 2, 3, 4, 5, 10, 50, 100, 1,000, 10,000, 100,000 |
| Warp-to selection | Selects a rate leaving 1–10 real seconds of travel by default, subject to availability/caps | Selects a ladder step leaving about four real seconds with a five-second margin |
| Automatic warp-to drop | At a settled rate `w > 1`, requests 1× when the target is `(w + 1)/2` simulation seconds away. Rate changes use nominal one-second Unity-time interpolation; final-step handling prevents overshoot | Repeated ladder reductions; desired rate reaches 1× below 13 seconds remaining with the current ladder; arrival clears within two seconds |
| Maneuver handle | Signed `(projected displacement / (strength * DPI))²` m/s **per Update frame**, added repeatedly while held away from its initial position | Absolute displacement from drag start × 1 m/s per pixel, or × 0.1 with Shift |

The camera numbers are pre-collision, pre-blend input responses, not rendered
motion rates. Unity system mouse pixels and browser CSS pixels are not assumed
to be physically calibrated. Flight orbit is frame-dependent in this reference:
at 60 Hz, the normal coefficient is 0.075° per raw pixel; at 30 Hz it is 0.15°.
The comparison tests angle magnitude, not handedness or camera orientation.

Zoom inputs in these cases are **Unity action values**, not OS wheel notches.
For example, a flight zoom input of +0.1 scales distance by 0.904837; VAB
zoom +0.1 subtracts 0.3 m normally or 0.6 m with Shift. UI blocking, camera
volume limits, zoom bounds, free/blueprint/chase camera modes and smoothing
require additional runtime recordings. Map's fine zoom path also multiplies
the resulting inverse scale by 0.1; that unusual behavior is recorded in the
source notes but is not covered by the normal map zoom cases.

Maneuver drag therefore has no universal constant Δv per pixel. With **assumed
serialized strength 1**, DPI 96 and projected displacement 48 px, it adds
0.25 m/s each Update. Holding that displacement for 60 Updates adds 15 m/s.
At DPI 192 the same displacement adds 0.0625 m/s per Update. The local
slope is `2 * abs(displacement) / (strength² * DPI²)` m/s per extra pixel
per Update. Screen projection, holding duration, actual Unity `Screen.dpi`,
serialized strength and zero-DPI behavior must be captured before using these
as physical mouse measurements. KSP2's examined handle path has no Shift
precision factor.

Automatic warp has additional safety drops: physics warp caps at index 2
(4×) in atmosphere/terrain conditions; orbital transition handling caps at
index 4 (50×) with 50 simulation seconds of SOI lookahead or 10 for fuel
exhaustion, plus the deceleration distance. At 1,000× approaching an SOI,
the reference drop-to-50× threshold is `(1000 + 50)/2 + 50 = 575` simulation
seconds. Altitude requirements, at-rest exceptions, thrust constraints and
the final auto-warp step are separate from the unrestricted numerical cases.
The fixture records these policies; the strict probe currently compares the
warp ladder, unrestricted target selection and target drop trigger only.

## Automated checks

From the repository root:

```powershell
# Re-record against the installed Global configuration and local decompiled code.
# Requires .NET 9; extracted C# stays under the ignored .cache directory.
.venv/Scripts/python.exe tools/record_interaction_reference.py

# Run the real OSE input functions against every recorded case. Exit 1 = gap.
.venv/Scripts/python.exe tools/check_interaction_parity.py

# Also reject a source-only reference, even if all numerical cases someday match.
.venv/Scripts/python.exe tools/check_interaction_parity.py --require-live

# Test the gate's coverage, tolerance and rejection of missing/duplicate results.
.venv/Scripts/python.exe -B -m unittest discover -s tools -p test_interaction_parity.py

# Matching throttle/symmetry cases plus normal client regressions.
cargo test --offline -p ose-client
cargo check --offline -p ose-client --target wasm32-unknown-unknown
```

The strict gate runs `interaction_probe`, which calls the **same functions used
by OSE's browser handlers**. It does not read the fixture's expected values.
The small handler refactor preserves existing OSE behavior, including known
differences; this change adds measurements and tests, not gameplay tuning.
The gate refuses missing/extra/duplicate cases and missing reference categories.
It uses absolute tolerance 1e-6 and relative tolerance 2e-6 for C# float/Rust
double differences, exact booleans and matching array lengths. Its full report
is written to `.cache/interaction-parity-report.json`.

Initial result: **49/130 cases aligned, 81 differed**. Throttle and radial
symmetry pass completely. The full parity gate intentionally exits 1 for
the documented differences; it is not a passing end-to-end parity claim.
Comparator unit tests use clearly marked synthetic results to test the gate
itself and must not be confused with the strict OSE comparison.

The remaining empirical work is a synchronized KSP2/OSE recording with raw
key/pointer events, frame timing, wheel action values, DPI/display scaling,
camera mode/settings and before/after values for every requested interaction.
