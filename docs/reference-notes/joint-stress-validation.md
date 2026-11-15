# KSP2 / OSE joint stress validation — 2026-11-11

**Result: structural parity fails.** The installed KSP2 was run with eight craft
designs. Its live rigidbodies and joints were captured and subjected to prescribed
loads in isolated native Unity/PhysX scenes. The identical per-part force histories
were replayed through OSE's production `RigidVessel` solver. This is a controlled
structural comparison, not a matched flight campaign.

There were 128 native cases: eight designs × four loading modes × two signs × two
repetitions. There were 384 OSE replays, covering three mass/inertia variants. All
64 pairs of native repetitions had identical break histories and recorded angles.
The evidence contains 1,166,838 native joint samples and 1,811,382 OSE link samples.

With KSP2 physical-part masses and body inertia tensors supplied to OSE, OSE broke
earlier in **122/128 cases**. In the other six cases, all on the two aircraft, OSE
survived until the reference's first break. **No case matched the first-break tick
within one 20 ms step.** This rules out a general claim that the two joint models
behave the same. It supports the report that OSE rockets can be too fragile, but
does not establish how often an ordinary ascent will break.

## Conditions and scope

Reference: KSP2 `0.2.2.0.32913`, Unity `2022.3.5f1`, installed Steam build. Runtime
settings were captured, rather than inferred from C# defaults: solver iterations
25, velocity iterations 1, inertia tensor scaling enabled, global tensor scalar
5, low-mass tensor scalar 50. `UnbreakableJoints` was false. The profile's
`MultiJointsEnabled` preference was false even though `ENABLE_MULTI_JOINT` was true.
**Reinforced connections were still present.** The preference was not changed.

Each craft was loaded through KSP2's OAB and launched, so its physics components,
procedural part tensors, attachment settings and reinforcement graph came from the
running game. Public native joint settings and physical-part mass properties were
copied into independent physics scenes. The copies began at the captured relative
part poses with zero velocities. There were no colliders or gameplay callbacks.
The measured event is the first native joint break, before destruction callbacks
would change the rest of the graph.

Each 50 Hz case had 0.6 s unloaded, a 0.6 s 1 kN pulse, 1.2 s unloaded, then a
5%-per-step load ramp to 1,000,000 kN followed by a 0.6 s hold. X/Y/Z cases used
opposed forces on the root and farthest physical part, with prescribed offsets in
the root frame. The torsion case used opposed couples about root Y, with 1 m force
separation: its numerical force level also equals applied kN m. Both signs were
tested. Each case was built and run twice.

OSE imported the same craft. GUID changes made by the OAB were resolved by part
definition and unique placement in the root frame; maximum placement error was
**0.0742 mm**. Unity Z, rotations, forces and application offsets were converted
to OSE's handedness. Timestamps and sample phases were aligned to completed 20 ms
solver steps. OSE received the recorded forces and offsets without guessing a
thrust, lift or drag distribution.

The three OSE variants retained production joints and break logic:

1. Original catalog mass and inertia, with the captured craft placement.
2. Runtime physical-part masses supplied to OSE.
3. Runtime physical-part masses, full body inertia tensors and centres of mass
   supplied to OSE. OSE's separate wobble layer still uses its own scalar subtree
   inertia approximation; this variant does not replace that algorithm.

KSP2 parts without independent rigidbodies, such as fuel lines and struts, were
listed in the specimens rather than assigned invented native bodies. OSE retains
its own representation of those parts. The tall-stack comparisons contain no
such missing physical bodies.

## Measured comparison

The following table uses variant 3. Counts include both repetitions. “OSE earlier”
includes reference survivors through the complete tested load range. “OSE survives
reference break” is censored at that reference break, not a later measured OSE
failure threshold.

| Design | Physical bodies / native joints | OSE earlier | Within 1 tick | OSE survives reference break |
|---|---:|---:|---:|---:|
| One-tank stack | 3 / 4 | 16/16 | 0/16 | 0/16 |
| Six-tank stack | 8 / 14 | 16/16 | 0/16 | 0/16 |
| Twelve-tank stack | 14 / 26 | 16/16 | 0/16 | 0/16 |
| Radial boosters | 9 / 14 | 16/16 | 0/16 | 0/16 |
| Braced radial boosters | 9 / 15 | 16/16 | 0/16 | 0/16 |
| Wide Kerbal-K2 rocket | 40 / 72 | 16/16 | 0/16 | 0/16 |
| Jet | 12 / 22 | 14/16 | 0/16 | 2/16 |
| RC-Sparrow aircraft | 14 / 26 | 12/16 | 0/16 | 4/16 |

In the original catalog variant OSE broke earlier in 126/128 cases; matching
physical-part mass alone left that count unchanged. Supplying native body inertia
changed it to 122/128. The mismatch therefore survives these calibration changes.

Positive-X bending, first repetition, measured applied force at the first break:

| Stack | OSE with native mass/body inertia | Native KSP2-built joints | Native / OSE load |
|---|---:|---:|---:|
| One tank | 11,146.9 kN | 241,033.3 kN | 21.62× |
| Six tanks | 21,019.0 kN | 188,855.9 kN | 8.98× |
| Twelve tanks | 11,146.9 kN | 279,026.1 kN | 25.03× |

These are deliberately extreme bench loads, **not launch failure thresholds**.
The ramp is dynamic, with 5% resolution per step. The height trend is not
monotonic; the data does not justify a simple “strength decreases with height”
formula. Native reinforcement topology and solver response matter.

In positive torsion, all three native stacks survived the 1,000,000 kN m test
ceiling. OSE broke at 253,084.9 kN m. Thus the reference failure threshold is only
bounded below; it was not measured in those cases.

Local figure: [tall-stack load comparison](../../reference/joint-parity-20261111/tall-stack-comparison.png).

## Why the current model is different

**Breakage load semantics differ.** `rigid.rs::step_turning` compares full subtree
force and, for flexible joints, `stiffness * bend.length()` directly with the
limits. KSP2's `PartOwnerBehavior.CreateJointsBetweenParts` uses strong linear
drives and angular spring drives on limited native joints. Drive actuation is
excluded from PhysX's reported constraint force and breakage force. This is
documented by [NVIDIA under joint actuation](https://nvidia-omniverse.github.io/PhysX/physx/5.1.1/docs/Joints.html#actuation)
and is consistent with the native zero/small reaction readings under nonzero
prescribed loads in these captures. A native `currentForce` of zero must not be
interpreted as zero physical load, or directly compared to OSE's full load.

**The graph is different.** KSP2's `Module_ReinforcedConnection.SecureReinforcedStrut`
creates additional size-2 stack connections with break limits set to `float.MaxValue`.
Its target can be an ancestor, so this is not just a larger spring on the existing
parent link. For example, the six-tank stack had 14 native joints across 12 body
pairs, including seven effectively unbreakable reinforced joints. OSE builds
seven parent links. The native twelve-tank stack had 26 joints across 24 pairs;
OSE builds thirteen parent links. Copying the authored break-force constant
cannot reproduce that graph.

**The dynamics are different.** KSP2 applies runtime inertia scaling and solves
coupled body/joint motion. OSE uses fixed reduced-coordinate links, an artificial
attitude-hold force distribution, and a separate angular bend integrator. Its
wobble inertia approximation uses subtree boxes even when detailed body tensors
are supplied. Native mass/body-inertia calibration therefore does not make the
models equivalent.

These are source-backed differences supported by the observed failures. The
benchmark does not separately attribute a percentage of the mismatch to each
cause. No production physics constants or break rules were changed in this task.
There is no “OSE fixed” result to claim.

## Reproduction and evidence

Harness and exact workflow: [tools/joint-parity/README.md](../../tools/joint-parity/README.md).

```powershell
cargo run --manifest-path tools/joint-parity/ose-replay/Cargo.toml `
  --target-dir target -- reference/joint-parity-20261111
python tools/joint-parity/analyze.py reference/joint-parity-20261111
python tools/joint-parity/plot.py reference/joint-parity-20261111
```

Local artifacts below are ignored game-derived measurements, not included in a
clean checkout:

- [Raw-data index](../../reference/joint-parity-20261111/raw-data.html): native
  joint CSVs, exact applied-input CSVs, specimen settings, and each OSE replay.
- [Comparison CSV](../../reference/joint-parity-20261111/comparison.csv) and
  [comparison JSON](../../reference/joint-parity-20261111/comparison.json): all
  384 results, censored outcomes, pulse peaks, repetition checks and native hashes.
- [Runtime settings](../../reference/joint-parity-20261111/native/settings.json),
  [completion log](../../reference/joint-parity-20261111/native/steps.log), and
  [provenance](../../reference/joint-parity-20261111/provenance.json).

The plugin built with zero warnings/errors. The Rust replay built and completed
all 384 cases. Analysis required all eight designs and sixteen native cases per
design, no idle/pulse breaks, finite measured angles and OSE forces, and identical
native repeated break histories. All checks passed. Passing these integrity
checks validates the measurements, not physics parity. Source hashes were checked
again after the run and had not changed. The temporary plugin was removed and the
existing pilot's original run file was restored with its SHA-256 verified.

## Remaining limits

This is sufficient to reject structural parity under matched prescribed loads.
It does not validate full aircraft aerodynamics, control response, ordinary launch
trajectories, max-Q, collision damage, landing loads, fuel depletion, breakup
callback cascades, or the other multi-joint preference setting. Native scenes
copy public physics settings, not hidden accumulated solver impulses or complete
gameplay state. Small unloaded/pulse deflection readings can include initial
constraint settling, especially on long stacks; they are preserved as raw data
and are not used here as proof of matched low-load stiffness or damping.
