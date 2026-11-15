# Client load and intermittent stalls

## Conditions and observations

The report is high CPU/GPU use around 100 FPS, followed by jumpy craft controls and occasional
multi-second stalls after playing for a while. An FPS average alone cannot distinguish a blocked
browser from a browser continuing to render while authoritative vessel updates stop arriving.
This is an engine scheduling investigation; no KSP2 behavior or visual parity is claimed.

Local measurements use release WASM, Chromium/WebGPU, real `assets/web` assets, a 1920 × 1080 viewport,
and the Tin Can / FL-T200 / LV-T45 probe craft. The before and after samples warm up for ten seconds
and measure three seconds each on the pad and during ascent. The after sample explicitly selects
Unlimited so the frame cap cannot account for a reduction in CPU time per rendered frame.

| Condition | Before | Changed behavior |
| --- | --- | --- |
| Mesh uniforms | One GPU queue write for each primitive | One aligned buffer upload per frame, with a binding for each primitive |
| Static models | Allocate and evaluate their default node pose every frame | Reuse the model's default world matrices; animations and control rotations still evaluate |
| Idle engine effects | Full HDR scene copy plus an empty effects pass | Skip the copy and effects pass when no effects are drawn |
| Text HUD | Rebuild on every animation frame | Refresh at most 20 times per second; retain unchanged text nodes |
| Hidden map under 3D scenes | Clear a full-screen Canvas 2D surface every frame | Only clear it while drawing the 2D map |
| Render cadence | Follow browser refresh rate | Default 60 FPS, with 30, 120 and Unlimited choices saved per browser |
| Flight input | Polled as part of the full render update | Poll every animation callback, including callbacks skipped by the render cap |
| Hidden tab | Depends on browser throttling | Skip full rendering while hidden |
| Stall readout | FPS average | Also retain worst rendered-frame and vessel-update intervals for a ten-second window |
| Server save | Hold simulation/registry/campaign/craft locks through JSON serialization and disk writes | Capture an owned snapshot under the locks, then release them before serialization and I/O; saves remain serialized |
| Map geometry | Recompute every body's orbit and every vessel's trajectory points each frame, even when GPU buffers are cached | Generate vertices only on a cache miss; camera-relative origins and camera projections still update each frame |
| Trajectory invalidation | Any vessel prediction changes the global version of all trajectory strips | Version changed predictions per vessel; identical predictions retain their version |
| Removed vessels | Retain their trajectory records | Prune prediction records and versions when vessels disappear |
| Closest approach | Search 721 times for both vessel positions every rendered frame | Refresh at most twice a second, or immediately when the target, prediction, body or warp changes or time rewinds |

## Short local comparison

| Measurement | Pad before | Pad after | Ascent before | Ascent after |
| --- | ---: | ---: | ---: | ---: |
| Object uniform writes / frame | 1082 | 1 | 1082 | 1 |
| Texture copies / frame | 1 | 0 | 1 | 1 |
| DOM mutations / frame | 45.1 | 25.0 | 46.8 | 27.7 |
| Median CPU callback, ms | 7.3 | 7.0 | 8.4 | 7.0 |
| CPU callback p95, ms | 8.2 | 7.8 | 10.5 | 8.6 |
| Main-thread task time / rendered frame, ms | 10.0 | 8.6 | 11.5 | 8.8 |
| Median timed GPU render passes, ms | 0.962 | 0.947 | 0.989 | 0.970 |

Both samples rendered about 32 FPS because of local browser pacing. These samples establish less
work per frame, not a measured improvement from 100 to 60 FPS or an electrical power saving. The
number of mesh draws stayed at 1082; active heat refraction and exhaust still render during ascent.
The GPU pass totals exclude the texture copy, so they do not measure the removed copy's cost.

Other work was landing in the shared checkout during this investigation. The baseline used protocol
46; the after client required rebuilding the private server for protocol 47. The draw counts and
conditions above match, but these are not isolated revision-to-revision benchmark results.

Local artifacts (ignored, not supplied by a clean checkout):

- [Baseline metrics](../../.cache/client-perf/before.json) and [optimized Unlimited metrics](../../.cache/client-perf/after-unlimited.json).
- [Baseline pad capture](../../.cache/client-perf/before.json.0.png) and [optimized pad capture](../../.cache/client-perf/after-unlimited.json.0.png).
- Matching ascent captures and CPU profiles are alongside those files.

Visual inspection shows the same campus, craft and terrain composition. Existing concurrent attitude
changes affect the navball marker, so these captures are not evidence of pixel equality.

## Orbital map comparison

The user narrowed the stalls to orbit/map view. A second comparison uses four copies of the probe
at 100 km around Kerbin, map view with a selected vessel target, and ten-second samples after warmup.
Both clients use the same release protocol-47 server and Unlimited rendering. The before build already
contains uniform batching and the frame-cap implementation; this comparison isolates the subsequent
map caching changes more closely than the earlier ascent comparison.

| Measurement | Map before | Map after |
| --- | ---: | ---: |
| Rendered FPS | 60.0 | 60.0 |
| Median CPU callback, ms | 3.1 | 2.3 |
| CPU callback p95, ms | 4.0 | 3.0 |
| Main-thread task time / frame, ms | 5.63 | 3.75 |
| Maximum frame interval, ms | 17.1 | 17.0 |
| Body mesh / line draws per frame | 17 / 21 | 17 / 21 |

Main-thread work per frame fell about 33%. No multi-second pause occurred in either short sample.
The nearest-approach marker search now has up to 0.5 s refresh latency during unchanged predictions;
camera projection and its displayed countdown remain per-frame. Target/prediction/body/warp changes
invalidate immediately. This approximation does not alter authoritative simulation or controls.

Local artifacts: [map before](../../.cache/client-perf/map-before.json),
[map after](../../.cache/client-perf/map-after.json),
[before capture](../../.cache/client-perf/map-before.json.0.png), and
[after capture](../../.cache/client-perf/map-after.json.0.png). The after capture was visually checked
for body/orbit geometry, vessel and target labels, apsides, and closest-approach markers.

The [three-minute orbital-map probe](../../.cache/client-perf/map-sustained.json) uses four vessels,
a target, a requested 120 FPS cap with diagnostic unthrottled-browser flags, a 35 ms W-key tap every
ten seconds, and an isolated release server writing a 41 KB save every five seconds. Actual browser
rendering averaged 50.4 FPS; the requested 120 FPS was not achieved. Across 9132 rendered frames:

- Maximum frame interval: 37.1 ms; maximum callback: 4.9 ms.
- No frame gaps over 100 ms and no browser long tasks over 50 ms.
- Maximum incoming-message gap: 118.5 ms.
- The 18 keyboard press/delay/release operations completed in 40.2–50.9 ms each, including their
  intentional 35 ms hold. These timings measure browser dispatch round trips, not server acknowledgments.
- Sampled JS heap ranged from 173.3 to 176.4 MB. This does not include all native/GPU allocations.

No multi-second stall was reproduced in this scenario. Stability at the user's original 100 FPS,
with their full campaign, network conditions, vessel count and session duration, remains unverified.

## Repeat and diagnose

Build both client and server from matching sources, then start a private test instance:

```powershell
cargo build -p ose-server --release
node e2e/scripts/build-client.mjs
target/release/ose-server.exe --listen 127.0.0.1:7801 --static-dir target/web --assets-dir assets/web --db :memory: --save none
```

In another terminal, from `e2e`:

```powershell
$env:OSE_BASE_URL = 'http://127.0.0.1:7801'
$env:OSE_FLIGHT = '1'
$env:OSE_SKIPS = ''
$env:OSE_FRAME_LIMIT = '0'
node scripts/profile-ascent.mjs ../target/client-profile.json

# Longer flight: record long tasks, largest frame gap, message gap, JS heap and particle count.
$env:OSE_SAMPLE_MS = '120000'
$env:OSE_ALTS = '1'
$env:OSE_GPU_TIMING = '0' # avoid allocating GPU timestamp readbacks during the long sample
node scripts/profile-ascent.mjs ../target/client-sustained.json

# Reproduce orbital map load with a target and four vessels.
$env:OSE_ORBIT = '1'
$env:OSE_MAP = '1'
$env:OSE_VESSELS = '4'
$env:OSE_FRAME_LIMIT = '120'
$env:OSE_UNTHROTTLED = '1' # diagnostic browser flags bypass the display's frame pacing
$env:OSE_SAMPLE_MS = '180000'
$env:OSE_TAP_CONTROLS = '1'
node scripts/profile-ascent.mjs ../target/map-sustained.json
```

The profiler creates a new private sandbox campaign. `OSE_CHROMIUM_EXECUTABLE` selects an installed
Chromium executable; `PLAYWRIGHT_BROWSERS_PATH` can select an existing Playwright browser cache.
To exercise frequent autosaves during the map probe, start the private server with
`--save .cache/client-perf/orbit-soak-save.json --save-interval-s 5` in place of `--save none`.

Graphics > Show FPS and stalls shows `frame max` and `state max` intervals in milliseconds.
A large frame interval points to rendering/main-thread scheduling; normal frame intervals with large
state intervals point to delayed authoritative updates. State arrival gaps also include network delay;
they are not a direct measurement of server simulation time. The save-lock change removes a concrete
way for disk latency to block physics, but does not establish that autosave caused the original report.

## Validation

- Native pacing tests exercise 30/60/120/Unlimited against 60/100/120/144/165/240 Hz callback streams,
  long gaps, live setting changes, and retaining stall evidence after FPS recovers.
- Native cache tests cover unchanged versus changed predictions, independent vessel invalidation,
  removed/reused vessel ids, and closest-approach invalidation after target/prediction/warp/time changes.
- Browser regression checks cover the saved cap, actual WebGPU submissions at 30 FPS, one object
  upload per rendered menu frame, no idle refraction copies, and flight press/release messages.
- The [two-minute local flight probe](../../.cache/client-perf/sustained-flight.json) recorded 3844
  rendered frames, a 32.2 ms maximum frame gap, a 15.7 ms maximum render callback, no frame gaps over
  100 ms, no browser long tasks over 50 ms, and a maximum incoming-message gap of 115.1 ms. Sampled
  JS heap ranged from 233.8 to 240.1 MB. This does not measure total process/GPU memory, and the
  profiling arrays themselves grow during capture. No multi-second stall was reproduced. The local
  browser paced rendering near 32 FPS, so this does not establish stability under a 100 FPS workload.
- The private flight server used `--save none`; the sustained sample does not exercise autosave or
  validate autosave as the user's cause. A matching server rebuild is required for the save-lock fix.
- `cargo test -p ose-client --lib`: 121 passed. Release WASM and release server builds passed.
- `cargo test -p ose-server --test campaign_access`: passed, including save/restart persistence.
- `client_performance.spec.ts --project=chromium`: both WebGPU browser checks passed. TypeScript
  checking and the changed-file whitespace checks passed. Earlier test attempts exposed a stale
  protocol-46 server and an outdated probe discriminant; both were corrected before these passes.
