# Multiplayer streaming performance

The server publishes one shared snapshot batch at 10 Hz and one shared trajectory batch at 1 Hz.
World traversal, flight readouts, prediction and postcard encoding run once per publication, independent
of the number of connected players. Connections share immutable encoded buffers. Previously each
connection performed all this work independently while contending for the universe mutex.

For 16 players this removes 15 of the 16 repeated builds of each periodic update. Network bytes still
scale with players and vessels: this change does not implement deltas or interest management, and does
not change the wire protocol or the client update rates. Explicit burn requests still get immediate predictions.

Each connection retains only the latest pending snapshot and trajectory batch through watch channels.
A delayed reader skips superseded publications; it does not accumulate an application queue. Publisher
timers skip missed intervals. A write that stalls for five seconds ends the session so its warp request
and playing status can be released. TCP/browser buffers can still contain earlier data already sent.
Command replies and science events continue through the existing reliable session path.

The producer skips snapshot construction and trajectory prediction when nobody is subscribed. Vessel
registry maintenance still runs, including registering debris that belongs to a disconnected player.
New connections wait for a fresh publication after their handshake instead of replaying cached state.

## Validation and local load probe

Run the protocol tests and the optional timing probe:

```text
cargo test -p ose-server
cargo test -p ose-server --test ws_session multiplayer_latency_probe -- --ignored --nocapture
```

The regression suite checks shared world contents and matching time/body/vessel ticks across eight
active players plus one idle reader. The stream unit test checks that a reader skips 99 superseded
batches and shares the newest encoded storage with a fast reader. A separate test covers joining while
an older world update is still being built. Existing tests cover multiplayer
launches, warp arbitration, campaign changes, science and save/load.

Example probe run before integration with protocol v37, on Windows, 2026-10-25, protocol v28,
development build, loopback, one orbital probe per player,
30 ping samples per connection at approximately 100 ms intervals:

| Players | Ping p50 | Ping p95 | Max ping | Observed snapshots/client/s |
| --- | --- | --- | --- | --- |
| 1 | 0.33 ms | 0.52 ms | 15.87 ms | 9.53 |
| 4 | 0.37 ms | 0.68 ms | 0.77 ms | 9.55 |
| 16 | 0.14 ms | 1.32 ms | 2.53 ms | 9.54 |

These are post-change observations, not a before/after speedup claim. The short sampling window includes
a final sleep, so the observed delivery rate is slightly below the configured 10 Hz. The probe does not
measure WAN behavior, browser rendering, or large powered craft. Timing is reported rather than asserted
as a CI threshold.

To profile a real session, set `RUST_LOG=info,ose_server::streaming=debug` before launching the server.
The JSON log reports publication elapsed microseconds (including lock acquisition, construction and
encoding) and subscriber counts for snapshots and trajectories. Keep debug logging off during normal
play. Large fleet bandwidth, physics step time and browser rendering remain separate scaling costs.

## Bandwidth probe

The latency probe also reports the aggregate postcard payload rate across all connections. Set
`OSE_PERF_POWERED=1` to additionally launch a three-part starter craft on the pad for each player.
This includes flight readouts and physics, but no thrust, staging, complex craft or asset downloads.

Example local samples on 2026-10-25 before integration with protocol v37 (protocol v28;
Mbps, excluding WebSocket/TCP/IP/TLS overhead):

| Players | Orbital probes | With a starter craft per player |
| --- | --- | --- |
| 1 | 0.073 | 0.100 |
| 4 | 0.413 | 0.838 |
| 16 | 3.567 | 10.597 |

These short samples count streamed world messages consumed during the ping probe; they are estimates
of ongoing payload traffic, not link-level measurements or maximum capacity tests. With a shared
30 Mbps host uplink, the measured 16-player starter-craft scenario uses about 35% before overhead.
Asset downloads share that uplink if served from the same host. Larger fleets increase traffic to every
player, since there is currently no interest filtering or delta encoding.

## Control response and attitude presentation

Held steering, throttle and EVA inputs now send at 20 Hz (50 ms), with starts, reversals and pending
final values sent immediately on key changes. The sender keeps its deadline anchored across render-frame
jitter; it skips catch-up bursts after a pause. Discrete commands still send immediately. The 10 Hz world
stream and wire protocol are unchanged, so this adds only small control intents and acknowledgements.

The flight model and navball use a presentation attitude derived from authoritative nose/top axes.
Quaternion prediction follows the last observed angular velocity for at most 100 ms and 30 degrees;
prediction error blends away over 50 ms without a discontinuity at snapshot arrival. Angular rates use
server simulation time and warp rather than packet arrival spacing. Body/warp/surface transitions,
time rewinds, large pose jumps and stale connections reset prediction. Campaign changes and successful
loads/reverts clear the histories, and vanished/on-rails vessels lose their tracks. Game rules and
authoritative telemetry use the original snapshots.

Native tests cover input edges, frame-timing drift, quaternion wraparound, orthonormal axes, jitter,
stops, bounded prediction and discontinuity resets. The browser regression can be run with:

```text
cd e2e
npx playwright test multiplayer_controls.spec.ts
```

It launches a small rocket, adds 15 ms to each direction of the WebSocket, holds steering for two
seconds, and reports control payload and requestAnimationFrame cadence. This models 30 ms of additional
round-trip delay, not a bandwidth cap or loss. Before integration with protocol v37, Chromium and Firefox
both delivered 41 control messages
(including the final value), totaling 1,107 payload bytes, in a light scene running at about 60 FPS.
With the full local `assets/web` directory, one short Chromium launch sample measured about 30.5 FPS
and 31 control messages while models/terrain/effects were active. Headless Firefox used Canvas fallback.
These local samples are not hardware-independent FPS promises or a rendering speedup measurement;
the browser's frame rate also limits how often held controls can be sampled.

After integration with protocol v37, the same delayed-network test passed in both browsers with full
local assets. Chromium/WebGPU measured 22.1 FPS and 24 control messages (648 payload bytes); headless
Firefox/Canvas measured 60 FPS and 41 messages (1,107 bytes), over the two-second steering sample.
These render paths have different visual costs and are not comparable GPU benchmarks. The result
confirms that rendering remains a bottleneck in the full scene despite the networking improvements.

## Smoothness under latency and jitter

Protocol 54 changes how the client draws what the 10 Hz stream tells it, not what it is told or how often.
Three sources of frame-to-frame jank were found with the counters below, all independent of the browser's
own frame rate:

- The clock. Every `TimeState` reset the client's time estimate to its arrival, so network and scheduler
  jitter on each message moved the displayed universal time back or forth by the jitter (tens of
  milliseconds on a WAN), and every vessel jumped by its speed times that step. `TimeSync` now filters the
  offset between server time and the page clock and slews the displayed time toward it at up to 3% of the
  warp rate, so the time shown is continuous; warp changes, rewinds and disagreements over 200 ms still snap.
- Extrapolation under power. A powered vessel was placed from its osculating orbit elements, which ignore
  thrust, drag and lift (and are degenerate just off the pad), so each new state disagreed with the
  prediction. `VesselState` now carries the vessel's non-gravitational acceleration from its last physics
  step, and the client carries position and velocity forward under gravity plus that acceleration.
- Arrival discontinuities. What remained of the prediction error showed as a step at each arrival. The scene
  now records where each powered vessel was drawn when its new state arrives and fades that difference out
  over one snapshot interval (scaled by warp), like the attitude presentation already did. Errors over 25 m
  are teleports (loads, decoupling, docking) and show at once.

Only presentation changed: game rules, instruments and the map still use the authoritative states and
orbits, and nothing of the flight model runs in the browser (ADR-0002).

### Counters and benchmark

The client keeps smoothness counters (`crates/ose-client/src/netstats.rs`) and publishes a summary of the
last second as JSON in `<body data-ose-net>`: stream rate and bytes, arrival spacing, how far the clock
moved at each `TimeState` and how much slew it has pending, frame spacing with stalls over 50 ms, time spent
applying each stream message, and the motion residual of the flown vessel: each frame, how far it is from
where the previous frame's position and velocity put it. Smooth motion has a residual near zero whatever
the latency; a residual in metres is a visible jump.

The counters also found the server itself: its simulation loop took one step per timer wake, and on
Windows tokio's timer wakes every 16 ms rather than 20, so universal time advanced at 0.65 of real time
(`utRate` in the summary; the Linux deployment was not affected). The loop now owes steps to the wall
clock and takes the ones a late wake missed, up to five at once; a longer backlog is dropped so an
overloaded server runs slow rather than spiralling. Snapshot ticks are still sampled by the 10 Hz
publisher's own timer, so their spacing jitters by a tick or two; the client clock absorbs that.

`e2e/tests/netcode_smoothness.spec.ts` launches a starter rocket with each WebSocket direction delayed by
a base latency plus random order-preserving jitter (`OSE_NET_DELAY_MS`, `OSE_NET_JITTER_MS`), flies it for
`OSE_NET_SECONDS` while steering for a third of the time, and reports those counters. `OSE_NET_MODE=legacy`
opens the page with `?netsmooth=0`, which makes the same build draw as before protocol 58, so one build
measures both. Nothing is asserted about the numbers.

### Before and after

Release server and release client on Windows, 2026-10-30, Chromium/WebGPU headless (which paces at about
32 FPS), no game assets, twelve seconds of a starter rocket's ascent per run, the first second dropped.
Legacy is the same build with `?netsmooth=0`. Residuals are metres per frame; jumps are frames whose
residual exceeded 0.5 m; clock steps are how far the displayed time moved at a `TimeState`, ms; pending
is how much slew the smoothed clock still had to make up, ms. Each run leaves its rocket on the server,
so stream rate grows with the run count rather than with the change.

| One-way delay + jitter | Mode | Residual p95 | Residual max | Jumps / 12 s | Clock step p95 / max | Pending p95 |
| --- | --- | --- | --- | --- | --- | --- |
| 0 ms + 0 ms | legacy | 0.50 m | 1.01 m | 28 | 35 / 45 ms | |
| 0 ms + 0 ms | smoothed | 0.007 m | 0.08 m | 0 | 0 / 0 ms | 7 ms |
| 60 ms + 0..50 ms | legacy | 0.70 m | 1.41 m | 60 | 44 / 79 ms | |
| 60 ms + 0..50 ms | smoothed | 0.009 m | 0.12 m | 0 | 0 / 0 ms | 9 ms |
| 120 ms + 0..80 ms | legacy | 0.69 m | 1.23 m | 63 | 64 / 116 ms | |
| 120 ms + 0..80 ms | smoothed | 0.011 m | 0.03 m | 0 | 0 / 0 ms | 16 ms |

Jumps even at zero added delay come from the snapshot publisher's tick jitter and the browser's own
message scheduling; the legacy clock passed both straight to the picture. Early in an ascent the rocket is
slow, so these metres understate what the same clock steps do at orbital speed (a 60 ms step at 2,300 m/s
is 140 m, though the camera follows the vessel, so what the player sees is the ground and other vessels
jumping).

With the full `assets/web` directory served, the same 60 ms + 0..50 ms run measured a residual p95 of
0.70 m (legacy) against 0.01 m (smoothed) with 0 jumps, but both modes saw 24 to 28 frame stalls over
50 ms in twelve seconds, up to 450 ms, while models and terrain loaded; those are rendering and asset
loading stalls (`docs/ascent-performance.md`), not stream handling, which stayed under 0.2 ms per
message. A 450 ms stall also delays the stream, and the smoothed clock came within 15 ms of its 200 ms
snap limit after one; the player sees a freeze and then continuous motion rather than a freeze and a jump.

Not changed here: the 10 Hz stream, its contents and size (deflate would shrink `Vessels` only 1.5x and
the body `Snapshot` not at all, so no compression was added), interest management, and prediction of the
flown vessel from the player's own inputs (ADR-0002 keeps the flight model out of the browser; the
attitude presentation already follows the authoritative angular velocity).
