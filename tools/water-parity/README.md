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

Use the installed game executable and its existing BepInEx loader. `run.ps1`
temporarily moves other plugin DLLs into the run's backup directory, installs
only this recorder, and restores the original DLLs after the process exits.
It uses a dedicated empty **OSE Water Validation** sandbox seed. Other campaigns
and recorder markers are preserved. Do not run another reference capture during
this benchmark. Game-derived fixtures, binaries and captures stay ignored.

```powershell
./tools/water-parity/run.ps1
```

Pass `-GameDir`, `-SaveDir`, `-Seed` and `-RunDir` for other local
paths. The seed must be an empty sandbox JSON. Existing or changed captures/seeds
are refused rather than overwritten. Activate the game window if startup pauses.
The recorder corrects invalid tiny window dimensions, waits for the actual game
states, uses universe time for holds, and quits after completion or setup failure.
Expect several minutes. Require `out/done.txt` and absence of `out/failed.txt`.

The driver copies `BepInEx/ose-water-validation/out` to the run's `native/`
directory after exit. It also copies the exact native `craft.json` to the run
root. Preserve unsuccessful attempts separately; never merge different attempts.

```powershell
cargo run --offline -p ose-sim --example water_probe -- .cache/water-parity
python -B -m unittest discover -s tools/tests -p test_water_parity.py
python tools/water-parity/analyze.py .cache/water-parity
python tools/water-parity/provenance.py .cache/water-parity `
  --game 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
```

`comparison.html` contains standalone trajectory plots and every comparison check.
`comparison.json` is machine-readable. Exit **0** means all fixed fixture checks
pass, **1** means measured gaps, and **2** means incomplete/unmatched evidence.
The analyzer rejects missing cases, water entries, completion, trace tails, sample
gaps, nonfinite data, and mismatched part mass/displacement. It does not align or
fit time, height or attitude. Limits are declared in `analyze.py` before capture.

The benchmark does not establish arbitrary compound-hull behavior,
planing fidelity, animated drag-cube weighting, Kerbal swimming,
save/load continuity of drag history, multiplayer presentation or splash effects.
Passing capsule fixtures cannot establish universal water-physics equivalence.
