# Reference notes: orbits, propagation and patched conics

Our own notes from reading the decompiled `KSP.Sim.impl` (`PatchedConicsOrbit`, `PatchedConicSolver`,
`PatchedConics`, `OrbiterComponent`, `CelestialBodyComponent`) and `KSP.Sim.CelestialFrame`. Written so that
`ose-math` and `ose-sim` can be implemented and tested against the original behaviour without copying code.

## Constants and derived body values

- `g0 = 9.80665`, `G = 6.6743e-11`.
- `mu = gravityASL * g0 * radius^2`; `mass = mu / G`. (`CelestialBodyComponent.SetupConstants`.)
- Sphere of influence: `a * (m / M_parent)^0.4` for a child of a body; a fixed `9.4607e15` m (one light year)
  for a child of the galactic origin; `0` for `ForceNone`; `+inf` for the root star. All stock bodies use the
  first rule (`SphereOfInfluenceCalculationType = 0`).
- Synchronous altitude: `max(cbrt(mu * T^2 / (4 pi^2)), radius + 1000)`.
- Atmosphere: `lapse = T_sl / depth`; the pressure and temperature curves in the body JSON are authoritative
  (`useAtmospherePressureCurve = true` for every stock body), sampled by altitude in metres.

## Frames and axis conventions

- The game keeps two conventions and swaps between them constantly: Unity **Y-up** for everything a transform
  touches, and a **Z-up** orbital frame for Kepler math. `SwapYAndZ` converts between them; every `...Zup`
  method returns Z-up vectors. We will use one right-handed Z-up frame everywhere in `ose-sim` and only
  convert at the renderer.
- Orbital frame from elements (`CelestialFrame.SetFrame(LAN, inc, argPe)` with angles in radians):

  ```
  X = ( cosL cosW - sinL cosI sinW,  sinL cosW + cosL cosI sinW,  sinI sinW )
  Y = (-cosL sinW - sinL cosI cosW, -sinL sinW + cosL cosI cosW,  sinI cosW )
  Z = ( sinL sinI,                 -cosL sinI,                   cosI      )
  ```
  X points at periapsis, Y is 90 degrees ahead in the orbit, Z is the orbit normal. The line of nodes is
  `an = forward x Z` (with `forward = (0,0,1)`), falling back to `right` for equatorial orbits.
- Planet orientation uses `PlanetaryFrame(ra, dec, rot)` which is the same construction with
  `(ra - 90, dec - 90, rot + 90)` degrees; stock bodies have zero axial tilt, so this is only `initialRotation`
  plus `360 * t / rotationPeriod` about Z.
- Each body also carries a `celestialTilt.swizzle` rotation; star-space positions are accumulated parent by
  parent (`GetPositionAtUTInStarSpace`): `pos_star = pos_star(parent) + rot * rel_pos_zup(child)` swapped to
  Y-up. For stock bodies with zero tilt this is a plain sum of relative positions.

## Elements and the epoch

Elements stored per orbit: `a` (semi-major axis, negative for hyperbolic), `e`, `i`, `LAN`, `argPe` (degrees),
`M0` (mean anomaly at epoch, radians), `epoch` (UT seconds).

- Mean motion `n = sqrt(mu / |a|^3)`; period `T = 2 pi / n` (elliptic) or `+inf`.
- `ObT` ("orbit time", seconds since periapsis) is the game's working variable instead of mean anomaly:
  `ObT = M / n`, and `ObTAtEpoch` is cached at element creation.
- `GetObtAtUT(UT)`:
  - elliptic: `ObT = (UT - epoch + ObTAtEpoch) mod T`, then if `ObT > T/2` subtract `T` so ObT is in
    `(-T/2, T/2]`. Negative means "before periapsis".
  - hyperbolic: `ObT = ObTAtEpoch + (UT - epoch)`.
- `TimeToPe = (T - ObT) mod T`, `TimeToAp = TimeToPe - T/2` wrapped into `[0, T)`.

## Anomaly solvers (`SolveEccentricAnomaly`)

- `e < 0.8`: Newton on `E - e sin E = M`, starting guess `M + e sin M + 0.5 e^2 sin 2M`, stop when the step is
  below `1e-7`.
- `0.8 <= e < 1`: eight fixed iterations of a Halley/Laguerre-style step with start `M + 0.85 e sign(sin M)`:
  `E -= 5 f / (f' + sign(f') sqrt(|16 f'^2 - 20 f f''|))` with `f = E - e sin E - M`, `f' = 1 - e cos E`,
  `f'' = e sin E`.
- `e > 1`: Newton on `e sinh H - H = M`, start `H = asinh(2M / e)`, tolerance `1e-7`. Infinite `M` passes
  through as infinite `H`.
- True anomaly from eccentric: `nu = 2 atan2(sqrt(1+e) sin(E/2), sqrt(1-e) cos(E/2))`; hyperbolic
  `nu = 2 atan2(sqrt(e+1) sinh(H/2), sqrt(e-1) cosh(H/2))`, and `H = +-inf` maps to `+-acos(-1/e)`.
- Inverse (`GetEccentricAnomaly`): `E = 2 atan2(sqrt(1-e) sin(nu/2), sqrt(1+e) cos(nu/2))`; hyperbolic
  `H = ln((1+x)/(1-x))` with `x = sqrt((e-1)/(e+1)) tan(nu/2)`, infinite when `|x| >= 1`.

## State vectors from elements (`GetOrbitalStateVectorsAtTrueAnomaly`)

```
p  = a (1 - e^2)                      semi-latus rectum
r  = p / (1 + e cos nu)
pos = X * (r cos nu) + Y * (r sin nu)
k  = sqrt(mu / p)
vel = X * (-k sin nu) + Y * (k (cos nu + e))
```
Both are then expressed in the body's Z-up frame at that UT (`ZupAtUT`), which for untilted bodies is the
identity. This is the hot path: on-rails vessels call `UpdateFromUT` every fixed step.

## Elements from state vectors (`UpdateFromStateVectors`)

Inputs are position and velocity relative to the reference body (converted to Z-up first).

```
h   = r x v                                   specific angular momentum
an  = forward x h                             node vector (fallback: right)
LAN = atan2(an.y, an.x) in [0, 360)
ev  = (v x h) / mu - r / |r|                  eccentricity vector, e = |ev|
E   = |v|^2 / 2 - mu / |r|                    specific energy
a   = -mu / (2E)            if e < 1
a   = -p / (e^2 - 1)        otherwise (p = |h|^2 / mu)
X   = ev / e  (or an / |an| when e ~ 0)
argPe = acos(an . X / |an|), reflected to 2 pi - argPe when X.z < 0
Z   = h / |h|;  Y = Z x X
inc = acos(Z.z)
nu  = atan2(r . Y, r . X);  E from nu;  M0 = M(E);  epoch = UT
```
Degenerate `h = 0` (radial trajectory) sets inclination from `r` and builds the frame from `an`.

## Patch handoff between bodies (`UpdateFromOrbitAtUT`)

To express an orbit around a new body at time UT: compute the vessel's and the new body's position and velocity
in star space by walking up the parent chain, take the differences, rotate them into the new body's frame, then
run elements-from-state-vectors with the new `mu`. This is used both for SOI escape (to the parent) and
encounter (to the child).

## Patched conic trajectory (`PatchedConicSolver`, `PatchedConics`)

A trajectory is a list of up to 10 patches; the user setting `ConicPatchLimit` (1 to 5) bounds how many are
computed per update. Every fixed update the solver recomputes from the current orbit:

1. First patch starts at the current UT and ends one period later (or at `T = inf` for hyperbolic).
2. **Collision check** (`WillCollideWithParent`): if `periapsis < radius + MaxTerrainHeight` and periapsis is
   ahead, bisect `radius + MaxTerrainHeight - |r(UT)|` on `[start, start + TimeToPe]` to find the impact UT.
   Patch ends with transition `Collision`.
3. **Encounters** with every body orbiting the same reference body (`CheckEncounter`):
   - quick reject unless the periapsis/apoapsis ranges overlap within `SOI * 1.1` (or `sqrt(a_sec / SOI)` for
     the targeted body);
   - find the closest-approach geometry between the two conics (`FindClosestPoints`, up to 25 iterations,
     tolerance 1e-4), giving up to two candidate true anomalies on each orbit;
   - convert them to UTs, keep the ones inside the patch's time window, then refine the closest approach in
     time (`SolveClosestApproach`, up to 50 iterations, tolerance 0.01 s);
   - if the closest distance is below the body's SOI, bisect for the exact SOI-crossing UT (`SolveSOI_BSP`),
     require the relative radial velocity to be inbound (`dot(dr, dv) < 0`), and start the next patch around that
     body at that UT. Transition `Encounter`.
4. **Escape**: if no encounter and `apoapsis > SOI_ref` or `e >= 1`, the exit true anomaly is
   `TrueAnomalyAtRadius(SOI_ref)`, the exit UT follows from `GetDTforTrueAnomaly`, and the next patch is built
   around the reference body's parent. If the reference body is the star (infinite SOI) the patch is `Final`
   with the asymptote anomaly `acos(-1/e)`.
5. Otherwise the patch is `Final` and ends after one period.

Each patch records `StartUT`, `EndUT`, start and end transition types (`Initial`, `Encounter`, `Escape`,
`Collision`, `Final`), the SOI-crossing UT, and closest-approach data for the map UI.

## Time warp interaction (`OrbiterComponent.OnFixedUpdate`)

- On-rails vessels (not `RigidBody` physics, or rigid body without a PhysX position driver) are updated by
  `UpdateFromUT(UT)` each fixed step.
- If a vessel's SOI crossing falls inside the next warped step, the game lowers the time-scale for that step so
  the crossing lands on a step boundary (`SetPendingTimescaleMultiplierOverride`). We should do the same in
  `ose-sim`: clamp the tick length so no vessel skips over an SOI boundary.
- After the step, if the body that owns the vessel's position (`GetMainBody`) differs from the orbit's reference
  body, elements are rebuilt around the new body from the current state vectors.
- Vessels under thrust while on rails (`IsOrbitalPhysicsUnderThrustActive`) rebuild elements from state vectors
  every step.
- Inactive on-rails vessels are checked for terrain collision each step (`CheckInactiveVesselCollision`).

## What we implement in `ose-math` / `ose-sim`

- `Orbit { mu, a, e, inc, lan, argpe, m0, epoch }` with `state_at(ut) -> (pos, vel)`, `from_state(pos, vel, mu, ut)`,
  `obt_at(ut)`, `time_to_true_anomaly(nu)`, `true_anomaly_at_radius(r)`, periapsis/apoapsis/period.
- Solver tolerances identical to the original (`1e-7`, 8 iterations for `e >= 0.8`, Newton for hyperbolic).
- Patched-conic trajectory with the same transition rules and the same iteration budgets, so predicted
  encounter times agree with the original within the solver tolerance.
- Parity tests: Kerbin to Mun Hohmann transfer encounter time, Kerbin escape UT for a given hyperbolic orbit,
  Moho's position at UT 0 and after one Kerbin year, all compared against numbers computed from the same
  elements with an independent high-precision propagator (and spot-checked in the original game).
