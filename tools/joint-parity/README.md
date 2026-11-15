# Native KSP2 / OSE structural comparison

This tool launches real craft through KSP2's OAB, reads their actual rigidbodies and
all native configurable joints (including reinforcement), then copies the physics
components into isolated Unity physics scenes. Each specimen receives a 50 Hz
prescribed load history: 0.6 s unloaded, 0.6 s at 1 kN, 1.2 s unloaded, then a 5%
per-tick ramp up to 1,000,000 kN and a 0.6 s hold. Separate cases apply X/Y/Z
forces and a Y-axis torque couple in both signs, repeated twice. The torsion case
uses two opposed forces 1 m apart, so the numerical load also equals kN m.

The paired forces use fixed vessel-frame offsets relative to each part origin;
the torsion couples have zero resultant force. Every applied force and offset is
recorded after conversion to the root frame. The native scene is stopped at the
first broken joint, before gameplay callbacks would alter the remaining graph.
There are no colliders, atmosphere, gravity, SAS, engine/fuel or damage callbacks in
the isolated benchmark. This tests the structural solver; it is not a matched
ascent, max-Q, runway or landing comparison.

`ose-replay` imports the identical craft, matches part definition/rest placement
when the OAB has regenerated GUIDs (rejecting errors above 5 cm), and replays those
forces through the production `RigidVessel` API. It mirrors Unity Z for positions,
rotations, forces and offsets. It runs three variants: original OSE mass/inertia,
native physical-part masses, and native physical-part masses plus full inertia
tensors and centres of mass. Nonphysical KSP2 parts remain OSE's own bodies; the
specimen lists them explicitly. OSE's graph, anchors, springs, break rules and
wobble inertia approximation are not replaced by the reference.

## Prepare and run

Use the already installed BepInEx 5 loader. Do not replace an existing loader or
plugin. Close KSP2 before installing/removing this test plugin. Back up and disable
any other autorunning measurement plugin's run file. Use a dedicated empty KSC
sandbox seed, copied by `prepare_seed.py`, to `Saves/SinglePlayer/OSE Joint
Validation/seed.json`; existing player campaigns need not be loaded or modified.
An empty seed save is local game data and must not be committed.

```powershell
$game = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
dotnet build tools/joint-parity/JointParity.csproj -c Release `
  "-p:GameManagedDir=$game/KSP2_x64_Data/Managed" `
  "-p:BepInExCoreDir=$game/BepInEx/core" -o .cache/joint-parity/build
python tools/joint-parity/prepare_crafts.py `
  --templates "$game/BepInEx/ose-parity" `
  --stock assets/ksp2/data/stockassemblies --out reference/joint-parity-RUN/inputs
```

Copy the inputs directory to `BepInEx/ose-joint-validation/` and the built
`OSE.JointParity.dll` to `BepInEx/plugins/`. The craft templates are local reference
craft from the existing parity pilot; the generator does not commit game-derived
craft. Activate the game window after launching through Steam. The plugin writes
`out/steps.log`, `settings.json`, each craft's `*-specimen.json`, `*-inputs.csv`,
`*-native.csv`, and a screenshot. Require `done.txt` and absence of `failed.txt`.
Archive `out/` as the run's `native/` directory after the game exits.

```powershell
cargo run --manifest-path tools/joint-parity/ose-replay/Cargo.toml `
  --target-dir target -- reference/joint-parity-RUN
python tools/joint-parity/analyze.py reference/joint-parity-RUN
python tools/joint-parity/plot.py reference/joint-parity-RUN
```

Plotting requires matplotlib. The analyzer uses the Python standard library. It
rejects missing craft/cases, early idle/pulse breaks, nonfinite loads/angles and
inconsistent repeated native break histories. It reports all mismatches, including
cases where OSE survives the native first break. A native survivor is a lower
bound, not a measured failure threshold. The figure uses only measured failures.
The 5% ramp is dynamic, not a quasi-static strength measurement.

Native `currentForce/currentTorque` exclude joint-drive actuation. Do not compare
these directly to OSE's full subtree load or `k * bend`, or interpret zero reported
reaction as zero physical stress. Record break events and deflection alongside the
reaction values. NVIDIA documents this distinction under
[joint actuation](https://nvidia-omniverse.github.io/PhysX/physx/5.1.1/docs/Joints.html#actuation).

After the run, preserve the artifacts, verify the test plugin's hash, remove only
that plugin and its owned input/output folder, and restore the other recorder's
original run file. Check resolved absolute paths before recursive moves/removal.
Record source/executable hashes and the OSE working-tree revision with the results.
