# KSP2 control capture

This measurement aid was compiled and run in KSP2 0.2.2.0.32913 in a separate
temporary sandbox campaign, with the user's approval. F8 is passive telemetry.
F9 is an explicit fixture: it prepares a 100 km Kerbin orbit, disables SAS and cuts
throttle input, waits two seconds, then supplies pitch/release/reversal/roll/yaw
through the stock FlightInputHandler override API for 12 seconds. It stops overrides
and recording automatically. It never changes physics parameters or writes a save.
The fixture does not ignite an engine; use an unignited craft. Version 0.3 records
live rigid-body intervals and supplies the stock one-shot overrides on every
physics tick. Earlier 0.1 measurements remain immutable diagnostic fixtures.

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
  main-body motion frame, Unity handedness and explicit live start/end phases;
- companion surfaces CSV: actual runtime moving/fixed lift coefficients, actuator
  speed/range, pressure offsets and hinge pivots.

The primary pose/rate are sampled before physics from live Rigidbodies, rather
than cached PartOwnerComponent statistics. `end_q*`/`end_w*` are sampled after
`WaitForFixedUpdate`; applied inputs describe that interval. `dt`, `fixed_count`
and `unity_frame` expose dropped ticks and multiple physics steps per render.
`cached_w*`, requested torque, wheel input and pending/accepted power distinguish
telemetry lag from actual module/force latency. `compare_control_traces.py`
rejects noncontiguous intervals or discontinuous live end/start rates.

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

## Live-interval regression

`fixtures/jumping-flea-interval.csv` is a separate 600-interval executable capture
with uninterrupted requested holds and continuous live body states. The old
capture and reports above are retained unchanged. The new fixture's manifest
records hashes, baseline commit `213248c` and successful cleanup: temporary loader
and campaign removed, all 75 Default files unchanged. Only generated GUID fields
were removed from metadata/part companions.

```powershell
cargo test -p ose-sim --test control_runtime
cargo run -q -p ose-sim --example control_trace -- 'Jumping Flea' tools/ksp2-control-capture/fixtures/jumping-flea-interval.csv > .cache/jumping-flea-interval-ose.csv
python -B tools/compare_control_traces.py --reference-unity tools/ksp2-control-capture/fixtures/jumping-flea-interval.csv .cache/jumping-flea-interval-ose.csv
```

The corrected wheel stages and PhysX pose order pass the unchanged thresholds on
this fixture: maximum attitude error 0.145481 degrees, angular-rate error
0.0024561 rad/s and mass error 0 t. The same capture at `213248c` fails at
12.770470 degrees / 0.6142401 rad/s. No input/time/pose offset, tensor multiplier
or controller gain was fitted. This validates SAS-off wheel pulses in this craft;
other craft, SAS/RCS, brownouts and joint/contact behavior still need captures.

## RCS runtime evidence

F10 prepares the same orbit, enables RCS, disables every reaction wheel and SAS,
then pulses translation X, Y, Z and reverse X. It records 600 live intervals plus
per-nozzle forces/positions, the shared pending propellant request, its delivered
fraction and connected resource remaining. F9 remains the wheel fixture.

`gumball-rcs.csv` and `gumball-rcs-repeat.csv` are actual captures from a single
Gumball capsule with one occupied seat. The repeat preserves the first run's
remaining propellant and previous request-delivery state. Their fixture JSONs use
the independently measured part mass/shape; a single dynamic body's inertia is
unscaled with that body's mass, rather than the multi-body scalar of 5.

```powershell
cargo run -q -p ose-sim --example control_trace -- tools/ksp2-control-capture/fixtures/gumball-rcs.craft.json tools/ksp2-control-capture/fixtures/gumball-rcs.csv > .cache/gumball-rcs-ose.csv
python -B tools/compare_control_traces.py --reference-unity tools/ksp2-control-capture/fixtures/gumball-rcs.csv .cache/gumball-rcs-ose.csv
```

This replay **passed** once the timing below was modelled. It was **failing** before
that change: 4.22574 degrees and 0.0150627 rad/s. Nothing is fitted; every rule
is read from the captures' own frame, request and end-of-interval columns.

- **Render-frame input.** `Module_RCS` reads translation in Update. A physics tick
  sees the input as of the end of the previous render frame (`unity_frame`).
  `control_trace` feeds the simulation that input and prints the applied input
  unchanged.
- **Retained delivery.** A module scales its command by the delivery fraction its
  previous request received. The request posted on a tick is filled after the module
  has computed that tick's thrust. So a module's first request yields nothing, and an
  idle module keeps its last delivery. The repeat capture starts with delivery 1,
  which `control_trace` reads from the first `.rcs.csv` row.
- **Queued force.** RigidbodyBehavior applies the computed thrust on the following tick.
- **Rigidbody re-basing.** The start of one interval sits 1.4 or 2.9 mm from the end
  of the previous one along the orbital radius. That movement happens outside
  physics. The comparator removes the accumulated discontinuity: 0.579 m over the run.
- **Cached vessel mass.** `mass_t` is refreshed once per render frame and shows the
  tank state from the tick before that frame's first physics tick. The comparator
  reads the candidate's mass at that tick.

| Fixture | Angle | Rate | Position | Mass |
|---|---|---|---|---|
| `gumball-rcs` | 0.00037° | 1.8e-6 rad/s | 5.8 mm | 8.9e-10 t |
| `gumball-rcs-repeat` | 0.0011° | 4.9e-6 rad/s | 5.6 mm | 3.1e-9 t |

The results are in `gumball-rcs-timing.json` and `gumball-rcs-repeat-timing.json`.

Both captures verify that two firing nozzles consume one module request per tick:
`Module_RCS.CalculateThrust` repeatedly overwrites the same handle, and the final
positive nozzle command survives. Source definitions now use that command in
transform order; legacy definitions keep their summed demand. Unequal-command and
reversed-order tests distinguish this rule from either summing or taking a maximum.

KSP2 was closed and all 22 added loader/recorder files, owned generated directories
and the `OSE RCS Validation 1029` campaign were removed. All 75 existing Default
campaign files hash identically. Raw captures, logs, install manifest and the
temporary campaign remain archived at `.cache/rcs-runtime-20261029` in this worktree.
The committed manifest hashes primary/nozzle CSVs unchanged; only generated GUID
fields were removed from companions. No game binaries or save files are committed.

## Repeat wheel and SAS captures (2026-10-31)

Recorder 0.4.0 adds F11: the F9 orbit setup, then pitch 1 for t in [1, 2), yaw 1
for [1.5, 2.5), and Stability Assist engaged at t = 3 s. Both captures below come
from one stock Jumping Flea with one occupied seat, in a temporary sandbox campaign.

- `jumping-flea-repeat.csv` repeats the F9 wheel pulse of `jumping-flea-interval.csv`:
  same craft, same inputs, same mass and inertia columns.
- `jumping-flea-sas.csv` was pressed straight after the repeat, so it starts
  from that run's ~2.9 rad/s tumble. SAS then has to stop it.

`control_trace` now places a fixture's crew in the craft's only crewed part, keeps
the vessel in physics (OSE would otherwise put a settled craft on rails), and
engages SAS as `Attitude::Fixed` on the facing at the sample where `sas` turns on.

```powershell
cargo run -q -p ose-sim --example control_trace -- 'Jumping Flea' tools/ksp2-control-capture/fixtures/jumping-flea-repeat.csv > .cache/jf-repeat-ose.csv
python -B tools/compare_control_traces.py --reference-unity tools/ksp2-control-capture/fixtures/jumping-flea-repeat.csv .cache/jf-repeat-ose.csv
python -B tools/ksp2_repeatability.py tools/ksp2-control-capture/fixtures/jumping-flea-interval.csv tools/ksp2-control-capture/fixtures/jumping-flea-repeat.csv
```

**KSP2 is not repeatable to the comparator's thresholds.** In body frame, the two
KSP2 runs of the same F9 fixture differ by up to 0.0659 rad/s, against a
0.01 rad/s threshold. They agree to 0.0075 rad/s through the first six seconds.
The runs part ways after the first pitch release: in one, |omega| rises by
0.0013 rad/s while no input is applied; in the other, it falls by 0.0032 rad/s.
The roll and yaw pulses at about 3 rad/s then amplify the difference.

OSE replays `jumping-flea-interval` to 0.0025 rad/s. On `jumping-flea-repeat`
it fails at 3.49 degrees and 0.0917 rad/s, which is the same order as KSP2's
own spread (`jumping-flea-repeatability.json`). The SAS replay fails the
per-sample test at 10.7 degrees and 0.145 rad/s, but its behaviour matches:
- Both stop the 2.9 rad/s tumble within about 2 s of SAS engaging.
- Both then hold with the same Â±0.02 rad/s limit cycle.

Nothing was fitted. A single capture cannot pin the simulation tighter than the
game reproduces itself. Thresholds for long, fast-spinning fixtures should come
from repeat captures, not from one trace.

KSP2 was closed, and all 22 added loader and recorder files plus the owned BepInEx
directories were removed. The `OSE Capture 1031` campaign was deleted, and all 87
existing save files hash identically. Raw captures, the loader log and a copy of
the temporary campaign are archived in `.cache/ksp2-capture-20261031` under the
main checkout. Only the `vessel=` and part `guid` fields were removed from the
committed copies.
