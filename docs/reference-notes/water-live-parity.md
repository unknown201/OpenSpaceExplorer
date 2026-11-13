# Water force centers and live splashdown comparison

Investigation: 2026-11-12. Base: `520d0b8` (current main when this worktree was
created). The pre-existing source-derived water implementation and tests remain
the starting point. The older dirty development checkout is not the PR base.

## Comparison and root causes

| Condition | Reference source behavior | OSE before | OSE change |
|---|---|---|---|
| Capsule under water | `CalculateBuoyantLocalForcePosition` retains `PartData.coBuoyancy` | Fully submerged force applied at the part origin | Import authored CoB; retain it through full immersion |
| Shallow entry | Corner blend uses raw submerged fraction; shallow force ramp scales force separately | Corner blend used the fraction after the shallow ramp | Separate the force point from the ramped buoyancy share |
| Part mounted at an angle | Water bounds and force point follow the part | Craft-axis bounding box used for immersion and corner weighting | Rotate the original local bounds corners |
| Horizontal water motion | Rigidbody linear damping acts at the body's mass center | Applied at the drag-cube bounds center, producing spurious torque | Apply drag and its structural load at the part mass center |
| Live splashdown comparison | Running KSP2 required to establish behavior | No complete accepted native trace | Eight-case native recorder, current-production OSE probe, strict analyzer and plots added; capture still pending |

The source paths are the local KSP2 0.2.2 `PartBehavior.BuoyancyUpdate`,
`CalculateBuoyantLocalForcePosition`, `GetPartBoundCoordsNativeArray`,
`CalculateLiquidDrag`, and `RigidbodyBehavior`. The Mk1 capsule's authored CoB
is `(0, 0.5, 0)` in Unity part axes. These are source/asset findings, not measured
live agreement.

OSE paths: `ose-importer::part`, `Structure::configure_surface`,
`water::bounds_corners` and `water::force_point`. The new catalog field defaults
to zero when reading older data. The existing displacement, water-density,
entry-destruction and splash-drag coefficients are retained.

## Reproduction and evidence boundary

See [the benchmark instructions](../../tools/water-parity/README.md). Each case
loads the same fresh single-pod craft, stages above open ocean, then sets its
initial rigidbody pose and surface velocity. Native telemetry is refreshed after
that setup so a previous descent speed cannot contaminate the entry check. Actual
part modules, forces, damping and destruction callbacks remain active.

The cases cover a gentle drop, a zero-speed release, 25° bank, 5 m/s horizontal
motion, and vertical/horizontal impacts on both sides of the source threshold.
The long cases last 45 simulated seconds. Captures retain origin and mass-center
altitudes, velocities, bank, native buoyancy force/point, survival, damping and UT.
The OSE probe imports the identical local craft and uses production simulation APIs.

The analyzer rejects missing completion, cases, water entry, nonfinite samples,
time gaps, revived destroyed fixtures, unmatched initial pose/speed and mismatched
mass/displacement. Complete terminal destruction may end a trace early. It does
not fit time, height or attitude. Stationary traces cannot pass the bobbing check.
Limits were declared before any accepted native measurement.

**Live compatibility is still unverified.** Windows Security currently blocks
reference startup with a network-access prompt that requires user dismissal.
Failed/setup-only attempts are excluded. No native results or compatibility pass
are claimed, and this PR must remain draft until the requested native comparison
is complete. Local generated artifacts live under the original checkout's
`.cache/water-parity`; they are not implied to exist in a clean checkout.

## Validation

- Regression: authored CoB import and old-catalog fallback.
- Regression: full immersion preserves CoB; shallow force ramp does not alter the
  raw corner blend.
- Regression: horizontal damping on a single capsule slows it without adding
  spin even though its bounds and mass centers differ.
- Python analyzer tests cover incomplete, nonfinite and mismatched traces,
  terminal destruction, and unobservable bobbing. These run in the normal Python
  CI discovery path.
- Native recorder builds against the installed game with no warnings.
- Full workspace checks and required GitHub CI are run before landing.

Animated cube weighting, compound/hollow hulls beyond these fixtures, Kerbal
swimming, continuity of water-drag history across save/load, and browser splash
presentation remain outside the comparison. KSP2 itself has no flooding or waves.
