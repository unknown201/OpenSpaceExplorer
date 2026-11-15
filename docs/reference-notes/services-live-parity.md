# Live solar, radio, transmission and flag measurements

Investigation: 2026-11-12. This audit adds runtime measurements to the earlier
source/asset comparisons. It does not change gameplay or claim full alignment.

## Conditions and evidence boundary

The installed KSP2 **0.2.2.0, Steam build 14669105**, runs through its existing BepInEx loader in the
dedicated, initially empty **OSE Services Validation** sandbox. The real OAB loads
and launches from the orbital OAB environment a five-part fixture: Mk1 pod, short
1.25 m methalox tank, fixed OX-STAT,
shielded 1×6 panel and Communotron 16. Infinite Power is disabled. The current dirty
OSE tree runs the same part list through its simulation; separate live WebSocket
sessions add a surface sampler and omit solar generation for the full/empty-battery
transmission cases. These are explicit setup differences, not identical craft runs.

The native benchmark changes EC quantities and antenna ranges and uses surface
teleports. It invokes the actual transmitter with a synthetic payload, so no native
research report or science credit is fabricated. It records the real module state,
resource consumption, graph results, panel output/pivot poses and flag objects.
Native flag removal invokes the real interaction callback; pointer selection,
interaction reach and keyboard input are not measured. OSE uses real experiment
reports and production network intents, including save/load and reconnect.

Every native record carries wall time and universal time. Scenario holds use
**universe time**, with a real-time timeout, because this game's simulation can run
slower than wall time. The initial reference attempts exposed an incomplete fixture
and inadequate wall-time waits; they are retained separately and excluded from the
final comparison. Do not mix their records into the accepted run.

The accepted evidence consists of **three distinct captures**: the services run
(`native/`, 3,031 frames), a normal terrestrial launchpad flag run (`native-flags/`,
1,387 frames), and a four-phase, 100 km circular-orbit solar run (`native-solar/`,
1,635 frames). The original services run's teleported flag attempt was rejected;
its flag result is excluded. The accepted launchpad run uses the EVA input handler
to jump off the ladder, with no teleport. The solar supplement records actual
body-frame position/star vectors and altitude after each orbital teleport. Low
output after the earlier longitude teleports did **not** establish an eclipse.

Source/binary/craft hashes and the dirty-tree revision are retained in the local
provenance manifest. Game-derived fixtures, captures, binaries and decompiled code
remain ignored. Local artifacts are not implied to exist in a clean checkout.

## OSE runtime findings

| Case | Measured OSE behavior | Implication |
|---|---|---|
| Fixed panel, local Y rotations 0° / 90° / 180°, normalized sunlight 1 | +0.075 EC in each one-second, 50-tick run | Production is invariant under these orientation changes |
| Deployed tracking panel under the same conditions | +1.000 EC/s for all three orientations | Output follows the tracking approximation; articulated tracking response is not established |
| Either isolated panel with sunlight 0 | 0 EC produced | Electrical service accepts zero illumination |
| Actual celestial geometry | Lit-side factor about 1.000103; opposite-side factor 0 | The body-shadow path is exercised separately from injected sunlight |
| Three antenna-range cuts and returns | First transition 1.00 simulated second; remaining transitions 0.66 s at the chosen phase offsets | Normal network updates occur at integer simulated seconds; they are not every tick |
| Real CrewObservation + SurfaceSurvey data/sample, full battery | All three reports submitted and 28 science credited; EC stays 50 | Transmission is immediate and consumes no EC |
| Same reports in a fresh zero-EC campaign | All three submitted and 28 science credited; EC stays 0 | Power cannot interrupt the current instantaneous submission |
| Flag placement twice by one EVA Kerbal | Both accepted; simulation-only surface offset 1.500000 m | OSE does not consume a carried flag |
| Live server flag placement | About 1.598 m spatial separation including height/terrain/settling | This is not the same metric as a horizontal authored offset |
| Flag recovery | RecoverVessel succeeds; quickload restores the saved flag | Vessel recovery works; Kerbal pickup has no network intent |
| Transport reconnection | Resume handshake and saved flag received | Separate from in-game CommNet connectivity |

The local WebSocket submission and handshake latencies are recorded in
`ose-ws-summary.json`; millisecond results on loopback are handler/transport timing,
not simulated packet duration or a general network performance budget.

## Live KSP2 / OSE comparison

| Observable | KSP2 runtime | OSE runtime | Alignment |
|---|---|---|---|
| Body eclipse | 289 frames with `Kerbin` blocker, illumination scale 0 and both producer outputs 0; battery change below 0.000001 EC during the eclipse phase | Geometric eclipse sunlight 0; zero-input electrical runs produce 0 EC | Qualitative on/off agreement |
| Solar incidence / tracking | Main attitude: fixed output 0, tracking output up to 0.0000142 EC/s; after a different surface orientation, fixed output reaches 0.1435 and tracker 0.1119 EC/s | Fixed output 0.075 EC/s and deployed tracker 1 EC/s at normalized sunlight 1, invariant under the tested local rotations | Exact attitude, flux normalization and tracking response not matched; no solar parity claim |
| Three range cuts | 2.90 / 3.04 / 2.76 simulated seconds; 18.926 / 19.938 / 18.124 real seconds in this slow reference run | First transition 1.00 simulated second; later phase-offset cuts 0.66 s | Timing mismatch |
| Three range restorations | 1.88 / 2.00 / 1.72 simulated seconds; 12.327 / 13.125 / 11.237 real seconds | 0.66 simulated second at the chosen offsets | Timing mismatch |
| One native 5-unit packet | Completion callback after 2.020 simulated seconds / 13.330 real seconds; battery 50 → 28 EC | Actual reports submitted in about 0.9 ms on loopback, with no EC use | Duration and power mismatch; payloads are different |
| Remove power during a long transmission | `OutOfResources` after 0.020 simulated second; restoring EC alone leaves this direct transmitter stopped; explicit restart accepts previously completed packet units | Empty battery still submits all reports and awards 28 science | Power interruption mismatch; native research-manager queue resumption is not tested |
| Remove radio link during a long transmission | `LostConnection`; direct transmitter remains stopped on reconnection | Transmission finishes immediately, leaving no in-flight packet to interrupt | Interruption mismatch |
| Plant, then plant again | Placement accepted at 0.450022 m spatial offset; carrying flag becomes false; second plant rejected | Both plant intents accepted; 1.500000 m simulation surface offset, about 1.598 m spatial separation in the server/terrain fixture | Placement/inventory mismatch; surfaces/orientations not a matched spatial pair |
| Pick up / recover | Actual removal callback destroys the flag and restores carrying=true | RecoverVessel removes the flag; no Kerbal pickup intent; saved flag restores on quickload and transport reconnect | Different player operation; OSE persistence verified |

Native transmission progress is continuous fractional **packet units** exposed by
the component, not evidence of discrete per-frame network packets. The observed
22 EC cost over about two simulated seconds supports an 11 EC/s runtime drain for
this antenna. The reference game's low simulation rate is a capture condition,
not a claim about normal KSP2 performance. Link latency is bracketed by the last
old-state and first new-state frame in the structured report.

## Reference path and comparison

Relevant native classes are `Module_SolarPanel`, `PartComponentModule_SolarPanel`,
`Module_Deployable`, `CommNetManager`, `TelemetryComponent`,
`PartComponentModule_DataTransmitter`, `Data_Transmitter`, `KerbalBehavior`,
`EVAUtils`, and `FlagBehavior`. Source readings explain the measurements; authored
rates or constants alone are not runtime evidence. The live Communotron reports
5 units per packet and a 2-second transmission interval.

OSE paths: `Structure::electric_services`, `Universe::sunlight`, the once-per-second
CommNet update in `Universe::step_active`, `Universe::plant_flag`, and the server's
`science_request`. A science transmission explicitly rebuilds CommNet immediately,
so its acceptance need not wait for the normal periodic connectivity snapshot.

Exact solar incidence curves, tracking speed/limits, self-shadow raycasts, deployed
collision/drag, native research inventory resumption, flown radio range crossings,
native flag save/reload, flag pointing and OSE browser presentation are not matched
by this audit. Body-shadow on/off agreement alone cannot establish solar parity.
The launchpad screenshots retain the vessel camera and do not resolve the flag or
Kerbal clearly; placement and pickup conclusions come from live object/inventory
telemetry, not a visual alignment measurement.

## Reproduce and assess

See [the capture tool instructions](../../tools/services-parity/README.md).

```powershell
cargo run --offline -p ose-sim --example services_probe -- .cache/services-parity
cargo test --offline -p ose-server --test services_runtime -- --nocapture
.venv/Scripts/python.exe -B -m unittest discover -s tools/services-parity -p test_analyze.py
.venv/Scripts/python.exe tools/services-parity/analyze.py .cache/services-parity
.venv/Scripts/python.exe tools/services-parity/plot.py .cache/services-parity
```

The WebSocket test verifies a successful capture, not parity. The analyzer refuses
unfinished/truncated native traces, missing link transitions, missing native power
interruption/completion, missing flag placement/pickup, and infinite-power runs.
It exits 1 for measured differences and 2 for incomplete or unmatched evidence.
The reader must inspect the stated setup differences before comparing numbers.

Verification: both native plugin variants and the solar extension build with zero
warnings/errors. The OSE live WebSocket capture passes; four capture-integrity unit
tests pass; Clippy with `-D warnings` passes for the simulation example and server
integration target. The comparison intentionally exits **1**, reporting three
measured mismatch areas, qualitative eclipse agreement and partial tracking
evidence. The HTML report was opened in Chromium with all 10 images loaded and its
rendered layout inspected. The audit plugin was removed, capture inputs/campaign
archived, and the original autorun marker restored byte-for-byte; see local
`cleanup.json`. The existing loader and unrelated plugins were retained.

Local evidence:

- [Standalone comparison and trace plots](../../.cache/services-parity/comparison.html).
- [Structured comparison](../../.cache/services-parity/comparison.json).
- [OSE simulation measurements](../../.cache/services-parity/ose-sim.json).
- [OSE live server summary](../../.cache/services-parity/ose-ws-summary.json).
- [Native event log](../../.cache/services-parity/native/events.jsonl).
- [Native per-frame samples](../../.cache/services-parity/native/samples.jsonl).
- [Launchpad flag event log](../../.cache/services-parity/native-flags/events.jsonl).
- [Orbital solar event log and geometry](../../.cache/services-parity/native-solar/events.jsonl).
- [Source/binary/craft provenance](../../.cache/services-parity/provenance.json).
