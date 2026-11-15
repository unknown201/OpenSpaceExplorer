# Live splashdown comparison

This benchmark launches a fresh real Mk1 pod for each of eight controlled cases:
gentle splashdown, a zero-speed release, a 25° bank, a 5 m/s horizontal release,
and vertical/horizontal impacts on both sides of the native destruction threshold.
The 45-second cases measure equilibrium, bobbing and righting. OSE imports the
identical local OAB craft and calls production propulsion, aerodynamic, surface
and flight APIs at 50 Hz. No native force or output trajectory is injected into OSE.

The native recorder uses real KSP2 `PartBehavior`, water detection, buoyancy,
drag and destruction callbacks. It teleports above the ocean, then sets only the
initial rigidbody pose and velocity. It records UT, survival, origin/CoM altitude,
surface velocity, attitude, buoyancy fraction/force and rigidbody damping.
Screenshots accompany each completed case. Setup differences and measurement
results must be reported; a passing unit test does not establish live parity.

## Run

Use the installed game and its existing BepInEx loader. `run.ps1` copies the
small runtime/loader files into an isolated local launch, with junctions to the
installed data and Mono runtime. It installs only the water plugin in that launch,
leaves installed plugins/markers alone, and uses a dedicated empty **OSE Water
Validation** sandbox seed. Do not run two instances of this benchmark against the
same validation campaign. Game-derived fixtures, binaries and captures stay ignored.

```powershell
./tools/water-parity/run.ps1
```

Pass `-GameDir`, `-SaveDir`, `-Seed`, `-RunDir` and `-IsolatedDir` for other local
paths. The seed must be an empty sandbox JSON. Existing or changed captures/seeds
are refused rather than overwritten. Activate the game window if startup pauses.
The recorder corrects invalid tiny window dimensions, waits for the actual game
states, uses universe time for holds, and quits after completion or setup failure.
Expect several minutes. Require `out/done.txt` and absence of `out/failed.txt`.

Copy the isolated `BepInEx/ose-water-validation/out` to the run's `native/`
directory after exit, preserving unsuccessful attempts separately. Copy its
`craft.json` to the run root. Never merge files from different attempts.

```powershell
cargo run --offline -p ose-sim --example water_probe -- .cache/water-parity
.venv/Scripts/python.exe -B -m unittest discover -s tools/water-parity -p test_analyze.py
.venv/Scripts/python.exe tools/water-parity/analyze.py .cache/water-parity
.venv/Scripts/python.exe tools/water-parity/provenance.py .cache/water-parity `
  --game 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
```

`comparison.html` contains standalone trajectory plots and every comparison check.
`comparison.json` is machine-readable. Exit **0** means all fixed fixture checks
pass, **1** means measured gaps, and **2** means incomplete/unmatched evidence.
The analyzer rejects missing cases, water entries, completion, trace tails, sample
gaps, nonfinite data, and mismatched part mass/displacement. It does not align or
fit time, height or attitude. Limits are declared in `analyze.py` before capture.

The benchmark does not establish arbitrary compound-hull behavior, wave or
planing fidelity, animated drag-cube weighting, flooding, Kerbal swimming,
save/load continuity of drag history, multiplayer presentation or splash effects.
Passing capsule fixtures cannot establish universal water-physics equivalence.
