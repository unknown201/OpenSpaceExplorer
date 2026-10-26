# Ascent frame stalls

The distant-scatter placement pass (`ScatterFile::place_far`) rebuilt the entire 2.5 km disc of rocks
and trees whenever the camera moved 60 m in any direction. A vertical launch repeatedly ran millions
of candidate-cell checks even though the ground under the camera had barely changed. The WASM
CPU profile showed time in placement, biome-mask lookup and trigonometry, with individual render
callbacks exceeding 200 ms. Suppressing GPU draws did not remove those CPU stalls.

The renderer now measures lateral movement on the planet surface, reusing the existing placement
during vertical ascent. The cached positions remain anchored to the original placement origin; the
existing shader uses current camera distance for fading. Above the scatter range the placement and
pending work are discarded, and descent schedules a new placement.
Integration with newer main also preserves the authored vegetation mask: empty mask areas remain
empty during resumed jobs. Low planet detail cancels pending placement and clears distant props.

Necessary rebuilds resume across frames with a 1.5 ms scheduling budget, checked every 256 terrain
samples or candidate cells. The old GPU buffer remains visible until the replacement is complete.
Changing terrain exclusions invalidates both the pending job and the old buffer. Final nearest-instance
selection uses partitioning instead of sorting every discarded instance. This is a scheduling budget,
not a hard real-time bound on an individual work chunk or the final upload.

## Local before/after

On 2026-10-25, release builds from `bf202ec` with and without only this scatter change, full local
assets, Chromium/WebGPU at 1920 × 1080, and a fresh private server for each run:

| Phase | FPS before | FPS after | CPU callback p95 before | CPU callback p95 after |
| --- | --- | --- | --- | --- |
| Pad | 55.5 | 56.1 | 7.9 ms | 6.7 ms |
| Ascent, around 2 km | 14.8 | 55.9 | 225.3 ms | 6.9 ms |

The ascent frame-interval p95 fell from 217 ms to 18.5 ms. These are short local samples, not a promise
of 120 FPS on every machine. Earlier diagnostic runs were capped near 32 FPS even with all GPU draws
suppressed; their CPU profiles showed the same placement stalls. No rendering-quality settings were
lowered for the change. Initial distant props can appear later while their first placement builds;
subsequent updates retain the old placement until the replacement is ready.

## Reproduce

Build a release client and serve it from a private test server with the full `assets/web` directory.
From `e2e`, run in PowerShell:

```powershell
$env:OSE_BASE_URL = 'http://127.0.0.1:7788'
$env:OSE_FLIGHT = '1'
node scripts/profile-ascent.mjs ../target/ascent-profile.json
```

The script launches a Tin Can / FL-T200 / LV-T45 rocket, warms each phase for ten seconds and samples
three seconds on the pad and during ascent at 1920 × 1080. It writes JSON, screenshots and Chrome CPU
profiles. `OSE_SKIPS` is a comma-separated list of GPU pipeline-label substrings to suppress, with an
empty entry for normal rendering and `all` for CPU isolation. Flight mode defaults to normal/all.
Unset `OSE_FLIGHT` for fixed planet views; `OSE_ALTS` selects their altitudes in metres.

GPU pass timing uses WebGPU timestamp queries when available. Instrumentation adds overhead. Browser
frame pacing can cap FPS even when all drawing is suppressed, so compare CPU callback times and GPU
times as well as FPS. This probe is diagnostic, not a machine-independent FPS assertion.

Native regression tests cover vertical versus lateral movement, yielding within the candidate budget,
resuming across item/package boundaries, deterministic ground placement, the nearest-instance cap and
placement after returning from above the render range. Run `cargo test -p ose-client planet_scatter`.
