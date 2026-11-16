# Waypoint timing and warp controls

Recorded November 15, 2026. This fixes maneuver timing edits, planning modifiers,
long-horizon path rendering, coasting warp-to, and cancellation in the browser client.
No protocol or save-format change is required.

The initial investigation used checkout `f745021`. The release ports these fixes
onto current main (`fc3d4f1` initially), preserving protocol 72, chained nodes,
the logarithmic mouse slider, finite-burn execution, and rendezvous controls.
The initial before/after measurements below refer to the investigation checkout;
the release verification is recorded separately at the end.

## Conditions and reference evidence

The reproduction uses a fresh isolated Sandbox campaign, Stock12_StartRocket
placed in a 300 km Kerbin orbit through the launchpad's Orbit button, and Chromium's
WebGPU renderer with the real converted assets. Default Up attitude/SAS is retained.
The script first fine-drags a 40 m/s prograde maneuver, then changes its time,
sets full throttle deliberately, clicks Warp to, and attempts to slow/stop warp.
Full throttle is an explicit reproduction condition for the inherited-throttle check.
The fine-drag sequence also reproduced an unintended throttle increase (2% on the final
before capture): Shift was simultaneously a precision modifier and a held throttle-up
key. Holding it longer can reach full throttle. The old Warp to handler carried that
throttle through warp and did not clear held throttle keys.

For the reported crossing-line pattern, the test widens the timing slider to schedule
a maneuver 21 Kerbin days, 5 hours, and 475 seconds ahead. Read-only WebSocket telemetry
confirms an actual maneuver patch spanning about 165 revolutions. Captures before and
during warp reproduce the crossing chords. The same fixed client draws one clean orbit
while keeping that future maneuver's time intact.

The local KSP2 reference is 0.2.2.0.32913, as recorded in
[the interaction comparison](interaction-parity.md). Source evidence comes from:

- `KSP.Sim.impl/TimeWarp.cs`: `IncreaseTimeWarp`, `DecreaseTimeWarp`,
  `StopTimeWarp`, `CancelAutoWarp`, `WarpTo`, and `AutoWarpTo`. Manual rate changes
  cancel automatic warp; stop cancels it and selects real time. WarpTo does not issue
  an engine-throttle command.
- `KSP.Map/MapManeuverGizmo.cs`: `BeginMovement`, `LateUpdate`, and
  `ManipulateManeuver` keep movement separate from delta-v manipulation and publish
  movement while the gizmo is held.
- `KSP.Map/OrbitRenderer.cs` configures an adaptive generator with per-period seed
  and point limits. `OrbitRenderSegment.UpdateDrawData` supplies the patch parameter
  interval. OSE's fixed 240-segment strip across arbitrary numbers of periods has
  no equivalent adaptive density. The fix draws a single revolution of a closed
  conic; it does not claim to reproduce KSP2's adaptive renderer.

KSP2 was not run for this change. These are source observations, not a live timing
or visual-parity claim. The release preserves main's existing finite-duration burn
executor and multi-node planner. The older investigation checkout used impulsive
scheduled burns; its simulation check below describes that checkout.

| Player action | KSP2 source reference | OSE before (reproduced or inspected) | OSE fixed |
|---|---|---|---|
| Adjust time after fine-dragging a burn | Time movement and delta-v manipulation are separate | Fine-dragged 40.7 m/s became 41.0 m/s when the time slider reread every input | The time edit preserves 40.7 m/s exactly |
| Use Shift/Ctrl while planning | Exact slider-arrow policy not compared; requested OSE behavior | Shift also throttled up the ship; native sliders did not implement modifier increments | Arrows change by 1, Shift by 10, Ctrl by 0.1; Shift+Ctrl by 1; planning modifiers never throttle the ship |
| Plan in the next minute | Node time is supplied by the movement/controller path; no 60 s minimum established by this review | A requested 25 s was clamped to 60 s by HTML | The time slider accepts 25 s, down to zero offset |
| Drag a node along its orbit | Movement publishes continuously; exact projection parity not established | Nearest of 360 points produces discrete jumps; closed-orbit times can jump at the seam | Segment interpolation gives continuous times; closed-orbit drags retain the nearest revolution |
| Leave a maneuver many days ahead, then warp | Adaptive generator uses per-period density; no live parity claim | About 165 revolutions are squeezed into 240 segments, producing crossing chords that change as time advances | Both map renderers plot at most one revolution per closed patch, retaining partial event endpoints and the actual future node time |
| Click Warp to with throttle already open | WarpTo changes time rate, not throttle | Engine stays at 100%; powered integration changes the orbit and limits effective warp | Held throttle keys clear; a zero-throttle command precedes warp requests; coasting does not consume fuel or change apsides |
| Click warp down during automatic warp | DecreaseTimeWarp cancels automatic warp | Requested 100× → 50× → 100× without further input | Requested 50× remains 50× |
| Stop automatic warp | StopTimeWarp cancels it and selects real time | No visible stop button; slash was swallowed by focused sliders | Top Stop warp button, node button toggled to Stop warp, and slash from a focused slider all cancel and select 1× |
| Change/delete an active warp target | CancelAutoWarp clears the active target; edit behavior not compared live | Old target remains active | Starting a node drag, editing, replacing, or deleting a node stops automatic warp |
| Arrive before the maneuver | AutoWarpTo slows and stops at its target | Existing ladder reductions | Preserved, with an unconditional 1× request on arrival so a previously capped request cannot accelerate again |

## Implementation

`ose-client/src/maneuver.rs` projects the pointer onto adjacent orbit segments and
interpolates UT. For a full closed orbit it lifts time to the revolution nearest the
current node. Partial trajectories do not wrap. Fresh placement selects the upcoming
pass. Picking now includes the first sample so the orbit has no missing seam segment.

`lib.rs::node_inputs_changed` edits only the originating input. Browser range
sanitization of another component can no longer change the node's time or delta-v.
The release retains main's logarithmic time slider for mouse input. Its keyboard
arrows adjust the actual node offset by 1/10/0.1 seconds, then convert back to the
logarithmic thumb position without quantizing unrelated node components.

Slider arrow handlers apply the modifier increment explicitly and stop the browser's
default arrow action. Delta-v sliders support 0.1 increments; the logarithmic time
slider uses continuous positions. During map planning,
Shift/Ctrl are reserved for the editor, including fine gizmo drags, instead of acting
as continuous throttle input. Z/X and explicit Full/Cut remain available.

`scene::trajectory_points` is shared by WebGPU and Canvas map drawing. A finite
closed-orbit patch is sampled over at most one orbital period. Shorter event intervals
are preserved. Open trajectories retain their finite endpoint or the existing six-hour
fallback horizon, and sampling still stops outside the sphere of influence. This fixes
temporal aliasing rather than hiding retained GPU buffers; the renderer already prunes
unused trajectory buffer keys each frame.

`start_warp_to` clears held controls and cuts the relevant vessel's throttle before
the first warp request. An assigned analog throttle is ignored while automatic warp
is engaged. Explicit keyboard/button throttle input cancels automatic warp first.
`stop_warp` clears the controller and requests 1×. The visible warp buttons cancel
the controller before changing rate, matching the existing period/comma shortcuts.
The slash shortcut also works when a range input has focus.

## Reproduction and verification

```powershell
cargo test --offline -p ose-client --lib
cargo check --offline -p ose-client --target wasm32-unknown-unknown
cargo test --offline -p ose-sim scheduled_burn_raises_apoapsis_and_lands_on_the_tick
cargo build --offline -p ose-server

# Build the current client to target/waypoint-after-web, then run the isolated test:
$env:OSE_WAYPOINT_PORT = '7808'
node e2e/scripts/check-waypoint-planning.mjs

# To capture an independently built original client:
$env:OSE_BEFORE = '1'
$env:OSE_STATIC_DIR = 'D:/Projects/OpenSpaceExplorer/target/waypoint-before-web'
node e2e/scripts/check-waypoint-planning.mjs
```

The browser script creates its own campaign with an in-memory account database and
no save file, and stops its server/browser on exit. It verifies actual fine dragging,
physical time-slider dragging, 25 s timing, inherited-throttle cutoff, manual rate
override, both stop buttons, slash with slider focus, explicit thrust cancellation,
target edits/deletion, coasting fuel/apsides, and automatic arrival before the burn.
It also exercises modifier arrows on all four sliders and captures a long-horizon
patch before/during warp, verifying the patch's multi-revolution span on the wire.

Results:

- All 126 client unit tests passed, including continuous orbit picking, seam crossing,
  fresh placement, partial-patch boundaries, degenerate-sample cases, and smooth
  single-revolution rendering as the prediction start advances toward a distant node.
- Native server build and WASM check/release build passed.
- The existing simulation regression passed: a high-warp step lands on a scheduled
  burn exactly and follows the predicted post-burn apoapsis.
- The real WebGPU browser regression passed without page errors. Fine-dragged
  40.7 m/s stays 40.7 m/s after timing edits; 25 s stays 25 s; warp-to reports 0%
  throttle; manual 50× stays 50×; stopped warps stay at 1×. A second coast keeps
  the same fuel and apsides. Automatic arrival leaves 16 seconds before the node,
  at 1× with zero throttle. All four sliders pass normal/coarse/fine/combined arrow
  checks without changing throttle.
- Strict WASM Clippy is blocked by the existing `clippy::never_loop` in `analog.rs`.
  With only that existing lint allowed, `-D warnings` passes. Workspace formatting
  check also reports unrelated existing formatting; the edited Rust files are formatted.
- Trunk's default release pipeline attempted to download an unavailable wasm-opt
  version. Verification used a temporary copy of index.html with `data-wasm-opt="0"`;
  the Rust release optimization and real WebGPU renderer were still used.

Local artifacts (ignored; not available in a clean checkout):

- [Before screenshot](../../.cache/waypoint-planning/before.png)
- [After screenshot](../../.cache/waypoint-planning/after.png)
- [Original long-horizon path](../../.cache/waypoint-planning/before-long-horizon.png)
- [Fixed long-horizon path](../../.cache/waypoint-planning/after-long-horizon.png)
- [Original path during warp](../../.cache/waypoint-planning/before-long-horizon-warp.png)
- [Fixed path during warp](../../.cache/waypoint-planning/after-long-horizon-warp.png)
- [Side-by-side comparison](../../.cache/waypoint-planning/comparison.html)
- [Before measurements](../../.cache/waypoint-planning/before.json)
- [After measurements](../../.cache/waypoint-planning/after.json)

The before/after screenshots share craft, starting orbit, viewport, camera, and
input sequence. They are captured after equal real time in warp. Their UT and
vessel position differ because the original is capped by thrust and the fixed
client coasts at the requested rate. They illustrate the control/state difference,
not pixel parity with KSP2.

## Release integration

The current-main port also separates pick samples at patch boundaries, so a
long coast followed by a different conic cannot introduce an interpolated straight
line across the unsampled interval. Timing drags use the orbit of the edited patch,
including nodes after earlier maneuvers or sphere-of-influence changes.

The warp lead-time estimate now requires measured nonzero thrust. Full throttle
with an unlit engine otherwise implied an enormous burn duration and incorrectly
reported that a node ten minutes ahead was already too close to warp to.

The release browser script supports the current guest-enabled isolated test server
and logarithmic slider. It passed with zero page errors: both planning drag and
all four modifier-arrow sequences kept throttle at zero, the real long-horizon
patch spanned 165.36 revolutions, warp-to cut inherited throttle, manual 10× remained
10×, coasting preserved 300 km circular apsides and all fuel, and automatic arrival
returned to 1× with zero throttle 17 seconds before the node.

`e2e/tests/waypoint_planning.spec.ts` adds the focused control regression to CI
in Chromium and Firefox. Both browser projects passed locally; the test covers
fractional burn preservation, physical seconds on the logarithmic time slider,
held modifiers, inherited throttle with an unlit engine, slash from slider focus,
the visible stop button, and cleanup of its private test campaign.
