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

`ose-sim/src/sas.rs` keeps the controller separate from the integration and allocation code.
Unity angular axes and rotations are reflected into OSE's right-handed physics frame.

## Verification and remaining parity boundary

Tests check the reference PID step values, autotune breakpoints, manual override and release,
and small-angle torque values at three timesteps. Integration regressions check resource bounds,
free angular momentum, steering direction, pointing modes, braking and eventual attitude hold.
The previous monotonic-convergence and fixed-roll-rate expectations were replaced because those
were properties of the removed controller, not of the reference PID algorithm.

This is a port of the input/SAS core, not proof of identical whole-game flight response. OSE still
uses its bounded actuator allocator, proxy RCS geometry, approximate control-surface forces,
box inertia, and delivered-thrust gimbal cone. Reaction-wheel torque response and resource
accounting, physical gimbal response, authored asymmetric limits, selectable control points,
and all per-module enable/authority settings are not yet reference-equivalent. The renderer's
gimbal smoothing is separate from authoritative actuation. Those differences affect controller
authority and therefore flight traces even when the SAS equations match.

Bit-exact or whole-craft 1:1 parity has not been established. That requires matching actuator
modules and comparing identical craft/input/timestep traces against the running reference.
