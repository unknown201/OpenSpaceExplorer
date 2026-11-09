# Atmospheric heating feedback

## Conditions and evidence

An unpowered Mk1 pod starts at 52 km over Kerbin, travelling east at 3,000 m/s
relative to the rotating atmosphere, with vertical speed -300 or +300 m/s.
Use Tracking Station → Atmospheric test pod → Fly, then switch to the map.
The test uses an isolated save, in-memory accounts, 1× time and the real WebGPU
renderer with exported part and FX assets. It does not modify the player's save.

The local decompiled reference is
`reference/ksp2/Assembly-CSharp/KSP.VFX/FXReentryContextualManager.cs`:

- `CalculateReentryIntensity` evaluates the Mach³/density^0.53 flux, low/high
  density flux curves, density blend, altitude curve and surface velocity curve.
- `Update` activates above `ReentryMinimumIntensity`, then remaps that threshold
  to zero before passing intensity to the mesh controllers.
- `CalculateVelocityAndVesselSpace` uses surface-relative velocity, independent
  of whether the craft is ascending, descending or firing an engine.
- `SetReentryMeshesActive` excludes parts shielded from the windstream.

`ReentryMinAtmosphereActivationThreshold` exists in `VFXReentryGlobalSettings`,
but the reviewed contextual manager does not gate activation on this density.
OSE incorrectly did so. Its 0.001 kg/m³ cutoff suppressed the effect during the
tested upper-atmosphere pass. OSE also passed unremapped intensity to the shader
and submitted shockwaves for parts marked shielded by the server.

No new live KSP2 gameplay capture was made. Reference evidence is the local
source, previously exported authored geometry/settings, and independent execution
of the original Shockwave DXBC. The shader comparison is not a whole-game timing
or post-processing comparison. Existing source/asset provenance and renderer
limitations are in [particles.md](particles.md).

## Comparison

| Condition | KSP2 reference evidence | OSE before | OSE after |
|---|---|---|---|
| Fast unpowered descent in thin air | Surface velocity and flux determine activation | Zero shockwave layers at approximately 50.3 km | One authored pod shockwave layer at approximately 50.2 km; atmospheric heating indicator |
| Fast ascent through thin air | Same calculation; no descent requirement | Density cutoff also applies on ascent | One shockwave layer at approximately 54 km |
| Leave the atmosphere | Atmospheric flux vanishes | Plasma off | Plasma off; hot-part warning can remain until cooling |
| Slow cold atmospheric flight | Velocity/flux curves suppress plasma | Off | Off; no heat warning |
| Intensity crosses activation threshold | `Update` remaps minimum intensity to zero | Starts at raw minimum intensity | Starts at zero and scales through the authored curves |
| Part inside a closed fairing/bay | Shielded meshes disabled | Submitted for rendering | Server `shielded` parts have shockwave emission disabled; their geometry still participates in occlusion |
| Craft approaches temperature limit | Existing thermal simulation and heat glow | Glow/sound, temperatures in the part window | Adds persistent hottest-part bar and explicit high-temperature/overheating text |

The compact HUD is OSE presentation tuning: amber atmospheric heating while
incoming reentry flux reaches 1 kW, high temperature at 65% of a part's limit,
red overheating at 85%. These thresholds do not change damage, ablation or
destruction. Temperature warnings use authoritative part temperatures and work
without atmospheric flux; they clear when the craft cools, crashes, or has no
flight readout. They remain visible in map view. No screen flashing is added.

## Verification

Passed on 2026-11-09:

- `cargo test -p ose-client --lib`: 115 tests, including thin air, onset remapping,
  vacuum/subsonic rejection, invalid telemetry and hottest-part cooling feedback.
- `cargo test -p ose-sim thermal::tests`: 2 existing thermal physics tests.
- Native server build and `wasm32-unknown-unknown` client build.
- Real client/server browser checks: descending, ascending, slow and vacuum passes,
  map feedback, and an overheating pass reaching 86% of the pod temperature limit.
  The overheating fixture disables part destruction so the warning can be captured.
  All runs used real WebGPU and reported no page errors.
- `check-reentry-fx.mjs`: pod, Swivel and long tank; cold/vacuum off, hot/thin air
  on, returning to cold/vacuum off, and animated frames.
- `check_reentry_parity.py`: all six frames passed the existing 0.5% effect-relative
  RMSE budget against original DXBC. Five matched exactly; worst was 0.25885%.

Local ignored artifacts: [.cache/reentry-flight/comparison.html](../../.cache/reentry-flight/comparison.html),
`before.png`, `after.png`, `ascent.png`, `slow.png`, `vacuum.png`, `overheating.png`,
and corresponding JSON results. The before client uses the original `intensity`
function and hides the newly added HUD. Both captures use the same craft/assets,
camera and starting conditions; wall-clock capture differences produce a small
altitude/time difference. Original/WGSL shader captures and results are in
`.cache/vfx-fix/reentry/`. These artifacts are local, not available in a clean checkout.

## Reproduce

Build the server and client, serve the exported assets, and generate stable module
names for the FX fixture scripts:

```powershell
cargo build -p ose-server
cargo build -p ose-client --target wasm32-unknown-unknown --target-dir target-polish
wasm-bindgen target-polish/wasm32-unknown-unknown/debug/ose_client.wasm --target web --out-dir target/reentry-web --out-name ose-client
$index = Get-Content crates/ose-client/index.html -Raw
$index = $index.Replace('<link data-trunk rel="rust" />', '<script type="module">import init from "/ose-client.js"; await init();</script>')
Set-Content target/reentry-web/index.html $index
node e2e/scripts/check-reentry-flight.mjs
$env:OSE_VERTICAL='300'; $env:OSE_CASE='ascent'
node e2e/scripts/check-reentry-flight.mjs
$env:OSE_ALTITUDE='80000'; $env:OSE_VERTICAL='30'; $env:OSE_CASE='vacuum'
node e2e/scripts/check-reentry-flight.mjs
$env:OSE_ALTITUDE='52000'; $env:OSE_SPEED='40'; $env:OSE_VERTICAL='0'; $env:OSE_CASE='slow'
node e2e/scripts/check-reentry-flight.mjs
$env:OSE_ALTITUDE='30000'; $env:OSE_SPEED='3000'; $env:OSE_VERTICAL='-300'
$env:OSE_HEAT_TEST='1'; $env:OSE_CASE='overheating'
node e2e/scripts/check-reentry-flight.mjs
```

`OSE_STATIC_DIR` and `OSE_SERVER` override build paths; `OSE_REENTRY_PORT` overrides
7797. `OSE_BEFORE=1` records a baseline without after-behavior assertions. Existing
shader fixture commands are documented in [particles.md](particles.md).

## Requirements and remaining limits

Rebuild the client and serve the existing exported `assets/web/fx` assets. There is
no protocol or simulation change. The HUD also works without WebGPU; authored
shockwaves require WebGPU and the exported assets. Flight rendering still requires
a known craft document to draw authored part effects: the isolated saved fixture
includes the normal vessel-to-library craft link. Resolving arbitrary restored or
cross-device vessels without that existing link remains a separate limitation.
The 65%/85% HUD thresholds and 1 kW indicator threshold are OSE choices, not measured
KSP2 UI parity. No deployment was performed.
