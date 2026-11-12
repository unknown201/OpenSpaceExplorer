# KSP2 / OSE structural regression capture

The plugin loads eight craft in a dedicated sandbox, teleports them to a 100 km
Kerbin orbit, captures native bodies/joints and applies prescribed loads. It
changes test-vessel forces, not global physics settings. Never use a player
campaign as the seed.

Each craft has sixteen independent native PhysX cases: X/Y/Z force and Y torque,
both signs, repeated twice. Each case has six one-second plateaus. Force levels
are 0, 1, 10, 30, 100, 0 kN; torque levels are 0, 1, 10,000, 100,000, 300,000, 0
kN m. Opposed force pairs use fixed vessel-frame offsets; torque couples have a
1 m lever separation. Two additional cases load the actual live vessel, keeping
KSP2 integration and breakup callbacks active. Every live physical body must be
dynamic with no frozen axes. Input/output tick N describe the same completed
20 ms integration; metadata records enhancement and multi-joint preferences.

OSE imports the same craft, mirrors Unity Z/rotations/forces, matches regenerated
GUIDs by definition and placement, and rejects position errors above 5 cm. Three
variants retain production joints: original mass/inertia, native physical-part
masses, and native masses/body tensors/centres. Nonphysical native parts remain
OSE's own representation and are listed in each specimen. Native drive actuation
is excluded from reported constraint reactions; it is not zero physical load.

## Capture

Use the existing BepInEx 5 loader. Close KSP2, back up and disable other autorun
recorders, and preserve existing files. Prepare an empty KSC sandbox as
`Saves/SinglePlayer/OSE Joint Validation/seed.json` with
`prepare_seed.py SOURCE DESTINATION`. It refuses a nonempty vessel save and an
existing destination.

```powershell
$game = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
dotnet build tools/joint-parity/JointParity.csproj -c Release `
  "-p:GameManagedDir=$game/KSP2_x64_Data/Managed" `
  "-p:BepInExCoreDir=$game/BepInEx/core" -o .cache/joint-parity/build
python tools/joint-parity/prepare_crafts.py `
  --templates "$game/BepInEx/ose-parity" `
  --stock assets/ksp2/data/stockassemblies --out reference/joint-parity-RUN/inputs
```

Templates are local parity-pilot craft; game-derived inputs remain ignored. Copy
inputs to `BepInEx/ose-joint-validation` and the DLL to `BepInEx/plugins`. Launch
through Steam and activate KSP2. Require `done.txt`, no `failed.txt`, and live
dynamics verification for every craft. After exit archive `out/` as `native/`,
remove only the owned plugin/folder, and restore the previous recorder. Verify
resolved absolute paths and hashes before cleanup.

## Replay

```powershell
cargo run --manifest-path tools/joint-parity/ose-replay/Cargo.toml `
  --target-dir .cache/joint-parity/target -- reference/joint-parity-RUN ose-after
python tools/joint-parity/validate_fix.py reference/joint-parity-RUN `
  --summary tools/joint-parity/reference-results.json
```

For before results copy the replay tool into a clean `dd8fdaa` worktree, build
with `--no-default-features`, and write `ose-before` in the same run directory.
This switch supports the earlier API. Preserve both executable hashes/revisions.

Validation requires all 144 cases/ticks, finite readings, identical repeated
native benchmark histories, no false breaks, and exact extra-connection
multiplicities. Native anchor separation must stay below 0.1 m, one tenth of
KSP2's 1 m linear limit. It writes an HTML raw-data index, comparison CSV, JSON
integrity results and hashes. The compact committed summary contains numeric
measurements, not craft assets or reference code.

The earlier extreme ramp developed metres of native constraint separation.
Those first-break values are not valid strength targets. These tests establish
survival under the stated loads and support topology, not exact stiffness,
damping, collision response or full-flight parity. See the
[comparison note](../../docs/reference-notes/joint-stress-validation.md).
