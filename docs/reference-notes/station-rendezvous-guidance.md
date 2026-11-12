# Station rendezvous guidance: implementation and validation

## Scope and reference conditions

Implements the [station assembly experience](../station-rendezvous-guidance-design.md):
manually launch the core; choose a destination for a delivery; optionally receive launch-plane,
intercept, braking, staging-point and port-alignment advice. A distinct outlined square on the
navball can be held with an explicit attitude-only action. Off, Minimal and Guided preferences
are available in flight and settings. World markers and audible burn prompts are separate options.
There is no automatically scheduled burn, throttle, staging or translation command in the director.

This is an OSE feature design, not a reproduction of an existing KSP2 rendezvous assistant.
The installed KSP2 game was not run for this investigation. Local decompiled reference files
`reference/ksp2/Assembly-CSharp/KSP.Modules/Module_DockingNode.cs` (particularly
`OnFixedUpdateAcquireDockerState`, `ValidateDockingConfiguration`, `ApplyAcquireForces`) and
`Data_DockingNode.cs` establish the existing acquire/alignment/relative-speed capture semantics.
That source inspection does not establish runtime or visual parity with PhysX.
The implementation uses stock exported part definitions, attach nodes, port dimensions and
capture limits; it retains authoritative automatic capture instead of adding a Dock button.

The replica's module topology and eight large solar arrays follow the
[NASA assembly elements](https://www.nasa.gov/international-space-station/international-space-station-assembly-elements/)
and [station reference guide](https://www.nasa.gov/wp-content/uploads/2022/06/167130main_assembly.pdf).
It is a stock-part silhouette/topology replica in Kerbin orbit, not an exact ISS engineering model.
Its central truss support is raised to keep service engines and solar wings clear of the axial modules.

## Comparison

| Player action or condition | Reference / design | OSE before | Implemented behavior |
|---|---|---|---|
| Launch a delivery | Optional advice; player flies | No rendezvous launch advice | Destination picker, optional orbital-plane countdown and heading; manual launch |
| Plan an intercept | Player flies burns | Ordinary target and automatically executed scheduled maneuvers only | Advisory Lambert transfer checks atmosphere clearance, propagation accuracy and departure plus braking fuel |
| Aim / burn | Explicit optional attitude assistance | Ordinary target/SAS directions | Separate navball square, aim error, finite-burn countdown, measured delta-v remaining; throttle stays manual |
| Imperfect finite burn | Make correction visible | No assembly director | Predicted pass distance and explicit Replan correction instruction |
| Approach a rotating station | Use authored port frame | No chosen-port guidance | Port-face range, closing speed, total relative speed, lateral drift, angular error and outside-bounds staging marker |
| Aligned Sr ports close | Existing acquire/capture semantics | Flat port/probe bounding spheres could bounce metres too early | Aligned port contacts excluded; other sphere candidates require overlapping authored boxes for that docking pair |
| Dock to a dormant station | Keep close target physical | Station could return to rails before capture | Targeted same-body docking neighbours stay physical within 500 m; existing physics warp cap applies |
| Dock / undock deployed hardware | Preserve the parts' state | Rebuilding the craft silently retracted solar arrays | Per-part deployment, temperatures and other saved states follow explicit part maps through merge and split |
| Reuse modules / observe a merge | Stable identity | Duplicate GUIDs or absorbed targets possible | Preserve destination GUIDs, deduplicate incoming identities, remap observer targets, require replanning after geometry changes |
| Change target, port or controls | Clear player ownership | No director state | Manual steering releases the hold; occupied/incompatible/obstructed ports pause advice; no silent port substitution |
| Reload / change assistance | Configurable | No preference | Per-campaign/vessel destination and port GUIDs, global assistance/marker/audio preferences |

## Implementation

- `ose-math/src/rendezvous.rs`: universal-variable Lambert solver, independently propagated
  encounter checks, safe-periapsis and complete fuel-budget rejection, multi-revolution phasing
  alternatives, encounter-preserving midcourse corrections, relative motion and analytic
  rotating-pad plane opportunities. Extra fuel does not hide a cheaper phasing option.
- `ose-client/src/rendezvous.rs` and `rendezvous_ui.rs`: advisory phases, authored port geometry,
  reflected Unity Z coordinate convention, centre-of-mass offsets, angular port velocities,
  active-engine budget/thrust estimates, authoritative velocity-sample burn progress and UI.
- `ose-client/src/navball.rs`, `flight3d.rs`, `index.html`: separate cue, optional world markers,
  alignment reticle, launch preparation and settings.
- `ose-sim/src/docking.rs`, `collide.rs`, `lib.rs`: docking-neighbour physics retention,
  before-contact port alignment, conservative box rejection, stable merges and target remapping.
- `/parts.json` adds docking definitions and RCS thrust metadata. The binary protocol is unchanged.

The UI releases guidance holds on phase changes and after five seconds without live vessel data.
Warp actions require a click and stop ahead of finite departure burns; shared warp ownership and
the existing safety cap remain in effect. It never changes the current control point.

## Reproduction and evidence

All generated saves, logs and captures are local, ignored artifacts. Use an isolated server;
do not overwrite a player's save. With the standard part/body data available:

```powershell
python tools/build_iss_test_crafts.py --output .cache/iss-validation
$env:CARGO_TARGET_DIR='D:/Projects/OpenSpaceExplorer/target'
cargo run --release -p ose-sim --example iss_assembly -- .cache/iss-validation 10
```

The test pilot launches Zarya from the surface, then independently launches Unity, Zvezda,
Destiny, Harmony, Columbus, Kibo, the central truss support, and both solar trusses. It uses
ordinary stage, attitude, throttle and RCS controls, shared advisory planning, measured finite
burns, correction replanning and authoritative docking capture. It verifies payload survival,
unique part identities and each added docked joint. It never teleports deliveries or directly
merges craft. Precision translation uses pulses above the real RCS mixer's 0.05 input deadband.

Generated checkpoints (`core-in-orbit.json`, `intercept-flight.json`, `approach-flight.json`,
`assembled-station.json`) open directly in ose-server. `--resume`, `--approach` and
`--resume-station` continue these actual flight states for diagnosing a failed leg.
`assembly-report.json` records the completed module/joint/part counts.

Run the real-browser checkpoint test with `e2e/scripts/check-station-rendezvous.mjs`, pointing
`OSE_BASE_URL` at an isolated localhost server loaded with `intercept-flight.json`, and
`OSE_TEST_OUTPUT` at a local capture directory. It checks real WebGPU assets, preferences,
manual steering override, optional pad advice, unchanged throttle on hold, and a manually
flown burst reducing measured delta-v. `e2e/tests/rendezvous.spec.ts` adds repeatable stock-craft
coverage to the regular Playwright suite without needing a generated ISS save.

## Verification results (2026-11-11)

- Completed the stock-part ISS replica: **10 modules, 179 parts, 9 docked joints, 8 large
  solar arrays**. Every expected payload GUID survived. Core and delivery launches were from
  the surface, followed by finite burns and RCS approaches; development retries resumed actual
  saved flight checkpoints. The retained assembly reached UT 53,131 s. No vessel was teleported
  or directly merged by the pilot.
- `cargo fmt --all --check`, workspace Clippy with `-D warnings`, full workspace tests,
  36 Python tests, WASM check and release Trunk build passed. Later correction/part-state
  changes also passed targeted tests and the workspace suite with the previously passed long
  Mun flight omitted. Seven advisory math tests include retained-encounter correction and
  economical phasing; docking regressions cover physical retention, identity, deployment
  preservation and undocking.
- Chromium and Firefox both passed `rendezvous.spec.ts`. The real ISS checkpoint browser
  script passed with WebGPU, actual assets and no page errors: advisory planning, attitude-only
  hold, manual steering override, Off/Minimal, reload, optional pad advice and a manually flown
  burst updating delta-v. The actual final-delivery approach also passed port selection and
  port-relative readout checks.
- One repeated local full-suite run failed the existing Mun bot's predicted-versus-actual
  periapsis check (65 km predicted, 21 km after handoff). The next full run passed; no assertion
  was relaxed. This is recorded separately from the station validation.

Local artifacts from this run (ignored; not present in a clean clone):

- `D:/Projects/OpenSpaceExplorer/.cache/iss-final/assembled-station.json` and
  `assembly-report.json`; stage-by-stage logs are `assembly.log`, `assembly-rest.log`,
  `assembly-truss.log`, `assembly-last.log` and `assembly-last-verified.log`.
- `D:/Projects/OpenSpaceExplorer/.cache/iss-guidance/browser/`: intercept/launch captures
  and the manual-burn browser report. `e2e/scripts/capture-iss-assembly.mjs` reproduces the
  final station and port-approach captures; its optional `OSE_MODEL_OVERRIDE` selects freshly
  exported solar GLBs without overwriting shared local assets.

Use current exported solar models when reviewing deployment. The existing local solar GLBs
were stale: fresh exports from the current `tools/export_web_assets.py` restored the animated
panels. The review asset overlay at `.cache/iss-release/assets` combines these current solar
models with the shared exports. Neither decompiled game sources nor generated game assets
are committed. Captures establish OSE presentation only; no matched original-game capture
or visual parity claim is made.

## Limits

Launch-plane timing is not an ascent or intercept prediction. Learned ascent profiles are not
implemented. Lambert plans are two-body impulsive estimates; finite burns and steering errors
can require several corrections. Launch and flight controls remain the player's responsibility.
Fuel estimates use current reachable propellant and conservative remaining-engine Isp, with a
reserve; they do not model every future engine-mode or crossfeed change.

Staging uses station/delivery catalog bounds; deployed solar wings can extend beyond these boxes.
Obstruction checks test the selected port's
centre line against authored boxes; they do not certify a swept delivery hull. Visual clearance
is still required. The contact fix uses box proxies for an aligned docking pair; other contacts
retain the existing sphere model. No mesh-collider or pixel-level KSP2 parity is claimed.
