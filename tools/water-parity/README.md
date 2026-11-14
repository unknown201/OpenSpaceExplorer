# Live splashdown comparison

Eight controlled real Mk1 pod releases measure splashdown, equilibrium, bobbing,
righting, lateral drag and impact destruction. OSE imports the identical local
craft and uses production APIs at 50 Hz. Native forces are never injected into OSE.
See [measured results and scope](../../docs/reference-notes/water-live-parity.md).

## Native capture

Use installed KSP2 0.2.2 and its existing BepInEx loader. Required local inputs
are an empty sandbox seed and `BepInEx/ose-parity/craft-stack.json` containing a
Mk1 pod. The preparer extracts a clean single-pod fixture and creates a dedicated
**OSE Water Validation** seed, refusing to overwrite a changed seed. Game-derived
fixtures, captures and binaries remain ignored. Do not run another capture at once.

```powershell
./tools/water-parity/run.ps1 -RunDir D:/scratch/water-run `
  -Seed 'C:/path/to/empty-sandbox/seed.json'
```

Optional `-GameDir` and `-SaveDir` override installed paths. The default seed
is the existing OSE Water Validation seed. Use a fresh RunDir for every attempt.
The driver temporarily preserves other top-level plugin DLLs, installs the
recorder and restores the plugins in `finally`. It starts a **fresh installed-game
process for every case**; reloading flight in one process is excluded because
reference module initialization can fail. Expect several minutes.

The recorder sets background execution each Update; no network-prompt action is
needed. It stages at 10,000 m relative to seabed, waits for actual game states,
primes motion caches for 0.2 s above water, then resets exact release pose and
velocity. Real part modules, water detection and destruction stay active. Samples
are taken after PhysX, using freshly read surface-frame rigidbody velocity.
Metadata records release UT, mass/crew, CoB, inertia and physics settings.

Each process writes a unique `out-<UTC timestamp>` directory and
`last-run-path.txt`. The driver preserves it under `native-cases/<case>` with
game/loader logs. `assemble_native.py` accepts exactly one completed release/end
per case, rejects failed or contaminated attempts, then creates `native/` with
combined traces, screenshots, a case hash manifest and `done.txt`. It also copies
the exact craft into the run root. Never mix partial attempts into accepted data.

| Case ID | Height | Vertical / lateral velocity | Bank | Duration |
|---|---:|---:|---:|---:|
| drop | 3 m | -5 / 0 m/s | 0° | 45 s |
| bob | 0 m | 0 / 0 m/s | 0° | 45 s |
| bank | 0 m | 0 / 0 m/s | 25° | 45 s |
| drag | 0 m | 0 / 5 m/s | 0° | 20 s |
| vertical_safe | 1 m | -16 / 0 m/s | 0° | 6 s |
| vertical_destroy | 1 m | -22 / 0 m/s | 0° | 3 s |
| horizontal_safe | 0 m | 0 / 100 m/s | 0° | 3 s |
| horizontal_destroy | 0 m | 0 / 220 m/s | 0° | 3 s |

All cases use latitude 0°, longitude -60°, SAS off. The last four IDs are legacy
identifiers, not expected survival: native 22 m/s down survives and 100 m/s lateral
destroys at the measured UT because native entry/drag projects onto an orbital
reference axis rather than local vertical. Destruction may end a trace early.

## OSE and comparison

Run from the repo root after native capture is complete. Use an isolated build
target to avoid stale cross-worktree catalog/source artifacts.

```powershell
$env:CARGO_TARGET_DIR = 'D:/scratch/water-target'
cargo run --offline -p ose-sim --example water_probe -- D:/scratch/water-run
python -B -m unittest discover -s tools/tests -p test_water_parity.py
python tools/water-parity/analyze.py D:/scratch/water-run
python tools/water-parity/provenance.py D:/scratch/water-run `
  --game 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
```

The probe requires complete native release events, uses their absolute UT and
normal crew APIs, and writes OSE samples/events. `comparison.html` contains
standalone trajectory plots and every check; `comparison.json` is machine-readable.
Exit 0 means all fixture checks pass, 1 means measured gaps, 2 means incomplete or
unmatched evidence. No fitted offsets or time shifts are allowed. Input guards,
full-trace and first-ten-second RMSE, peak immersion, equilibrium, observable bob
period, survival and destruction time are checked. Numeric limits are in
`analyze.py`. A passing capsule comparison does not establish arbitrary hull or
other-location compatibility.

## Pinned before/after comparison

Create a detached worktree at the full pre-fix commit (`git rev-parse 520d0b8`).
The adapter removes only probe accesses to APIs absent at that base; production
physics remains untouched. It refuses a mismatched revision or tracked output.

```powershell
$base = git rev-parse 520d0b8
git worktree add --detach D:/scratch/water-before $base
python tools/water-parity/prepare_baseline.py --worktree D:/scratch/water-before --base $base
New-Item -ItemType Directory -Path D:/scratch/water-before-run
Copy-Item D:/scratch/water-run/craft.json D:/scratch/water-before-run/craft.json
Copy-Item D:/scratch/water-run/native D:/scratch/water-before-run/native -Recurse
$env:CARGO_TARGET_DIR = 'D:/scratch/water-target-before'
cargo run --offline --manifest-path D:/scratch/water-before/Cargo.toml `
  -p ose-sim --example water_probe -- D:/scratch/water-before-run
python tools/water-parity/analyze.py D:/scratch/water-before-run
python tools/water-parity/provenance.py D:/scratch/water-run `
  --game 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2' `
  --baseline-worktree D:/scratch/water-before --baseline-run D:/scratch/water-before-run
```

The benchmark excludes compound/hollow hulls, planing, animated cube weighting,
Kerbal swimming, saved drag history, multiplayer and splash presentation.