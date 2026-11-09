# EVA pod exit compared with KSP2

Worked example of the project's [issue-fixing workflow](../ISSUE-FIXING.md).

2026-11-09. Compared the local KSP2 installation's EVA implementation and authored pod
transforms with OSE, and observed a live KSP2 pod exit above Kerbin's atmosphere.
The local visual comparison is [side-by-side.html](../../.cache/eva-parity/side-by-side.html).

| Action | KSP2 reference | OSE before | OSE fixed |
|---|---|---|---|
| Exit a pod in space | Start attached to the pod's climb trigger | Spawn 2.5 m sideways from the vessel's mass centre | Hold the occupied part's authored hatch frame; export covers 21 crewed parts |
| Wait at the hatch | Follow the supporting part | Independent orbit, allowing relative drift | Follow position and orientation without consuming fuel |
| Let go / grab | Ladder interaction, separate from boarding | F boarded immediately | F releases or grabs a nearby hatch; Space also releases |
| Release velocity | Carry the supporting rigid body's motion | Copy only centre-of-mass velocity | Inherit translation plus angular velocity crossed with hatch offset; no artificial separation impulse |
| Jetpack | Explicitly enabled with RCS control | Always enabled; throttle could act as a main engine | R enables the pack; translation uses pack axes; attachment suppresses thrust and effects |
| Space orientation | Preserve orientation relative to the ladder / free flight | Controls and rendering forced a planetary upright pose | Preserve inertial orientation and use the authored ladder idle animation |
| Board | Interact at an airlock | Measure reach from vessel centre | B or Board checks hatch reach and a free seat in that part |
| Nearby spacecraft | Remain visible on EVA | Placeholder probe replaced the pod | Known craft documents render their parts at the spacecraft's actual position, mass centre and attitude |
| Save/reload | Preserve current ladder reference | No ladder state | Store stable supporting-vessel id and part index; resolve after restoring all vessels |

Reference sources in `reference/ksp2/Assembly-CSharp/`:

- `KSP.Sim.impl/KerbalBehavior.cs`, `ActivateBestLocomotionHandler`: newly created
  EVAs try to climb the part identified by `EVAStartLocationId` without tweening.
- `KSP.Sim.impl/ClimbKerbalCharacterStateHandler.cs`: climb-trigger frame, ladder
  orientation and stop-climb separation; high-altitude release retains the ladder frame.
- `KSP.Sim.impl/JetpackKerbalCharacterStateHandler.cs`: jetpack is inactive while climbing.
- `KSP.Sim.impl/KerbalComponent.cs`: saves the current ladder part id.
- `tools/export_eva_hatches.py`: reads installed prefab airlock-climb transforms;
  `tools/export_kerbal_anim.py`: exports `kerbal_ladder_idle` with the existing EVA clips.

Validation: all 166 simulation library tests and 112 client library tests passed. Seven
EVA simulation tests cover surface walking, flight, attachment, rotation inheritance,
release/regrab, pack gating and loss of the supporting vessel. Server tests cover
JSON save/restore with forward references and a real WebSocket exit/release/grab/board
sequence. `e2e/scripts/check-eva-space.mjs` passed in Chromium WebGPU through real UI
inputs, including Space release, R toggling and B boarding, with no browser errors.

The screenshots compare behavior, not pixel parity: KSP2 was at about 123 km on a
suborbital trajectory and OSE at 100 km in a circular orbit; camera, lighting and craft
configuration differ. This change targets space exits. Surface exits retain the existing
stand-beside-vessel path. Hatch obstruction detection and external-seat handling remain separate work.
The 1.5 m grab radius, existing climb span, and 0.2 m release clearance are approximations.

The original local build used protocol 44. Integration onto current main keeps protocol
64 and reuses its existing `OnLadder`, `own_top`, complete jetpack settings, nearby-craft
rendering, and ladder animations. It adds authored hatch frames, point-velocity inheritance,
pack gating, explicit boarding controls, and stable save references to those systems rather
than introducing a second attachment mechanism. Existing ladder climbing remains supported.
The test counts above record the original local comparison; integration is tested again
against current main before release.

Reproduce against an isolated local server serving `target/eva-web` and `assets/web`:

```powershell
cargo test -p ose-sim --lib
cargo test -p ose-client --lib
cargo test -p ose-server --bin ose-server save::tests
cargo test -p ose-server --test ws_session orbital_eva
# Build with trunk from crates/ose-client, then serve on 127.0.0.1:7799.
node e2e/scripts/check-eva-space.mjs
```
