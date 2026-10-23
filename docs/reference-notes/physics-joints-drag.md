# Reference notes: physics bubble, part joints, aerodynamics, engines

From reading `KSP.Sim.impl` (`RigidbodyBehavior`, `PartOwnerBehavior`, `PartComponent`, `CelestialBodyComponent`,
`PartComponentModule_Engine`), `KSP.Modules` (`Module_Drag`, `Data_Drag`, `Data_Engine`), `PhysicsSettings` and
`AeroUtilities`. Numbers quoted are the C# defaults in `PhysicsSettings`; the shipped settings asset can override
them, so the importer should also extract `PhysicsSettings` from the bundles and we test against those values.

## Rigid bodies and gravity

- Each part is a PhysX rigid body; parts of one vessel are linked by configurable joints (below). The physics
  bubble is a floating-origin space; on-rails motion is handled by `OrbiterComponent` (see `orbits.md`).
- Unity gravity is off (`useGravity = false`). Every fixed step the game computes the gravitational acceleration
  at the body's position from the universe (`PhysicsSpace.GetGravityForceAtPosition`, sum over bodies in the
  current SOI chain) and applies it with `ForceMode.Acceleration`. Below a tiny threshold
  (`PHYSX_RB_SQR_MAG_GRAVITY_THRESHOLD`, default `double.Epsilon`) nothing is applied.
- Optional "gravity ease" ramps the acceleration over `GravityEaseFrames` frames after unpacking, to avoid
  joint snapping when a vessel goes from rails to physics.
- Module forces are registered on the part (`part.AddForce(PartForce)`) with a relative position and a relative
  force, and applied every step with `AddForceAtPosition` in either `Force` or `Acceleration` mode. Drag, body
  lift and engine thrust all go through this.
- At-rest detection: surface speed squared below `0.1` counts as at rest, below `0.25` m/s counts as landed.

## Part joints (`PartOwnerBehavior.CreateJoint`, `GetJointBreakLimits`)

One `ConfigurableJoint` per attach node (or `MultiJointMaxJoint + 1` joints arranged around the node when
`IsMultiJoint` is on; multi-joints are enabled by default), between the host part's rigid body and the connected
part's rigid body. Anchor is the node position, primary axis the node orientation, secondary axis any
orthonormal tangent, `autoConfigureConnectedAnchor = false`, `configuredInWorldSpace = false`.

Defaults (`PhysicsSettings`):

| Name | Default |
|---|---|
| `JOINT_RIGIDITY` | 1500 |
| `JOINT_STACK_NODE_FACTOR` | 50 |
| `JOINT_SURFACE_NODE_FACTOR` | 0.8 |
| `JOINT_STACK_ATTACH_STIFFNESS` | 10 |
| `JOINT_SURFACE_ATTACH_STIFFNESS` | 10 |
| `JointForce` | 1e20 |
| `JointDampingFactor` | 0.01 |
| `JointBreakForceFactor` / `JointBreakTorqueFactor` | 50 / 50 |
| `RigidJointBreakForceFactor` / `RigidJointBreakTorqueFactor` | 1 / 1 |

Per joint, with `size` the attach node size (0, 1, 2...):

```
nodeFactor = (size + 1) * JOINT_STACK_NODE_FACTOR        (stack node)
           = (size + 1) * JOINT_SURFACE_NODE_FACTOR      (surface node)
stiffness  = JOINT_RIGIDITY * nodeFactor * JOINT_{STACK|SURFACE}_ATTACH_STIFFNESS
sizeDiv    = size < 2 ? 1 : 3           (1 when MultiJointFullBreakStrength)
breakForce  = min(hostBreakingForce,  otherBreakingForce)  * nodeFactor / sizeDiv / max(multiJointCount,1)
              * JointBreakForceFactor  * (rigid ? RigidJointBreakForceFactor  : 1)
breakTorque = min(hostBreakingTorque, otherBreakingTorque) * nodeFactor / sizeDiv / max(multiJointCount,1)
              * JointBreakTorqueFactor * (rigid ? RigidJointBreakTorqueFactor : 1)
```
Launch clamps make the joint unbreakable and infinitely stiff. The "UnbreakableJoints" difficulty option sets
both break limits to infinity.

Joint configuration:

- Linear x/y/z and angular x/y/z motion: `Limited` for normal joints, `Locked` for rigid ones
  (`AttachNode.isRigid`, or reinforced connections).
- Linear drives: spring `JointForce` (1e20), damper 0, max force `JointForce`, target position 0. Linear limit
  1 m, no bounce, no limit spring. In effect, translation is held by a near-infinite spring.
- Angular drives (X, YZ and slerp, `RotationDriveMode.XYAndZ`): spring `stiffness`, damper
  `stiffness * JointDampingFactor`, max force `JointForce`; rigid joints use infinite spring and zero damper.
  Angular limits 180 degrees (0 for rigid). So the "wobble" of a stack is entirely the angular spring, with
  stiffness growing linearly with node size.

Example, two size-1 parts with `breakingForce = 50`, `breakingTorque = 50` (the Terrier's values): nodeFactor
100, stiffness 1.5e6, breakForce `50 * 100 / 1 * 50 = 250000`, breakTorque the same.

Our mapping: rapier `GenericJoint` / `SpringJoint` per attach node with a stiff linear spring and an angular
spring of `stiffness` with damping ratio from `JointDampingFactor`, plus break thresholds checked each step from
the joint impulse. Keep the formulas above as a pure function in `ose-sim` so the values can be unit tested.

## Atmosphere properties used by flight (`CelestialBodyComponent`, `PartComponent`)

```
pressure(h)    = pressure curve (kPa) at altitude h, 0 above atmosphereDepth
temperature(h) = temperature curve (K) at altitude h
density        = pressure * 1000 * molarMass / (R * T)             R = 8.31447
dynamicPressure_kPa = 0.0005 * density * |v_rel|^2                  (v relative to the rotating body frame)
speedOfSound   = sqrt(adiabaticIndex * 1000 * pressure / density)   0 when pressure <= 0 or density <= 1e-7
mach           = |v_rel| / speedOfSound
staticPressureAtm = pressure / 101.325
```
Parts take these from the vessel (computed at the vessel's centre of mass) unless standalone.

## Drag cubes (`Data_Drag.GetAeroDataForDirection`, `Module_Drag.UpdateDrag`)

Each part carries one or more drag cubes with six faces in the order `+X, -X, +Y, -Y, +Z, -Z`; each face has
`area`, `drag` (a 0..1 "drag coefficient" from the cube bake), `depth` and `dragMultiplier`. Multiple cubes are
blended by `Weight`. `weightedExposedArea` is the per-face area after occlusion by attached parts
(`SetExposedArea`: a face covered by a stack-attached part loses the attached node's cross-section).

Per step, with `d = velocity direction` in part space and `m = mach`:

```
for each face i with normal n_i:
  dot      = n_i . d
  dotN     = (dot + 1) / 2
  curveVal = DragCurveValue(dotN, m)
           = lerp(Tail(m), Surface(m), dotN * 2) * Multiplier(m)            dotN <= 0.5
           = lerp(Surface(m), Tip(m), (dotN - 0.5) * 2) * Multiplier(m)     otherwise
  areaFace = exposedArea_i * curveVal
  cd       = drag_i < 1 ? Cd(drag_i) ^ CdPower(m) : drag_i
  AreaDrag += areaFace * cd * dragMultiplier_i
  Area     += areaFace
  AreaAirstream += area_i * clamp01(dot)
  if dot > 0:
    LiftForce += -n_i * dot * exposedArea_i * drag_i * dragMultiplier_i * BodyLiftCurve(dot)
DragCoeff = AreaDrag / Area
```

Then (`Module_Drag.UpdateDrag`), with `q` the dynamic pressure in Pa (`1000 * q_kPa`):

```
pseudoRe   = PseudoReynolds(density * |v|)
dragScalar = AreaDrag * DragCubeMultiplier(0.1) * pseudoRe * DragMultiplier(8) * q * 0.001
F_drag     = -d * dragScalar         applied at coPressureOffset (+ lifting surface CoP offset)
bodyLift   = q_kPa * bodyLiftMultiplier(part) * BodyLiftMultiplier(18) * liftMachCurve(m)
F_lift     = projectOnPlane(bodyLift * LiftForce, -d)     applied at coLiftOffset
```
So drag force in newtons is `AreaDrag * 0.1 * pseudoRe * 8 * q_Pa / 1000 = 0.0008 * AreaDrag * pseudoRe * q_Pa`.

Default curves (`time, value, inTangent, outTangent`), all evaluated with Unity's Hermite interpolation and
clamped ends:

- `DragCurveTip` (mach): (0, 1), (0.85, 1.19, 0.696), (1.1, 2.83, 0.730), (5, 4)
- `DragCurveSurface` (mach): (0, 0.02), (0.85, 0.02), (0.9, 0.01524, -0.0794), (1.1, 0.0025, -0.00528/-0.00194),
  (2, 0.002083, -2.3e-5), (5, 0.003333, -1.8e-4), (25, 0.001429, -7.1e-5/0)
- `DragCurveTail` (mach): (0, 1), (0.85, 1), (1.1, 0.25, -0.0222/-0.0249), (1.4, 0.22, -0.0339), (5, 0.15, -0.0012),
  (25, 0.14)
- `DragCurveMultiplier` (mach): (0, 0.5), (0.85, 0.5), (1.1, 1.3, 0/-0.0081), (2, 0.7, -0.110), (5, 0.6), (10, 0.85,
  0.022), (14, 0.9, 0.0077), (25, 0.95)
- `DragCurveCd` (cube drag value): (0.05, 0.0025, 0.15), (0.4, 0.15, 0.396), (0.7, 0.35, 0.907), (0.75, 0.45, 3.21),
  (0.8, 0.66, 3.50), (0.85, 0.8, 2.21), (0.9, 0.89, 1.1), (1, 1, 1)
- `DragCurveCdPower` (mach): (0, 1, 0/0.00716), (0.85, 1.25, 0.778), (1.1, 2.5, 0.249), (5, 3)
- `DragCurvePseudoReynolds` (density*speed): (0, 4, -111111/-2975), (1e-4, 3, -251), (0.01, 2, -19.6),
  (0.1, 1.2, -0.785/-0.391), (1, 1), (100, 1), (200, 0.82), (500, 0.86, 1.9e-4), (1000, 0.9, 1.5e-5), (10000, 0.95)
- `LiftMultiplier` 0.036, `LiftDragMultiplier` 0.015 (lifting surfaces, wings; separate module).

These curves are also in the shipped `PhysicsSettings` asset; the importer should extract that asset and we
should load the curves from data rather than hard-coding them.

## Engines (`Data_Engine.SetConsumptionRate`, `PartComponentModule_Engine.RequiredPropellantMass`)

Per engine mode, from the part JSON: `maxThrust`, `minThrust` (kN), `atmosphereCurve` (Isp in s against
pressure in atm), `propellant.mixtureName`, optional throttle/atm/velocity Isp curves, `multIsp`, `multFlow`.

```
IspVac       = atmosphereCurve(0)
MaxFuelFlow  = maxThrust / (IspVac * g0)        tonnes/s (thrust in kN, so mass in t)
MinFuelFlow  = minThrust / (IspVac * g0)
ThrottleMin  = MinFuelFlow / MaxFuelFlow

each step:
  Isp        = atmosphereCurve(staticPressureAtm)
               * throttleIspMult(atm, throttle)     if useThrottleIspCurve
               * atmCurveIsp(density * 0.8163)      if useAtmCurveIsp      (0.8163 = 1 / 1.225 kg/m^3)
               * velCurveIsp(mach)                  if useVelCurveIsp
  massFlow   = MaxFuelFlow * throttle * flowModifier * multFlow
  thrust     = massFlow * Isp * g0 * multIsp            (kN)
  FinalThrust = thrust * (useEngineResponseTime ? spool : 1)
```
The mass flow is turned into a resource flow request for the mixture (Methalox = 0.2 Methane + 0.8 Oxidizer
per unit; unit mass from the resource definitions). If the request is only partly satisfied the engine's
thrust scales by the delivered fraction; zero delivery flames out. Fuel request and delivery are described in
`resource-flow.md`.

Parity targets for Phase 5/6: Terrier at sea level and vacuum (60 kN, Isp 335 vac / 170 asl, mass flow
`60 / (335 * 9.80665) = 0.018262 t/s`), a stock craft's delta-v readout, and drag on a 1.25 m stack at 100 m/s
at sea level.
