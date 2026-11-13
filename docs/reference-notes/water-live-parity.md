# Live splashdown compatibility

Measured 2026-11-12 against installed KSP2 **0.2.2.0**, Unity **2022.3.5f1**,
at a 0.02 s physics step. All eight controlled Mk1 capsule cases pass. The
pre-fix production code at `520d0b8` passes three under the same checks.

## Fixture and measurement

Each case starts a fresh installed-game process and loads the same dedicated empty
sandbox. A real OAB Mk1 pod, SAS off, is released over Kerbin's ocean at latitude
0°, longitude -60°. Its mass is 0.9925 t: 0.8575 t dry, 0.04 t resources and
0.095 t crew. Native telemetry confirms mass, authored CoB `(0, 0.5, 0)`,
inertia, ocean density and initial pose/velocity. Crew is added in OSE through
the normal Universe crew API; measured native forces are never injected.

The recorder stages at 10,000 m relative to seabed, waits for flight setup,
primes motion caches for 0.2 simulated seconds, then restores exact release
pose and velocity. It samples after PhysX using `WaitForFixedUpdate`, including
a fresh surface-frame rigidbody velocity. The OSE probe uses the identical local
craft, geography, absolute release UT and production flight/surface APIs. Background
execution allows local capture without acting on the Windows network prompt.

Accepted local evidence is exclusively
`D:/Projects/OpenSpaceExplorer/.cache/water-parity/final-20261112c`.
It contains eight complete fresh-process captures, per-case screenshots and logs,
an input hash manifest, OSE traces, `comparison.json` and standalone trajectory
plots in `comparison.html`. Earlier partial/setup attempts are excluded. Raw
game-derived inputs and binaries stay ignored and are not included in the repo.
See [reproduction instructions](../../tools/water-parity/README.md).

## Results

Heights below are **part-origin altitude relative to sea level**, not draft from
the lowest bounds corner. Equilibrium is the final five seconds of each 45 s case.
RMSE uses corresponding timestamps without fitted time, height or attitude shifts.

| Release | Duration | Native / OSE result | Altitude RMSE | Other measured agreement |
|---|---:|---|---:|---|
| 3 m high, 5 m/s down | 45 s | Both survive | 0.0329 m | Minimum altitude -1.8511 / -1.7248 m; equilibrium -0.0509 / -0.0304 m; bob period 1.30 / 1.30 s |
| At sea level, stationary | 45 s | Both survive | 0.0065 m | Equilibrium -0.0246 / -0.0304 m; bob period 1.29 / 1.30 s |
| At sea level, 25° bank | 45 s | Both right and survive | 0.0033 m | Bank RMSE 0.5821°; equilibrium -0.0309 / -0.0304 m; bob period 1.32 / 1.42 s |
| At sea level, 5 m/s lateral | 20 s | Both slow and survive | 0.0131 m | Horizontal velocity RMSE 0.0840 m/s; bank RMSE 1.2405° |
| 1 m high, 16 m/s down | 6 s | Both survive | 0.0137 m | Minimum altitude -4.1489 / -4.1419 m |
| 1 m high, 22 m/s down | 3 s | Both survive | 0.0231 m | Minimum altitude -6.4890 / -6.4775 m |
| At sea level, 100 m/s lateral | 3 s or destruction | Both destroyed at 0.02 s | — | Survival and destruction time agree |
| At sea level, 220 m/s lateral | 3 s or destruction | Both destroyed at 0.02 s | — | Survival and destruction time agree |

Maximum equilibrium error is 0.0205 m. The old implementation's lateral velocity
RMSE was 2.9189 m/s; it also failed peak immersion on the gentle drop, altitude on
the 16 m/s entry, and survival on the 22 m/s downward and 100 m/s lateral entries.
The old bob, bank and 220 m/s cases passed. Baseline captures are under
`.cache/water-parity/before-final`; its production code is the pinned pre-fix
revision, with only an untracked probe adapter for interfaces absent at that base.

The numeric limits were declared before the accepted capture: altitude RMSE
0.15 m, vertical velocity 0.6 m/s, horizontal velocity 0.5 m/s and bank 3°.
These limits also apply separately to the first ten seconds, so a quiet tail
cannot hide entry errors. Peak-immersion error must be ≤0.15 m; 45 s equilibrium
error ≤0.05 m; bob-period error ≤0.2 s; survival must agree and destruction time
must be within 0.1 s. Initial height/velocity/bank guards are 0.01 m, 0.05 m/s and
0.05°, with mass/displacement agreement within 0.001. Stationary traces cannot
establish bobbing. Incomplete, nonfinite, gapped or mismatched traces are rejected.

## Source findings and changes

The live outcomes exposed a reference-frame quirk that source-only tests missed.
`PartBehavior.BuoyancyUpdate` calls `UniverseModel.GetUpAxis` with the body's
orbital-parent coordinate system and part-local `coBuoyancy`, then dots those
celestial components directly with body-frame surface velocity. The resulting
entry/damping axis is not the local vertical. Thus the nominal `1.2 * crashTolerance`
threshold does **not** mean a universal 18 m/s downward limit for this pod. At
the measured location and release UT, 22 m/s downward survives while 100 m/s lateral
destroys it. OSE now derives the same axis from body orbit and rotation; actual
buoyancy continues to follow gravity. Legacy capture IDs `vertical_destroy` and
`horizontal_safe` are identifiers only; the reference determines the outcome.

Other fixes follow `CalculateBuoyantLocalForcePosition`,
`GetPartBoundCoordsNativeArray`, `CalculateLiquidDrag` and `RigidbodyBehavior`:

- Import authored CoB, defaulting to zero for older catalogs; retain it at full
  immersion and blend corners using the raw wet fraction before shallow force ramping.
- Rotate part-local bounds rather than using a craft-axis AABB for immersion.
- Apply drag at each part's occupied mass center and use its own inertia tensor
  for angular damping, avoiding spurious righting torque from drag bounds.
- Consume water forces/damping one physics step after preparation, matching
  RigidbodyBehavior priority 4 before PartBehavior priority 40.
- Apply PhysX linear damping after gravity/buoyancy force accumulation rather
  than approximating it with implicit drag.

Displacement, density and authored coefficient values were retained. No measured
trajectory was used as a force input. Remaining centimetre-scale geometric
differences are reported as error; this run does not establish their cause.

## Validation and scope

Rust regressions cover authored CoB, rotated bounds, force points, mass-center
drag, delayed application, PhysX damping and frame-dependent entry. Python tests
cover comparator rejection paths, observable bobbing, early errors hidden by a
long tail, and the baseline adapter. Workspace formatting, Clippy, tests, Python
discovery and the client WASM check are required before landing.

The source-only settling fixture's arbitrary 6 s bound was corrected to 8 s:
the live gentle drop exceeds 0.05 m/s vertical motion until 7.52 s. Its equilibrium
and final-speed assertions remain. That synthetic fixture intentionally omits
orbital-parent context and still tests the aligned-axis crash threshold.

Compatibility is established for these capsule fixtures, location, release times
and 50 Hz step. Compound/hollow hulls, animated drag-cube weighting, other
locations/steps, planing, Kerbal swimming, saved water-drag history and browser
splash presentation remain outside this comparison.