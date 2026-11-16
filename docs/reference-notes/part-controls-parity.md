# Lights, legs, antennas and solar controls

Investigation: 2026-11-11. Reference: the installed KSP2 build 14669105,
extracted part JSON and prefab/AnimationClip data. This is an asset/source comparison;
a new live KSP2 gameplay capture has not been made for this investigation.

| Player action | KSP2 reference | OSE before | OSE fixed |
|---|---|---|---|
| Lights button or U | Module_Light switches lamps and emissive output, requests EC, posts on/off audio | Group flag and HUD indicator only | Powered lamps, visible emission and authored on/off sounds |
| Gear button or G | Landing legs and retractable wheels use their own opening/closing clips and timed PostAnimWWEvent cues | Reversed opening clip, one generic small-leg sound | Independent per-part clips and timed sounds; continuous reversal |
| Radio antenna control | Deployable antenna can extend/retract; fixed antennas remain fixed | No command or UI; imported rig unconnected | Button, authoritative state, animation and sounds |
| Solar button | Data_Deployable permissions apply; OX-4L/4W cannot retract, shielded panels can | All panels could retract; generic shielded-panel sound | Respect authored permissions and play each panel's timed clips/cues |
| Reload | Saved module state restored | Structure rebuild reset deployment | Restore attached-part state by stable part GUID |
| Lights while coasting in orbit | Lights work while the vessel coasts | ActionGroup only accessed Powered; a Dormant vessel refused the command | Update either flight state; electric services continue on rails without waking physics |

Reference sources under the ignored `reference/ksp2/Assembly-CSharp/`:
`KSP.Modules/Module_Light.cs`, `Data_Light.cs`, `Module_Deployable.cs`,
`KSPPartAudio.cs`, and `KSPPartAudioManager.cs`. Data comes from
`assets/ksp2/data/parts/` and the installed Addressables part bundles. Keep these
decompiled sources and game binaries out of commits.

The Communotron 16 opening clip has four audio cues at 0.0614, 0.5960, 1.1029
and 1.6375 seconds. LT-05 and shielded 1x6 panels have different cues and
separate closing clips. The spotlight prefab's on/off references resolve to
`Play_light_ON` and `Play_light_OFF`. These facts were read directly from assets.

## Implementation

The importer and committed part catalog now retain `Data_Deployable` extension,
retraction, initial-state and shielding permissions, and `Data_Light` resource
settings. Structure keeps attached-part deployment and effective powered-light
state. The new antenna and individual-panel/antenna commands use the same vessel
ownership checks as other flight controls. Fixed antennas and panels have no
deployment action. Extend-only panels remain extended; they do not prevent a
mixed group's reversible panels from being extended again.

Protocol **45** adds antenna and light-module snapshots, non-retractable part
indices, and antenna/individual deployment intents. Rebuild client and server
together. The action bar has a radio-antenna button. The parts manager exposes
antenna actions, individual panel actions and panel-specific state; it disables
retraction of an extended, non-retractable panel and labels fixed panels correctly.

`export_animation.py` retains `PostAnimWWEvent` times and each controller's
separate closing clip. `flight3d.rs` selects the commanded clip, advances it in
simulation time and plays crossed cues from that part's clip. It suppresses cue
replay at first observation. Save/load clears actuator playback on the next
restored snapshot, so restored terminal states do not replay an old transition.
The original representative leg/panel sounds no longer substitute for these cues.
`export_audio.py` resolves their Wwise events into local Ogg assets.

The prefab exporter retains actual lamp nodes, range, spot cones, brightness and
emission strength. The mesh shader receives up to eight nearby lamps; terrain
also receives local lighting. Lamp positions follow the part's animated rig.
All stock light modules examined have smooth transitions disabled, so the stock
toggle switches immediately. Lights request their authored EC rate and stop
emitting when power is unavailable, while retaining the selected switch state.

Deployment/gear state is saved by stable part GUID, avoiding an index mismatch
when lost parts are omitted from a save. Restoring gear also synchronizes its
physical support/drive state.

## Verification

All following checks passed on this Windows workspace:

- Native library suites: ose-client 115, ose-data 15, ose-importer 10,
  ose-sim 166, and ose-server 12 tests.
- Eight existing animation tests and three new control simulation tests cover
  independent groups, one-shot panels, shielding, lost parts, lamp consumption,
  power loss/recovery and coasting-vessel switches.
- The server's save tests include deployment/light restoration. All 20 enabled
  WebSocket integration tests pass; `multiplayer_latency_probe` remains ignored.
  The deployment integration test also exercises individual panel commands,
  antennas, powered lights, state isolation and rejection of another player's command.
- The exporter regression suite passes, including preservation of timed audio cues.
- `tools/check_part_controls.py` audits eight gear/leg rigs, eight panel rigs,
  five antenna rigs, 11 light modules (seven standalone lamps plus four gear
  lamps), and 187 applicable clip cues with existing exported sound files.
- Chromium runs the actual flight UI in a disposable Sandbox campaign with a
  crew pod, tank, engine, LT-05, Communotron 16, shielded 1x6 panel, OX-4L,
  spotlight and bulb at 100 km above Kerbin. It exercises button/U lights,
  button/G gear, antenna extension/retraction/reversal, mixed solar groups,
  and F5/F9 restoration. The check requires WebGPU and actual successful
  Web Audio buffer starts, and reports no browser errors.
- Deterministic production-renderer fixtures produce distinct stowed,
  halfway and extended images for all 21 rigs. Each of the seven standalone
  lamps changes its own visible emission and the rendering of a separate
  receiver surface. The receiver-only check hides the source mesh, so its
  own glow cannot make that assertion pass.
- Native server build, WASM check and release Trunk build pass.

Strict Clippy is **not clean** because of existing findings outside this change:
`input.rs:167` (`field_reassign_with_default`, native tests) and `analog.rs:63`
(`never_loop`, WASM). The same checks pass with just those respective lints
allowed. `git diff --check` passes.

Reproduction from the repository root (needs the local KSP2 install and exported assets):

```powershell
cargo test -p ose-sim --test part_controls
cargo test -p ose-client --test animation
cargo test -p ose-server --bin ose-server
cargo test -p ose-server --test ws_session -- --test-threads=2
.venv/Scripts/python.exe -B -m unittest discover -s tools/tests
python tools/check_part_controls.py
node e2e/scripts/build-client.mjs
# Run this server in another terminal, then seed its local browser fixture:
target/debug/ose-server.exe --listen 127.0.0.1:7797 --static-dir target/web --assets-dir assets/web --db :memory: --save none
cargo run -p ose-server --example part_controls_fixture -- 127.0.0.1:7797
node e2e/scripts/check-part-controls.mjs
node e2e/scripts/check-part-control-visuals.mjs
```

Regenerate affected models with `tools/export_web_assets.py --no-bodies --parts`
followed by the applicable part ids, then run `tools/export_audio.py`. This work
regenerated the 30 affected models and the sound manifest (201 sound entries).
Generated game-derived files remain ignored; ship the refreshed assets with the
new client and server. No running public deployment was changed.

Local evidence, not included in a clean checkout:

- [Labeled OSE pose/light contact sheet](../../.cache/part-controls/comparison.png).
- [Browser states and actual audio-start observations](../../.cache/part-controls/browser-results.json).
- [Individual production-renderer captures](../../.cache/part-controls/visuals/).

## Limits of the comparison

This establishes working OSE button/input/protocol/render/audio paths using the
installed game's authored data. It does **not** establish pixel, wall-clock
timing, or acoustic parity with a running Unity session. The contact sheet is
labeled OSE; it does not pretend to contain KSP2 gameplay captures. The orbital
UI captures and isolated fixture captures use different cameras and lighting.

Audio cues use an exported representative Wwise media variant, the existing OSE
effects gain and a nonspatial Web Audio mix. Wwise randomization, spatialization,
occlusion and its complete mixer are not reproduced. Animation/audio follows
simulation time; server load and rendering cadence can affect wall-clock timing.
Continuous reversals map normalized progress between the authored opening and
closing clips; this is not a full Unity Animator transition graph.

The snapshot/save format carries terminal commanded states, not the command's
start timestamp or intermediate deployment fraction. Joining or loading during
motion snaps to that terminal state. Existing solar tracking/production and
intermediate-pose collision/drag approximations are separate from button
deployment. Local lamp lighting has bounded light counts and no lamp shadow maps;
the terrain-light shader path has not received a separate night-time Unity
comparison. Advanced PAM light color/blink/aim controls remain outside this change.

The subsequent [live services audit](services-live-parity.md) measures solar EC,
CommNet transitions, transmitter interruption and flag behavior separately. Its
scripted runtime evidence does not establish live light/leg/audio parity for this
earlier asset/source investigation.
