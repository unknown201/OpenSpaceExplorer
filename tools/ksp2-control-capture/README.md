# Passive KSP2 control capture

This is a measurement aid, not a game patch. The recorder reads the active vessel
and writes local CSV files. It does not inject inputs, teleport a vessel, change
physics, or write a game save. It is compiled but has not yet been run in KSP2.

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

Do not install or launch the added software through Windows automation until the
pending user confirmation has been answered. The reference installation had no
BepInEx loader when inspected. Recheck before installation and preserve an existing
loader if that changes.

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
The recorder does not claim those conditions already match, and no reference
capture is currently present. Use `tools/compare_control_traces.py` only after this
preparation; it rejects different input sequences/timestamps and reports angle,
angular-rate and mass errors.
