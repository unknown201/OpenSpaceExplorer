# Building supports compared with KSP2

2026-11-11. Submitted against main `0c24eac`. Reference: locally installed KSP2
**0.2.2.0.32913**, identified in the installation's `Ksp2.log`. Evidence comes from
the local implementation and shipped part JSON. No matched live KSP2 support-load
capture was made; the numerical results below measure OSE, not KSP2 error.

The initial working checkout was older than main and lacked two-anchor authoring.
Main already has that editor, paired anchor persistence, symmetry targeting and
two-end rendering. This change preserves those implementations and fixes the
remaining differences below.

| Action / condition | KSP2 source or asset | Main before | Fixed behavior |
|---|---|---|---|
| Build with I-beams, trusses and panels | Authored masses, nodes and physical modes | Catalog already present; ordinary parts bend/break through their parent links | Preserve physical beam/truss behavior; overlap alone creates no attachment |
| Load a strut-braced assembly | Virtual size-zero stack joint between the anchors' parents; infinite break limits | Permanently multiply every spanned joint's stiffness, damping and break limits at build time | Solve coupled endpoint translation/orientation springs on the actual parent path; keep original parent-joint strengths |
| Destroy an anchor or split the parent path | `CheckEndsStillConnected` destroys the virtual joint | The one-time strength multiplication remains until the structure is rebuilt | Retire the brace immediately when either anchor disappears or the tree separates; unrelated joints keep their existing integration |
| Extend a strut | Shipped range is 0.01–10 m, overriding the C# 100 m default | No extent check | Validate the complete symmetry set before creating far anchors, and validate uploaded craft at save/pad/orbit launch boundaries |
| Double-click a strut in the picker | A placed first anchor requires a second placement | Shortcut adds an unlinked anchor without starting the second-end tool | Start the existing compound-part tool after shortcut placement |
| Use the tiny physicsless truss | Physics mode `None` shares a physical ancestor's body | Independent flexible, breakable link | Unbreakable rigid parent attachment; physical children use the physical ancestor's strength data |
| Run engines while held by clamps | Frozen clamp body, unbreakable joint, explicit staged release | Ordinary clamp joint; first-stage clamps may be trimmed before launch; thrust can lift through remaining clamps | Keep clamps until their stage fires, hold the rotating-ground position while fuel burns, and release for liftoff |
| Reload a clamped craft | Keep ground attachment | No explicit attachment state | Persist anchoring with powered state and preserve the held ground radius when rebuilding the structure |
| Render moving connectors | Connector follows both anchors | New cached GPU strip per rounded span, allowing stale geometry and accumulating buffers | Stable per-connector cache keys and in-place endpoint updates in VAB and flight |

## Reference evidence

Local, ignored source under the original workspace's `reference/ksp2/Assembly-CSharp/`:

- `KSP.Modules/Module_Strut.cs`: `CreateJoint`, `TryGetConnectedPart`,
  `CheckEndsStillConnected`, `DestroyJoint`. The brace joins anchor parents and its
  break force/torque are overridden to infinity.
- `KSP.OAB/ObjectAssemblyCompoundPartTool.cs` and `KSP.Modules/Data_CompoundPart.cs`:
  paired placement, `OtherEndAnchor`, `IsFirstAnchor`, extension limits.
- `KSP.Sim/AttachNodeData.cs`: the dynamically constructed strut node defaults to size zero.
- `KSP.Sim.impl/PartOwnerBehavior.cs`: physical-ancestor lookup and the clamp exception
  in `GetJointBreakLimits`.
- `KSP.Sim.impl/RigidbodyBehavior.cs`: physicsless parts reuse an ancestor's active body.
- `KSP.Modules/Module_GroundLaunchClamp.cs`: fixed-update freezing and staged `Release`.

The local `assets/ksp2/data/parts/strut_1v_extensible.json` supplies the **10 m** maximum;
the 100 m class default is not the shipped limit. The part catalog already records
EAS-4 mass 0.002 t per anchor, clamp 0.100 t, pocket I-beam 0.176 t, and the tiny
physicsless truss 0.001 t. No game binaries, decompiled source or original assets are
added by this change.

## Physics and implementation limits

The existing paired-anchor schema (`fuel_line_target`, `compound_ends`) is retained.
`rigid/support.rs` builds six linearized constraint rows: three relative translation
and three relative rotation components. Shared ancestors cancel; each row couples
the flexible joints between the two anchor parents. Backward Euler with diagonally
preconditioned conjugate gradients solves the coupled system. Linear reactions enter
parent-link force checks. A strut itself does not break from load; physical parent
joints can still fail and release it. Removing an anchor never permanently strengthens
the remaining craft.

This remains OSE's rigid-tree/flex-layer approximation, not PhysX equivalence. Angular
stiffness and damping use the existing loaded `joint_settings()` size-zero stack values
(750,000 kN m/rad and damping factor 0.01 in the committed settings). Translation uses
a finite 1e8 kN/m penalty rather than the reference's near-infinite drive. Subtree
inertias and large-deflection motion remain approximate.

Physicsless parts retain rigid bookkeeping links instead of literally sharing a Rapier
body. Clamps cancel net applied force at live clamp parts and constrain the vessel to
the rotating surface. Detailed clamp load/moment distribution, gravity preload, tower
extension and release animation remain approximate. Bounds contacts still approximate
hollow truss colliders, and connector spans still use the existing line renderer.

No wire-format change: main's protocol **64**, authoritative craft revisions, paired
anchor persistence and ownership checks are retained. The new powered-state JSON field
defaults to false in old saves and is refreshed from attached clamps.

## Reproduction and validation

In Sandbox, open the VAB. Double-click a pod, tank, engine and EAS-4 in the picker;
click the pod surface to complete the brace. Undo/redo, save and launch. The browser
script exercises these real UI actions, waits for loading overlays to disappear,
requires WebGPU and exported meshes, and checks browser errors.

The load fixture uses stock Kerbal K1: brace booster part 21 to the root, apply
equal/opposite 100 kN m torques, and run 100 steps of 0.02 s with no gravity or
translational force. Final booster-origin displacement is **0.008164 m unbraced** and
**0.000705 m braced**, about **91.4% less displacement**. This is an OSE regression
measurement, not a calibrated KSP2 motion comparison.

Other regressions cover anchor removal and separation, unchanged parent break limits,
unlinked anchors, physicsless versus full beams, extent limits, compaction and JSON
round trips. The clamp fixture ignites one stage before release, waits 50 ticks,
checks fuel burn and fixed ground radius, serializes/restores, then releases and
verifies liftoff. A real WebSocket test saves paired anchors, confirms a burning rocket
remains held, releases its clamp, and checks the authoritative current craft.

```powershell
cargo test --workspace --exclude ose-bot
cargo clippy -p ose-data -p ose-importer -p ose-sim -p ose-client -p ose-server --all-targets
cargo check -p ose-client --target wasm32-unknown-unknown
```

Main-based results: **660 tests passed, three existing tests ignored**; Clippy and
WASM checks passed. The initial full workspace attempt on the older checkout stalled
in the Mun bot's nested Cargo startup, so that bot was excluded from the completed runs.

Build and serve the browser regression in separate terminals. Run from the submitted
worktree; use absolute asset paths when the exported assets live in another checkout:

```powershell
cd crates/ose-client
trunk build --release --dist ../../target/support-web
cd ../..
cargo run -p ose-server -- --listen 127.0.0.1:7807 --static-dir target/support-web --assets-dir assets/web --db :memory: --save none
# Separate terminal:
cd e2e
node scripts/check-building-supports.mjs
```

Local artifacts, absent from a clean checkout: [browser results](../../.cache/support-parity/browser-results.json),
[visual comparison](../../.cache/support-parity/comparison.html),
[unconnected anchor](../../.cache/support-parity/ose-before-connection.png),
[connected brace in VAB](../../.cache/support-parity/ose-braced-vab.png), and
[saved craft on pad](../../.cache/support-parity/ose-braced-pad.png). These show states
of the patched OSE build, not a before/after implementation capture or KSP2 pixel parity.
