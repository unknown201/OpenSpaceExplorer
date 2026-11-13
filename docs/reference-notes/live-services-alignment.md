# Live service alignment

2026-11-12. Reference: KSP2 0.2.2.0, Steam build 14669105, Unity 2022.3.5f1.
The managed-assembly hash and compact measured values are in
[`services-live.json`](../../crates/ose-sim/tests/data/services-live.json).
The controlled native capture recorded 6,053 frames across the primary service run,
normal launchpad EVA supplement and four-phase 100 km orbital solar supplement.
Capture code and reproduction instructions are in
[`tools/services-parity`](../../tools/services-parity/README.md).
Raw game-derived saves, assemblies, JSONL and screenshots remain local under
`.cache/services-parity`; they are not repository assets.

The earlier investigation used an older OSE checkout. Base commit
`520d0b8958ea596d50cafd175141dc78bb9fd636` already had authored packet rates,
solar incidence/shadowing, one portable flag and pickup. This change addresses the
remaining interruption, persistence, tracking and presentation defects on that base.

| Service | Running-game observation | OSE implementation and verification |
| --- | --- | --- |
| Transmission | Communotron 16: five units per two simulation seconds, 11 EC/s. One packet completed in 2.020 s and consumed 21.9999995 EC. | Production simulation completes in 100 fixed ticks (2 s), consumes 22 EC; a real WebSocket session independently checks the 22 EC delta. Samples stay aboard. Completion publication now runs every simulation tick, avoiding the extra one-second mission-evaluation delay. |
| Power/link interruption | Power failure callback within 0.020 simulation seconds. Native terminal states include `OutOfResources` and `LostConnection`; restoring the service does not restart the failed payload. | Interrupt only the current report and retain its progress and stored data. Later queued reports can start when service returns. Explicit transmit retries the failed report with native packet-ceiling semantics. Tests restore power/link and prove no implicit progress or EC draw. Retracting the active antenna interrupts rather than silently switching transmitters. |
| CommNet timing | Three losses: 2.90/3.04/2.76 s; returns: 1.88/2.00/1.72 s, with a 0.06 s sampling bound. | Preserve the existing 150-tick graph cadence; remove the transmit command's forced refresh. The phase-controlled regression checks loss and restoration after 150 ticks, not an invented fixed two-second reconnect timer. The server test waits for the streamed connection before transmitting. |
| Solar | Four orbital phases measured; the Kerbin-blocked phase has 289 samples with zero illumination and producer flow, battery drift below 1e-6 EC. Lit scale is approximately 0.968. Native pivot quaternions were captured, but craft-local sun vectors were not captured each frame. | Replay the measured orbital projections against production sunlight with a 0.0002 lit-scale tolerance and exact zero in eclipse. Existing incidence and part-obstruction tests remain. Tracking uses normalized quaternion Lerp rather than linear angle interpolation; independent quaternion regressions cover large errors. Rails recompute the local sun direction as the orbit changes. |
| Rendered tracking | The native deployed panel pivot changes independently of deployment animation. | Export the authored pivot node, stream authoritative angle/incidence/offered EC flow by part, and apply a per-draw local rotation after deployment. The WebSocket test checks late-join state. Actual WebGPU renders of shielded 1x6 and accordion panels at +30/-60 degrees match the independent instance renders: 6,924 and 8,847 visible pixels, zero mismatches. The production pose helper also has a coordinate-reflection regression. |
| Flags | Placement distance 0.450021589 m; second plant rejected; inventory consumed. Native pickup callback removes the flag and restores inventory. | Use the 0.45 m radial offset and retain it across ticks/save. Save consumed inventory, expose it in snapshots and hide Plant Flag when unavailable. JSON save and WebSocket tests cover repeat rejection, quickload, reconnect, pickup and replant. The placement tolerance is 0.0001 m. |
| Recovery/save | Native runtime establishes the interrupted payload remains failed; OSE persistence must not erase recoverable reports. | Keep queued reports in storage until delivery is drained. Persist active/stopped transmissions, pending completed deliveries, solar angle and flag inventory with legacy defaults. Recovery before publication cancels pending credit and returns the report once. Docking interrupts and retains both vessels' transmissions. |

## Validation

Focused commands (from the repository root):

```powershell
cargo test -p ose-sim --test live_services
cargo test -p ose-sim --test ksp2_solar_parity --test ksp2_solar_tracking_parity
cargo test -p ose-server services_save_
cargo test -p ose-server --test ws_session live_
cargo test -p ose-client --test animation
cd e2e
npx playwright test tests/ladder.spec.ts --project=chromium
```

For the GPU check, build the current WASM client, serve it with the current server
and exported game assets, then run:

```powershell
$env:OSE_BASE_URL = 'http://127.0.0.1:17865'
node e2e/scripts/check-solar-tracking.mjs
```

The script fails on browser errors, missing/unchanged poses, invisible instances or
shared-pose contamination. It writes screenshots and JSON results to the ignored
`.cache/services-parity/solar-browser` directory (`OSE_AUDIT_OUT` overrides it).

Required repository checks are formatting, workspace clippy/tests, the Python tool
suite and the wasm32 client check. The browser measurements used Chrome with a
real WebGPU adapter and the exported stock GLBs; the CI gameplay suite runs without
redistributing those assets. The browser EVA regression plants, checks that Plant
Flag disappears, picks up, checks that Plant Flag returns, then boards again.

## Evidence limits

This is bounded service alignment, not a blanket physics or visual parity claim.
The quaternion tracking regression derives its expected transient from Unity's
Lerp operation; native per-frame sun directions were not available for a matched
transient replay. Native lit-phase panel outputs were close to edge-on and are not
a useful full-rating calibration. Part obstruction still uses conservative boxes,
not Unity collider triangles, and deployment geometry is not continuously raycast.
All eight shipped tracking pivots use YAW with rotation limits disabled; other
tracking modes and custom limited pivots are not established here. Native flag
pickup was an interaction-callback measurement, not a mouse/reach test. OSE uses
its existing reach checks. Wall time from the slowed native audit is not a gameplay
performance comparison. CommNet graph reconnection and WebSocket transport
reconnection are separate verified cases.
