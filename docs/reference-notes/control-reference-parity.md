# Flight control reference port

The installed KSP2 `KSP.Sim/VesselSAS.cs`, `PIDclamp.cs`, and
`KSP.Sim.impl/FlightInputHandler.cs` are the reference for this change.

## Implemented

- Full keyboard axis strength; the previous 0.3 multiplier is removed. Precision remains 0.1.
- Trim changes at 0.1 units per second while Alt is held; Alt-X resets it. Trim is added
  after precision scaling. Manual input no longer switches a pointing mode into Fixed mode.
- Separate pitch/roll/yaw PID history, reference gains (15000/0/2000/1500 for pitch/yaw,
  9000/0/600/2000 for roll), output clamps, piecewise acceleration autotuning and warp scalars.
- Reference coasting/stopping decisions, with Unity Euler wrapping before the coasting test
  and angular trimming afterward.
- Reference manual override comparisons, PID resets, damping cooldown, current/previous
  rotation locks, and stability-decay thresholds/rates, including its residual-spin reset.
- Persistent per-vessel controller state survives save/load. Control planning advances it
  once per physics tick; integration reuses the resource-resolved allocation.
- Per-axis torque authority replaces the previous scalar normalization for manual inputs.
  Holding roll applies torque, rather than enforcing the old 0.5 rad/s target rate.
- Both part-backed and scalar-inertia vessels now use the same SAS controller during integration.
- Stock reaction wheels receive shared vessel-axis commands rather than competing for an optimized
  torque allocation. Mount rotation no longer redirects a wheel's pitch/roll/yaw axes, matching
  `Module_ReactionWheel.ActiveUpdate`.
- Wheel actuation follows the stock `TorqueResponseSpeed = 30` clamped linear response on press,
  release and reversal. Electric-charge demand sums the absolute pitch/roll/yaw torques as the
  reference's `InputSumForResource` does. Residual torque during ramp-down still consumes charge.

`ose-sim/src/sas.rs` keeps the controller separate from the integration and allocation code.
Unity angular axes and rotations are reflected into OSE's right-handed physics frame.

## Verification and remaining parity boundary

Tests check the reference PID step values, autotune breakpoints, manual override and release,
and small-angle torque values at three timesteps. Integration regressions check resource bounds,
free angular momentum, steering direction, pointing modes, braking and eventual attitude hold.
The previous monotonic-convergence and fixed-roll-rate expectations were replaced because those
were properties of the removed controller, not of the reference PID algorithm.

The additional control/feel pass uses committed `flight-geometry.json`, overlaid by the
importer, for engine force anchors/directions/weights, asymmetric gimbal limits and
response, RCS nozzles, part mass offsets and compound collider tensors. The exporter
reads the installed game without modifying it. 398 of 400 parts have usable authored
collider inertia; the two non-rendering colony transport trunks retain bounds fallback.
These tensors integrate authored shapes; PhysX cooking and dynamically resized
procedural meshes have not been measured against the executable.

Physical gimbal response uses the reference clamped Lerp. Both dynamics and joint
loading retain the individual resolved nozzle forces. The renderer uses these same
per-pivot rotations for meshes, exhaust and ground blast. RCS normal-mode commands
combine positive rotation/translation projections per nozzle and clamp their sum,
matching `Module_RCS.ProcessThrust`; partial fuel delivery is reallocated within
per-part reservations and consumed once. Actual delivered nozzle powers drive plumes.

Surfaces now deflect normals before evaluating the lift/drag curves, including their
nonlinear stall behavior and actuator speed. Separate lift and pressure offsets are
used. The procedural visual rotation must be applied to their authored model axes.
The control mixer now follows the reference hinge projection, ahead-of-COM pitch/yaw
reversal and distance-weighted roll term. Surfaces actuate without air. Deployment
adds the reference target offset, 1.5-range clamp, inversion and actuator response.
Separately deployed grid fins use their authored lift coefficient, deployment-area
scale, 75-degree manual-control lockout, speed and IgnoreSASInputs flag. Their
independent deploy bone is driven from the authoritative angle. Runtime
raster-derived procedural moving areas remain unverified; neither this nor the
collider import is a claim of identical runtime procedural geometry.

Flight PAM supports wheel enable/authority/input mode, gimbal enable/travel/axis flags,
RCS enable/authority/axis flags, surface authority/axis flags/inversion/deploy angle,
and thrust limits. Command and docking parts expose Control from here. Selection rotates the
control frame without changing position or spin; detached control points reset. All
settings/control-frame data survive save/load and the server validates ownership,
attachment and finite bounds.

Gamepad/joystick axes are polled through the browser Gamepad API. Settings persist
axis assignment/inversion, dead zone, response exponent and optional throttle axis.
Disconnects and missing axes produce zero rotation. Keyboard precision and trim still
apply. Raw module inputs are sent separately from the precision-scaled, trimmed SAS
inputs; manual-only wheels and IgnoreSASInputs surfaces therefore receive the same
raw input as the reference. Analog input participates in trim before scaling.
Button mapping and device-specific hardware calibration remain outside this increment.

Protocol 53 carries precision mode explicitly. The normal flight handler retains its
0.1 translation scale, followed by the RCS module's authored lever-distance reduction
or 0.1 factor. Full-thrust snapping precedes precision; the strict 0.05 request
threshold follows it. Shielded thrusters obey shieldedCanActivate. These details are
covered by threshold/order, raw wheel-mode and grid-fin deployment regressions.

Client joint flex is displayed at physical scale (previously exaggerated twelve times).
Attitude extrapolation now uses authoritative full angular velocity, including roll,
rather than waiting for a second pose sample to infer stopping. Prediction remains
bounded to 100 ms and corrections to 50 ms; network input delay is not eliminated.

`tools/check_control_parity.py` compiles the local reference `PIDclamp.cs` and extracts
`VesselSAS.EvaluateScalar` into an isolated .NET harness. Its committed numerical
fixture contains 864 PID updates and 13 autotune values with source hashes. Rust tests
compare every step at 20/40/80 ms, including reversal, reset and changing tuning/clamps.
This validates those equations, **not the whole running game**.

`cargo run -p ose-sim --example control_trace` emits a repeatable 50 Hz vacuum trace
with press/release/reversal/roll/SAS transitions. `tools/compare_control_traces.py`
compares matching CSV captures with explicit angle, rate and mass tolerances; it
rejects mismatched timestamps or applied inputs. It does not align away initial-state
errors. Running-KSP2 captures are still required before claiming whole-craft 1:1 feel.
No executable reference captures have been fabricated or substituted with OSE runs.

Resource delivery still uses OSE's same-tick partial-delivery model rather than the
reference's pending-request handshake. Existing surface/contact/airflow approximations
can also change observed response. These are tracked fidelity boundaries, not closed
by passing the numerical controller or stock-craft regressions.

Exporter setup: the project Python environment needs UnityPy, NumPy and SciPy
(`scipy.spatial.ConvexHull` is used for convex collision meshes). Run
`python tools/export_flight_geometry.py` after extracting the catalog, then
`python -B -m unittest discover -s tools -p test_flight_geometry.py`.
The .NET fixture generator needs the .NET 9 SDK and the local decompiled reference;
reference C# files stay under gitignored `reference/`/`.cache/` and are not redistributed.

`tools/ksp2-control-capture/` was built and run in the installed KSP2 0.2.2.0.32913,
with user authorization, in a separate temporary sandbox campaign. F8 records passively;
F9 explicitly prepares a 100 km circular orbit, disables SAS/cuts throttle input, then
feeds pitch/release/reversal/roll/yaw through the stock input override API. It does
not ignite engines or write a save. Capture data includes actual applied inputs,
orientation/angular velocity, position/velocity, mass, runtime MOI and surface geometry.

Each useful pulse fixture has 600 samples at the nominal 20 ms interval. Unity
Z reflection uses quaternion [-x,-y,z,w], polar position/velocity [x,y,-z] and axial
angular velocity [-x,-y,z]. The native example accepts a reference CSV as its second
argument and initializes from its first pose/state, then replays every recorded input.
There is no fitted time/pose offset, mass correction or inertia calibration.

The baseline executable comparison at `abcfc19` **fails**: the captured Jumping Flea starts at
2.598231454 t versus OSE's 2.5031 t, and OSE rotates substantially faster. The roughly
95 kg difference includes the omitted occupied-seat mass; source inspection also
found RigidbodyBehavior's enabled inertia-tensor scaling absent from the imported
collider model. Procedural surfaces also have runtime coefficients/offsets different
from their prefab defaults. These are measured gaps, not closed parity claims.
Full joint/PhysX sample-phase equivalence is also unverified. See the recorder README
for capture conditions and cleanup requirements.

Measured numeric fixtures and an explicit Unity-frame comparison option are
committed with the recorder. Cleanup is complete: temporary loader, plugin and
campaign removed; the original campaign was verified unchanged.

The mass/inertia increment adds occupied-seat mass (0.095 t per Kerbal, including
the reference IVA pack mass), keeps it synchronized with the flight and joint
solvers, and restores it after loading a saved structure. Per-instance `PartFlight`
data retains measured dry mass, mass centre, unscaled collider tensor and moving
surface geometry. Measurements for Jumping Flea and Aeris-K2 are stored separately
from catalogue defaults. Stock re-import checks the original assembly fingerprint;
legacy saves recover known stock part measurements by stable part GUID/type.
Modified or unknown parts keep their supplied geometry/defaults.

The imported inertia settings reproduce RigidbodyBehavior's cached unscaled tensor,
inverse-mass scaling, detached single-part rule and low-mass scalar. Scaling changes
the part's own inertia, not the mass used by the parallel-axis term. Physical inertia
uses each part COM. SAS receives the separate origin-translated MOI reported by
PartOwnerComponent; the captured Flea verifies both quantities independently.

The [PhysX 4.1 integrator](https://raw.githubusercontent.com/NVIDIAGameWorks/PhysX/4.1/physx/source/lowleveldynamics/src/DyBodyCoreIntegrator.h)
advances each body's angular velocity without its own gyroscopic term. Summing
constrained bodies therefore leaves the gyroscopic contribution from the joint
lever masses. Imported reference bodies now use that aggregate equation. The
physical path retains full angular-momentum integration and its conservation
regression. A single-body reference regression checks constant world spin about
non-principal axes; it is deliberately different from physical free precession.
The aggregate joint approximation is supported by the source and runtime trace,
but is not a replacement for measuring flexible-joint transients.

Moving and fixed lift now retain their separate pressure points and local airflow.
Surface actuation follows the reference force-before-deflection update order;
new deflection affects the next force tick. Authored mirrored-surface signs are
preserved through stock imports and editor mirror placement. Their tests retain
both force-location and one-tick ordering assertions.

The mass/inertia increment at `e8f04ec` replayed those 600 samples with zero mass error, maximum angular
rate error 0.2222502 rad/s (baseline 9.5071410), and maximum attitude error 8.147963
degrees (baseline 179.819182). RMS attitude error is 6.020884 degrees. The original
0.5-degree / 0.01-rad/s thresholds remain unchanged, so **the comparison still fails**.
See `jumping-flea-after-mass-inertia.json` beside the baseline fixture. Remaining
work at that point included exact capture/module/PhysX phase and resource-request timing,
additional procedural shapes and joint-solver behavior. No time/pose offset or
controller gain was fitted to the recording.

Reaction-wheel resource acceptance now retains each authored ElectricCharge
`AcceptanceThreshold` (29 catalogue parts). Resource requests below that minimum
are rejected without consuming charge. When a partial delivery is accepted,
Module_ReactionWheel gates its full interpolated torque on the acceptance boolean;
it does not scale torque by the normalized delivered amount. OSE now follows that
rule. Legacy/custom electric definitions without an authored threshold retain
proportional delivery. Tests cover the boundary, retained charge after rejection,
and a lower-priority consumer using that charge. This closes the acceptance rule;
the asynchronous pending-request/module update timing remains unverified.

Source inspection also confirms that PartOwnerComponent caches its physics stats:
OnLateUpdate marks them dirty, and OnFixedUpdate refreshes them only when dirty.
The recorder's FixedUpdate samples can therefore repeat cached angular velocity
across physics ticks. Existing fixtures and failed comparisons are retained as
recorded; their phase has not been corrected by shifting or fitting the data.

The 0.2 recorder captures a live rigid-body state before each physics tick and its
end state after `WaitForFixedUpdate`, with the interval's actual input, rendered
frame, physics tick, requested torque and wheel power flags. It supplies the stock
one-shot overrides before every fixed tick, so multiple physics ticks in one
rendered frame no longer create unintended input dropouts. Live states use actual
Rigidbody COM, mass-weighted velocity/angular velocity and the control owner's
body rotation composed with its authored control offset. Cached vessel telemetry
remains a separate diagnostic. See Unity's [execution order](https://docs.unity3d.com/2020.3/Documentation/Manual/ExecutionOrder.html).

`jumping-flea-interval.csv` is the new executable capture. All 600 intervals have
positive 20 ms durations, consecutive physics counts and exactly continuous live
end/start angular velocity. Cached end-state velocity differs from live velocity
by up to 0.112767 rad/s. The intended holds/release/reversal occur without the old
per-render input dropouts. The source CSV, numerical geometry companions, original
metadata, hashes and before/after reports are preserved; GUID fields are omitted.
The temporary loader/campaign were removed and all 75 Default files retained
their original hashes.

The runtime wheel path now retains the preceding command, filtered demand,
resource acceptance and queued local torque. The source module receives the
preceding FlightInputHandler state; its resource component requests the preceding
filtered demand and observes accepted delivery later; RigidbodyBehavior applies
the existing torque object before the module replaces it. The live trace confirms
these stages. Accepted partial requests still permit full filtered torque. Staging
and structural rebuilds retain these transient queues. Tests distinguish paid,
delayed torque from new requests rejected on an empty battery. Legacy/custom
definitions without a source threshold retain their immediate proportional path.
The fixed-tick request schedule is an approximation to KSP2's render-dependent
component update schedule; it is not validated for every frame rate or brownout.

For imported reference bodies, the beginning-of-tick frame now supplies the tensor
and mounted-force transformation for the velocity solve. The joint lever-mass
gyroscopic estimate is solved implicitly in that frame. Orientation advances once
using the resulting angular velocity, matching the post-solver pose order in
PhysX's `integrateCore`. The physical path retains its midpoint/conservative
integration. A single-body impulse/pose regression independently checks the order.

On this same new capture, `213248c` gives maximum attitude error 12.770470 degrees
and maximum rate error 0.6142401 rad/s. The staged-wheel/pose increment gives
**0.145481 degrees, 0.0024561 rad/s and zero mass error**, passing the original
0.5-degree / 0.01-rad/s / 1e-6-t thresholds. A Rust integration test replays all
600 intervals in the normal test suite. This is one SAS-off, unignited vacuum
fixture; it does not establish SAS/RCS, powered atmospheric or whole-game parity.

The next mode/authority increment retains both preceding global control commands
and raw manual inputs, then applies the current wheel mode and authority when
the module updates. Switching ManualOnly/SASOnly or changing authority therefore
does not reuse a command already scaled for the previous setting. Disabling torque
or turning SAS off in SASOnly pauses the source response filter and clears the next
torque object; an already queued physical torque still completes its tick. The
stored raw inputs and queue survive structural rebuilds. Source-only regressions
cover pause/resume, precision-mode switching and disable/re-enable with authority.

SAS now gets the wheel `ITorqueProvider` potential separately from physical actuator
limits. The source excludes ManualOnly wheels from that potential and advertises
the rated torque independently of the authority/toggle controls. Tests check both
the provider values and that equal advertised torque produces equal controller
tuning while different physical limits still bound the torque request. Other
providers retain their existing potential estimate; these wheel mode cases still
need dedicated executable SAS-enabled captures.

RCS input deadbands, full-thrust snapping and the strict post-precision request
threshold compare at the source's single-precision boundaries. This includes a
double just below 0.05 that rounds to the source deadband boundary and a nominal
0.2 input compared with an authored float 0.2 full-thrust threshold. Small values
that round to the 0.05 request cutoff do not fire or consume fuel. The comparisons
are explicit; the simulation retains double precision for its dynamics.

Source RCS definitions now carry Module_RCS.RequestPropellant's normalized minimum
delivery of 0.5. The five stock RCS definitions import and retain this rule. A
delivery below half the requested amount is rejected without drawing any mixture
ingredient, so retained fuel may still support a smaller later pulse. Accepted
partial delivery scales the originally commanded nozzles in that module. It is
not redirected to another module to improve the requested force/torque result.
Source-directed mixed assemblies also preserve the broadcast nozzle pattern;
legacy-only definitions keep the preceding bounded reallocation behavior.

The same acceptance gate applies when source data uses the older transform/mount
fallback. Nozzle resolution is selected consistently so these paths cannot charge
fuel twice or bypass the minimum. Tests cover the 50% boundary, retained gas and
smaller retries, both transform paths, independent starvation in two modules, and
all-ingredient retention when a mixture request is rejected. Definitions without
the new optional rule deserialize to their legacy behavior.

The subsequent 0.3 recorder run provides two executable 600-interval RCS captures
from a single Gumball, one occupied seat, SAS/wheels off. Their per-nozzle data
confirms the shared-handle behavior: the final positive command in authored
transform order replaces preceding commands, while each commanded nozzle applies
its own force. Source definitions now charge that final command; legacy definitions
retain summed demand. Unequal-nozzle and reversed-order tests distinguish the rule
from summing or choosing the largest request. The recorded two-nozzle flow is also
checked directly without an OSE self-comparison.

The same capture's maximum mass error falls from 0.001300139 t to 0.0000339914 t.
Its maximum angle/rate errors remain 4.22574 degrees / 0.0150627 rad/s, so this is
still a failing RCS fixture. Translation comparisons now also reject different
inputs/enable states and check position and velocity. The reference samples input
in render updates, consumes a pending request later and applies a queued force on
the next physics interval. First activation starts with no accepted request;
subsequent activations can reuse prior delivery. Cached vessel mass lags actual
connected-tank consumption. The simulation's current same-tick delivery does not
match these stages. No input/time/pose offset or controller gain was fitted.
The SAS-off reaction-wheel fixture remains a separate passing check. Capture
cleanup removed the owned loader and campaign and verified all 75 Default files.
