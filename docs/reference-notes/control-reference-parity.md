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
The mounted response direction is found from physical force derivatives; the full
reference control-mixing/deployment algorithm and runtime raster-derived procedural
moving areas are **still approximations**. Separately deployed grid fins are not
completed by this increment. Neither this nor the collider import is a claim of
identical runtime procedural geometry.

Flight PAM supports wheel enable/authority/input mode, gimbal enable/travel/axis flags,
RCS enable/authority/axis flags, surface authority/axis flags/inversion, and thrust
limits. Command and docking parts expose Control from here. Selection rotates the
control frame without changing position or spin; detached control points reset. All
settings/control-frame data survive save/load and the server validates ownership,
attachment and finite bounds.

Gamepad/joystick axes are polled through the browser Gamepad API. Settings persist
axis assignment/inversion, dead zone, response exponent and optional throttle axis.
Disconnects and missing axes produce zero rotation. Keyboard precision and trim still
apply. Button mapping, device-specific hardware calibration and reference RCS lever
precision mode remain outside this increment.

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
