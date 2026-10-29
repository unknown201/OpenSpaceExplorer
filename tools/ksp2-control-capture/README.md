# KSP2 control capture

This measurement aid was compiled and run in KSP2 0.2.2.0.32913 in a separate
temporary sandbox campaign, with the user's approval. F8 is passive telemetry.
F9 is an explicit fixture: it prepares a 100 km Kerbin orbit, disables SAS and cuts
throttle input, waits two seconds, then supplies pitch/release/reversal/roll/yaw
through the stock FlightInputHandler override API for 12 seconds. It stops overrides
and recording automatically. It never changes physics parameters or writes a save.
The fixture does not ignite an engine; use an unignited craft.

## Build

Use the installed game's `KSP2_x64_Data/Managed` directory and the official BepInEx 5
core directory. No game or BepInEx binaries are committed here.

```powershell
dotnet build tools/ksp2-control-capture/ControlCapture.csproj -c Release `
  '-p:GameManagedDir=C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2\KSP2_x64_Data\Managed' `
  '-p:BepInExCoreDir=D:\Projects\OpenSpaceExplorer\.cache\ksp2-trace-loader\BepInEx\core'
```

Build output: `bin/Release/netstandard2.1/OSE.ControlCapture.dll`.
It builds against the installed assemblies with zero warnings/errors.

The cached loader is the official [BepInEx 5.4.23.2 release](https://github.com/BepInEx/BepInEx/releases/tag/v5.4.23.2),
`BepInEx_win_x64_5.4.23.2.zip`, SHA-256
`f752ce4e838f4c305b9da1404b6745f2cff23b8bfd494f79f0c84d0a01f59b46`.
Installation follows the [BepInEx documentation](https://docs.bepinex.dev/v5.4.21/articles/user_guide/installation/index.html).

## Installation and cleanup boundary

The user approved temporary installation, reference captures and cleanup. The
reference installation had no BepInEx loader. Preserve an existing loader in future
runs and obtain authorization for installing/running the added software.

The temporary additions are:

- `winhttp.dll`, `doorstop_config.ini`, `.doorstop_version` in the KSP2 game directory;
- `BepInEx/core/` from the official archive;
- `BepInEx/plugins/OSE.ControlCapture.dll` from this build.

Record the exact added paths/hashes before launching. The loader will also create
its own config/cache/logs under `BepInEx/`. After capturing, close KSP2, preserve
captures and logs in the workspace cache, and remove only the added files and
owned generated directories. Verify resolved absolute cleanup targets remain within
the named game installation. Preserve any unexpected/new user files.

## Capture

Load the agreed reference craft and test conditions. F8 starts/stops capture; a
vessel change stops it automatically. Outputs are in `BepInEx/ose-control-traces/`:

- timestamped CSV: simulation time, effective manual input (precision + trim),
  applied SAS commands, throttle, quaternion, angular velocity, mass, translation,
  SAS enable and absolute UT;
- companion metadata: source executable assembly version, vessel identity,
  main-body motion frame, Unity handedness and post-FixedUpdate sample phase;
- companion surfaces CSV: actual runtime moving/fixed lift coefficients, actuator
  speed/range, pressure offsets and hinge pivots.

The logger preserves the game's values and coordinate frame. Before comparing with
OSE, explicitly convert Unity's handedness and the world/control frame, and match
initial state, craft configuration, environment, applied inputs and sample phase.
A floating physics-origin frame must not be confused with this main-body frame.
The recorder does not claim those conditions already match. Two passive captures
and two 600-sample orbit/pulse captures were obtained on 2026-10-29 UTC. The first
replay fails and exposes missing occupied-seat mass, runtime inertia scaling and
procedural geometry. A pass must not be reported from this baseline. Use `tools/compare_control_traces.py` only after this
preparation; it rejects different input sequences/timestamps and reports angle,
angular-rate and mass errors.

## Recorded evidence and cleanup

`fixtures/jumping-flea.csv` is the second actual orbit/pulse capture, with its
coordinate metadata and companion `jumping-flea.parts.csv`. Only the generated part
GUIDs were removed from the companion. `aeris-k2.surfaces.csv` contains the earlier
stock jet's runtime areas/offsets. These are numerical measurements; no loader,
game binaries, reference source code or user save is redistributed.

Reproduce the **failing** baseline from repository root:

```powershell
cargo run -q -p ose-sim --example control_trace -- 'Jumping Flea' tools/ksp2-control-capture/fixtures/jumping-flea.csv > .cache/jumping-flea-ose.csv
python -B tools/compare_control_traces.py --reference-unity tools/ksp2-control-capture/fixtures/jumping-flea.csv .cache/jumping-flea-ose.csv
```

The comparison exits 1. `fixtures/jumping-flea-baseline.json` records the result at
c1c0e23: initial poses/velocities and the recorded input sequence are reused;
catalog masses/tensors are left untouched. Actual engines were unignited, RCS/SAS
off. No controller tuning, fitted pose/time shift or compensating inertia/mass
multiplier is applied. This is a diagnostic mismatch, not a passed parity test.

The per-part data confirms 0.095 t occupied crew mass and procedural fin mass/COM
changes. The three fixed parts' principal inertias match the imported unit tensors
after RigidbodyBehavior's `GLOBAL_TENSOR_SCALAR / mass` scaling. Procedural fin
shapes differ from their prefab defaults. PartOwnerComponent's MOI also translates
each tensor from the part origin, whereas physical compound inertia uses each part
COM; those quantities must not be silently substituted in SAS autotuning.

After the captures, KSP2 was closed. The 22 verified installed files, owned loader
cache/config/log directories, and the isolated `OSE Control Validation 2026102`
campaign were removed. All captures/logs and a copy of the temporary campaign are
archived under `.cache/ksp2-runtime-captures-20261029` in the original workspace.
The existing Default campaign's files hashed identically before and after cleanup.
The subsequent mass/inertia implementation resolves that mass-only experiment's
jet regression by retaining separate fixed/moving force locations and the reference
force-before-deflection order. It includes crew mass, measured instance geometry,
reference inertia scaling, PhysX-style aggregate rotation, and separate SAS MOI.
`jumping-flea-after-mass-inertia.json` records the improved but still failing replay:
0 t mass error, 8.147963-degree maximum angle error and 0.2222502 rad/s maximum rate
error. Thresholds are unchanged; this is not a whole-game parity pass.
