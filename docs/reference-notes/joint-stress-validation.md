# Joint stress and premature breakup: KSP2 validation

OSE treated spring-drive loads as breakable constraint reactions and omitted
KSP2's reinforced connection graph. The fix removes those false breakup signals,
adds the coupled supports and preserves authored aircraft support targets.

## Before / after

Reference: KSP2 **0.2.2.0.32913**, Unity **2022.3.5f1**, captured from the local
Steam installation. Enhanced joints were enabled in **Heaviest** mode;
multi-joints and unbreakable joints were disabled. OSE before is `dd8fdaa` on main.

| Condition | KSP2 reference | OSE before | OSE fixed |
|---|---|---|---|
| Prescribed loads on eight designs | 128/128 benchmark cases survive | False breaks in torque probes | 128/128 survive |
| Torque probes on free, live vessels in 100 km orbit | 16/16 survive | 16/16 falsely break | 16/16 survive |
| Additional connection graph | Authored targets plus enhanced supports | Mostly missing | Exact connection multiplicities match all eight specimens |
| Wing/gear support target | Authored grandparent overrides enhanced mode | Discarded with unmapped module data | Preserved in typed data and respected by the solver |
| Locked constraints / angular stops | Breakable hard reactions, excluding drives | Full force and spring torque treated as failure loads | Hard reactions remain breakable; regressions cover both |
| Stock booster thrust / roll reference | Braced and unbraced sides stay nearly rigid | Unbraced/roll check ignored | Flex-bound check enabled and passing |

Each build replayed all 144 prescribed histories in three variants: original
mass/inertia, native physical-part masses, and native masses plus full body
inertia tensors/centres. **Before: 48 false breaks per variant. After: zero per
variant**, across 432 replays per build.

| Design | Cases | Before false breaks | After false breaks |
|---|---:|---:|---:|
| One-tank stack | 18 | 6 | 0 |
| Six-tank stack | 18 | 6 | 0 |
| Twelve-tank stack | 18 | 6 | 0 |
| Radial boosters | 18 | 6 | 0 |
| Braced radial boosters | 18 | 6 | 0 |
| Jet | 18 | 6 | 0 |
| Wide Kerbal-K2 | 18 | 6 | 0 |
| RC-Sparrow | 18 | 6 | 0 |

The torque probes deliberately exceed the old model's thresholds. They are not
ordinary launch-load estimates. These results establish survival and graph
topology under the stated conditions, not exact stiffness/damping or flight parity.

## Cause and implementation

`rigid.rs::step_turning` formerly compared full subtree force and `k * |bend|`
with the break limits. KSP2 holds translation and supplies angular spring forces
with native joint drives. PhysX excludes drive actuation from reported constraint
force and breakage; see [NVIDIA's actuation documentation](https://nvidia-omniverse.github.io/PhysX/physx/5.1.1/docs/Joints.html#actuation).
A small native reaction reading under a large drive load does not mean zero
physical load.

OSE retains full transmitted loads for readouts but stores separate hard
reactions for breakup and the danger readout. Rigid links carry the full reaction.
Flexible links carry a hard angular reaction at their stop; translation remains
held by the existing reduced-coordinate tree, representing the near-infinite
native translation drive. An impulse removes outward motion at 177 degrees.
This spherical stop approximates the native twist/swing limits.

KSP2's `PartOwnerComponent.InitializeExtraJoints` and
`Module_ReinforcedConnection` add size-2 supports with effectively unbreakable
limits. Eligible parts normally connect to the first heaviest physical part;
decouplers and clamps do not receive automatic connections. Authored wing/gear
supports instead use their ancestor/root/heaviest target, even with enhanced
joints disabled.

The importer now preserves that target in typed data through slim-catalog
export. All 25 authored stock modules inspected locally use GrandParent, which
is the backward-compatible fallback when an older slim catalog retains only the
module name. Root/Heaviest overrides are also parsed and tested.

The existing coupled flex solver handles these extra supports with size-2 angular
stiffness and its finite translation penalty. Parent links remain flexible.
Supports rebuild for initial wet mass/removal and release when the tree splits.
Break strengths and the global unbreakable-joints setting are not increased.

## Capture and integrity

The plugin loaded each real craft through the OAB, launched it, teleported it to
a 100 km Kerbin orbit, disabled SAS and captured actual native bodies and joints.
Settings include runtime tensors, centres, drives, limits, iterations and profile.

Each independent 50 Hz case has one-second plateaus: 0, 1, 10, 30, 100, 0 kN for
translation, or 0, 1, 10,000, 100,000, 300,000, 0 kN m for torque. Both signs are
tested twice: sixteen cases per craft. Two more cases apply the torque sequence
directly to the live vessel, retaining the game's integration and breakup code.
Every live body is checked nonkinematic with no frozen axes; every vessel is
recorded as Orbiting.

Input/output tick N describe the same completed 20 ms integration. OSE replays
the recorded forces/offsets after mirroring Unity's frame. Regenerated GUIDs are
matched by definition and unique placement, rejecting errors over 5 cm. Parts
without independent native bodies remain OSE's representation and are identified.

Validation requires complete cases/ticks, finite values, identical repeated
native histories, matching connection multiplicities, and no false breaks. It
rejects anchor separation above 0.1 m, one tenth of the native 1 m linear limit.
An earlier extreme ramp developed metres of native constraint separation; its
reported **9×/25× first-break ratios are not clean strength comparisons and are
not tuning targets for this fix**.

Exact commands are in the [capture/replay workflow](../../tools/joint-parity/README.md).
[Committed measurements and hashes](../../tools/joint-parity/reference-results.json)
provide the compact audit record. Full local, ignored evidence is in the
[raw-data index](../../reference/joint-parity-fixed-20261111/raw-data.html),
[comparison CSV](../../reference/joint-parity-fixed-20261111/fix-validation.csv),
and [native completion log](../../reference/joint-parity-fixed-20261111/native/steps.log).
The temporary plugin was removed and the prior recorder's run file restored
with its SHA-256 verified.

## Verification and remaining limits

The native plugin built with zero warnings/errors. Native capture, replay and
graph checks passed. Regressions cover loads above rated drive thresholds, finite
parallel stiffness, authored target retention, rigid failure, angular-stop
impulses, staging/debris mass, normal K1/K2 flight and the recorded booster roll.

Local checks passed: workspace tests (696 passed), final affected-package tests
(431 passed), Clippy with warnings denied, workspace and replay formatting,
the WASM client check and Python tests (36 passed).

The solver still uses a reduced-coordinate tree and linearized flex. The finite
support penalty and spherical angular stop do not reproduce every native motion
or extreme-load instability. Supplying native body tensors does not replace the
flex layer's scalar subtree inertia. Live probes are in vacuum; uniform free fall
does not load joints and gravity gradients are not replayed. Exact stiffness,
damping, high-deformation thresholds, collision cascades, other enhancement
profiles and complete aircraft/launch trajectories remain outside this claim.
