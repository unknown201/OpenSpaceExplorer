# Rigs and animation compared with KSP

Analysis and implementation: 2026-10-24. The concrete reference is the installed KSP2 build and its
authored assets, consistent with the rest of this repository. KSP1 Breaking Ground's hinges, pistons,
rotors and KAL controller are a separate gameplay system; exporting a skeleton does not implement
physical robotics. The official [Breaking Ground announcement](https://store.steampowered.com/news/posts/?appids=220200&enddate=1558702858)
describes that additional scope.

## Findings

OSE previously retained mesh-node rest transforms in its GLB files, then flattened them in the renderer.
The exporter omitted AnimationClip data, SkinnedMeshRenderer geometry, joints, weights and bind poses.
Consequently a flight command could change the simulation's solar/gear state without changing the mesh.
Part-joint flex already existed, but moving an entire part is not the same as animating its internal rig.

The local reference sources inspected under `reference/ksp2/Assembly-CSharp/` were:

- `KSP.Modules/Module_Deployable.cs` and `Data_Deployable.cs`: retracted, extending, extended,
  retracting and broken states; normalized clip time, reversal, deployment limits and tracking pivots.
- `KSP.Modules/Module_Gimbal.cs`: named model transforms, initial rotations, pitch/yaw/roll mapping,
  response smoothing, and either procedural rotation or animator parameters.
- `KSP.Animation/AnimationManager.cs`, `AnimationManager_EVA.cs`, `AnimationManager_IVA.cs` and
  `AnimationManager_Expressions.cs`: character animation layers, movement parameters, expressions,
  fidgets and playback/time-warp control.
- `KSP.Animation/LateUpdateConstraint_AimAt.cs` and `LateUpdateConstraint_Position.cs`: constraints
  applied after ordinary animation; model-space targets must be converted through the parent frame.
- Part bundle Transform, AnimatorController, AnimationClip, Mesh and SkinnedMeshRenderer data,
  including the solar accordion, retractable gear and landing legs.

| Component | KSP reference behavior | OSE after this change |
|---|---|---|
| Rig hierarchy | Named transforms, bones and procedural pivots | Hierarchy retained, validated and evaluated in parent order; matrix nodes preserved |
| Transform clips | Timed position/rotation/scale curves | glTF STEP, LINEAR and CUBICSPLINE; quaternion slerp/normalization; clamp, loop and seek |
| Authored part asset import | Unity streamed, dense and constant curve storage | Generic TRS curves decoded, sampled at source rate plus key times, exported as glTF channels |
| Skinning | Bone palettes, weights and bind matrices | Four influences, inverse bind matrices, mesh-relative palettes, normalized weights and normal transforms |
| Multiple instances | Each part owns a pose despite sharing its model asset | Pose evaluation and deformed vertex buffers belong to the draw instance |
| Gear/panel deployment | Clip follows part state and can reverse | Existing server commands drive original clips per craft part; reversal is continuous; first observation uses the current terminal state |
| Layer blending | Character body/face layers and masks | Engine API for weighted, masked local-pose blending; character state machines remain separate work |
| Late constraints | Aim and position constraints after animation | Engine APIs for model-space aiming and position copying through parents |
| Procedural gimbal/tracking | Response-limited transforms driven by flight inputs/sun direction | Bones and pivots survive import; game-specific actuator controllers are still missing |
| Wheels | Suspension, steering and spinning transforms | Skin and deployment supported; wheel spin, steering and suspension animation are still missing |
| EVA/IVA | Humanoid retargeting, locomotion and facial layers | Skin infrastructure supported; humanoid retargeting, full locomotion/expressions and blend shapes remain missing |
| Animated collision/drag | Deployment state affects physical behavior | Existing terminal-state gear/solar simulation retained; intermediate visual poses do not change collision or drag |

## Implementation

`crates/ose-client/src/animation.rs` is a browser-independent runtime. It reads checked glTF accessors
with offsets, stride, normalized integer weights and finite-value checks. It validates hierarchy cycles,
parent ownership, animation time ordering, channel sizes and skin references. Pose sampling starts from
rest on every evaluation; an animation cannot leak stale transforms into another instance. Blending
and late constraints operate on caller-owned poses before world matrices/palettes are computed.

`tools/export_animation.py` converts generic Unity transform curves. The Unity-to-glTF reflection applies
to translations, quaternions and inverse bind matrices. Quaternion signs remain continuous. Times are
deduplicated after float32 encoding, avoiding repeated keys produced by the union of authored key times
and the sampling grid. This is a sampled conversion, not a promise of exact Unity interpolation between
samples. Humanoid, material, visibility and blend-shape bindings are reported rather than silently
treated as transform curves.

The converter retains the rig's non-mesh transforms and only the highest-detail LOD renderers. It binds
animation paths relative to the controlled Animator. An unbound FBX dependency can have identical bone
paths; it must not overwrite the live prefab's animation targets. The regression suite covers this case.
Deployment clip selection uses the AnimatorController's semantic state, and controller ownership comes
from part modules. An engine's OPEN state is not automatically driven by the solar or gear controls.

`render.rs` retains node indices instead of discarding them during flattening. Animated node matrices
drive rigid meshes and CPU skinning drives skinned meshes through the same production material path.
Each skinned draw has a separate GPU vertex buffer. Lighting handles nonuniform node scale, and pose
bounds include deformed vertices. Large static environments do not retain an unnecessary CPU vertex copy.
CPU skinning and per-frame buffer allocation are a deliberate initial implementation; a GPU palette path
and buffer pooling are appropriate before scaling to many simultaneously visible characters.

`flight3d.rs` keys reversible deployment playback by vessel and craft part index and advances it with
simulation time. Pausing freezes it and time warp advances it. Late join/reload uses the observed terminal
state; the current protocol does not carry the original command timestamp or the in-progress fraction,
so joining during deployment cannot reconstruct the exact intermediate pose. Other deployment modules
(bays, ladders, antennae, science equipment, engine modes) are imported but have no new flight commands.

Protocol **26** adds the attached panels' individual indices/states to FlightReadout, alongside the existing
per-gear states. Update server and client together. Integration testing also found a pre-existing import
bug: generic Data_Deployable settings marked legs and other deployables as solar panels. The importer now
only preserves solar flags on solar-producing parts. Simulation/snapshot checks also guard older catalogs,
and absent/detached parts no longer count as deployed panels.

## Asset audit and verification

The local conversion examined 400 catalog entries and produced 398 part models, containing 54 skins and
409 clips with at least one resolved generic TRS channel. The two colony transport trunk entries had no
renderable mesh. Eight panel and eight retractable gear/leg controllers are connected to flight state.
These counts do not imply full character or whole-game animation parity.

Unresolved bindings remain on `cargobay_1v_1x8`, `engine_1v_methalox_dart`, `eva_kerbal`,
`landinggear_3v`, `landinggear_4v`, `parachute_1v`, and `solararray_1v_circular`. The largest group is
the EVA character's retargeting data. Each exported GLB records an audit count and up to 32 examples.
The successful generic channels are still usable, but those assets are not considered fully converted.

The independently evaluated closed/open model extents demonstrate actual deformation:

| Part | Folded extent | Deployed extent |
|---|---:|---:|
| Accordion solar array, X | 0.342 m | 7.315 m |
| Small retractable landing gear, Y | 0.449 m | 2.072 m |
| Small landing leg, Y | 0.870 m | 1.528 m |

Verification includes eight native animation tests (sampling, layering/constraints, instance isolation,
bind transforms, normals, reversal, accessor layouts and corrupt inputs), three synthetic Python exporter
tests, and a real WebSocket integration test with two identical solar panels plus a landing leg.
The latter verifies indices, independent module state, extension/retraction and protocol decoding.
All ten server integration tests, workspace library tests, native/wasm clippy and the WASM build pass.
An attempted full workspace run timed
out in the Mun bot's sandbox-blocked local-server health check; it is not recorded as a full-suite pass.

Chromium/WebGPU renders the production GLB/material/skinning path for the accordion array, landing gear
and landing leg. Closed, halfway and open images differ. A pixel comparison checks that rendering two
opposing poses together agrees with separately rendered instances, catching shared vertex-buffer aliasing.
This verifies our rendering integration, not pixel parity against Unity.

## Reproduction

Run from the repository root; the export needs the user's local KSP2 install and UnityPy environment.
Generated assets, model audits and screenshots remain gitignored.

```powershell
.venv/Scripts/python.exe -B -m unittest discover -s tools/tests
cargo test -p ose-client --test animation
cargo test -p ose-server --test ws_session deployment_snapshots_identify_panel_and_gear_instances
.venv/Scripts/python.exe -B tools/export_web_assets.py --no-bodies
cargo run -p ose-client --example audit_rigs -- assets/web/parts
```

Build the client with `trunk build --dist ../../target/rig-web --filehash false` from
`crates/ose-client`, then run a local server with `--static-dir target/rig-web --assets-dir assets/web`
on port 7796. `node e2e/scripts/check-rig-animation.mjs` saves screenshots under
`.cache/animation-audit/browser/`. `OSE_BASE_URL` selects another local test server;
`OSE_CHROMIUM_EXECUTABLE` selects an installed Chromium with WebGPU support. Local socket/browser tests
must run in an environment that permits loopback connections and browser subprocesses.
