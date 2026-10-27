# Physics and flight work

Updated 2026-10-27. Implementation following the [feature gap audit](../ksp2-feature-gap-analysis.md).

## Completed: fuel-resolved per-engine propulsion

The previous flight path derived one thrust and Isp estimate from the stage report, interpolated between vacuum and one atmosphere, advanced motion, then split the burned mass between engines by rated thrust. Resource shortfalls were discovered after thrust had already been applied. One jet's air multiplier could affect the whole stage. Structural loads also rechecked fuel after consumption, so a final partial burn could disappear from the load calculation.

The server now resolves the engines independently before integrating motion:

1. Evaluate each attached, activated stage engine's vacuum Isp and its full pressure curve at the vessel's atmospheric pressure.
2. Compute its requested mass flow from vacuum thrust / (vacuum Isp × standard gravity), throttle, and that engine's air multiplier.
3. Request its mixture through the existing resource graph. A missing ingredient reduces all ingredients and thrust by the same delivered fraction.
4. Sum delivered thrust and subtract the actual tank mass change exactly once from the flight state.
5. Pass each engine's delivered thrust to structural load targets, including a partial step that empties its tank.

The HUD's current thrust, available full-throttle TWR, and engine-effect eligibility use the same per-engine demand model. A dry engine or a jet without air cannot appear in the current thrust estimate merely because another engine is running. This remains a current-state readout; it does not promise a full future tick of thrust from an almost-empty tank.

The local KSP2 reference used was `KSP.Modules/Data_Engine.cs::SetConsumptionRate` and `KSP.Sim.impl/PartComponentModule_Engine.cs::RequiredPropellantMass`, plus its propellant-delivery handling. The governing equations are:

```
requested mass flow = rated vacuum thrust / (Isp(vacuum) * g0) * throttle * flow multiplier
requested thrust    = requested mass flow * Isp(pressure) * g0
delivered thrust    = requested thrust * delivered resource fraction
```

The existing cubic-Hermite curve evaluator is used, without the old 0–1 atmosphere clamp. Solid engines retain their throttle lock. The default imported engine mode remains the selected mode. The legacy stage estimate is retained only for flight states without a part structure, such as simplified probes/EVA and old state.

Code: [per-engine demands and resolution](../../crates/ose-sim/src/structure.rs), [integration](../../crates/ose-sim/src/flight.rs), [universe loop](../../crates/ose-sim/src/lib.rs), [flight readout](../../crates/ose-server/src/main.rs).

## Regression evidence

Six added tests cover:

- Two equal-thrust engines with 100 s and 200 s Isp consume fuel at a 2:1 rate.
- A curve with 100/80/20 s at 0/1/5 atm produces the expected thrust at 5 atm and at an interpolated 3 atm, while pressure alone does not alter mass flow.
- A tank holding only a quarter of the requested fuel produces a quarter-step impulse; its final structural load is retained and the next burn produces zero thrust.
- A rocket continues firing when a jet in the same stage has no air, without draining the unlit jet's fuel.
- Ignition, throttle lock, and engine detachment gate propulsion correctly.
- The full universe loop produces the analytical fuel-limited velocity increment, and flight/tank mass agree without a second fuel drain. A separate timestep-splitting assertion checks equal fuel and impulse through exhaustion at 0.08 s versus four 0.02 s steps.

The first two bullets share one test. These are independent analytical fixtures and integration checks, not live measurements from the original game's renderer or executable.

Run the focused checks with `cargo test -p ose-sim propulsion --lib` and `cargo test -p ose-sim universe_applies_only --lib`. The workspace tests also exercise stock launches, aircraft takeoff, staging, reentry, docking, and the protocol-driven Mun journey.

## Completed: mounted forces and rotational dynamics

The second increment removes the assumption that every engine pushes along the vessel axis. Each engine's delivered thrust is rotated from its part-local +Y axis into the right-handed craft frame. Net force is the vector sum; net torque is the sum of `(part origin - mass centre) × force`. Craft roll participates in the conversion to inertial space. On the pad, an attitude command no longer redirects part-backed thrust before the craft has physically turned.

Mass properties are rebuilt from attached dry masses and current tank contents before each propulsion step. The flight inertia tensor includes oriented box inertia and the parallel-axis contribution from each part. Fuel drainage and detachment therefore change both the mass centre and resistance to rotation.

Part-backed attitude integration advances inertial angular momentum with a midpoint orientation solve and angular substeps. A torque-free asymmetric tumble conserves angular momentum while angular velocity can change direction and magnitude. SAS and pilot inputs request torque within the existing control-authority budget; external torque can exceed that budget and spin a craft faster than the commanded turn-rate limit. Pitch, yaw and roll share this update. Moving along an orbit changes the local roll reference without rotating the craft's inertial orientation.

Structural engine loads use the mounted directions. Actuator torque is passed separately from total angular acceleration, so an engine's external moment is not recreated as reaction-wheel or gimbal torque.

Six additional analytical tests cover rotated-box inertia, balanced and angled engine mounts, mass-centre shifts from fuel drainage and detachment, roll-dependent force and angular impulse, limited SAS authority against roll torque, and conservation of momentum/energy during asymmetric tumbling. The existing SAS-free integration test now checks angular momentum with engines off; the maneuver-hold fixture explicitly provides wheel torque instead of depending on zero-inertia instant steering.

This is not a complete rigid-body/contact replacement. The force application point remains the part origin and direction remains local +Y, pending import of the original `Module_Engine` thrust transforms and weights. Flight inertia uses the structure's existing box proxies (dimensions clamped to 0.1–5 m), with fuel distributed across its tank box; exact collider inertia and internal mass offsets are unavailable. Control authority is still a scalar stage estimate, including approximate gimbal authority on each axis. The structural solver still has its original mass/inertia approximations and thrust lag. These limits require further work before claiming reference parity.

## Completed: roll-aware airflow and per-part loads

The third increment passes the same craft basis used by propulsion and rendering to drag-cube evaluation, lifting surfaces, aerodynamic torque, heating, control-surface readouts and wing condensation readouts. The former facing-only basis discarded roll and could leave forces pointing in the old direction as a craft banked. Mounted part rotations are applied inside this shared frame.

Lifting-surface calculations now share one force evaluator. A shielded wing contributes neither lift nor induced drag, and is excluded from control-surface deflection and condensation readouts. Aerodynamic moments and control lever arms use the current fuel-weighted mass centre instead of the initial rigid-body mass centre.

The flight loop retains each part's aerodynamic force and its bounds-centre application offset for the structural solver. It no longer redistributes the combined lift and drag across unrelated parts by frontal area. Fairing-shell drag is assigned to the fairing base, so its force contributes to both structural load and aerodynamic moment. A set of opposing forces still loads the structure even when its net force is small.

Applying off-centre forces exposed a separate Rapier integration error: the solver cleared force accumulators each tick but left torque accumulators active. Torques are now cleared too. A single-body impulse regression checks that removing an offset force stops angular acceleration; the stock parachute recovery regression also passes with the corrected per-part loading.

Six new tests cover analytical banked lift and equivalent wing mounting rotations, agreement with wing-effect readouts, shielded force/control removal, fuel-dependent torque and retained force locations, anisotropic drag under roll, rotated heating equivalence, and clearing persistent torque. Related assertions share tests.

This increment retains the existing lift/drag curves and aerodynamic application-point approximations. Part-local airflow from angular motion, aerodynamic damping, imported centre-of-pressure offsets, the reference's `perpendicularOnly` lift option, control-surface force deflection and per-axis authority still need work. Heating remains the existing thermal approximation, now evaluated in the correct rolled frame. Contact, suspension and buoyancy behavior are unchanged.

## Completed: ground-normal impacts, sliding and sampled sweeps

Ground contacts now use the local normal of the terrain, ocean surface or built-site plane instead of assuming the normal is always radial. The response resolves incoming speed along that normal separately from collision damage. The local reference `KSP.Sim.impl/PartBehavior.cs::HandleCollisionEnter` compares full relative speed with `CrashTolerance`; `IsSurvivableCrash` additionally considers the other rigid body's mass. Ground/body damage retains the relative-speed check, while the existing intact-wheel sink-speed exception remains an approximation. A non-wheeled craft making a shallow impact retains tangential motion; an inelastic normal response and Coulomb friction remove energy without reversing the slip direction. Grounded non-walking craft continue sliding under gravity and thrust until friction stops them. Static friction holds shallow slopes; steep slopes overcome it.

Before landing, the flight proxy samples its movement segment and bisects the first detected surface crossing. This catches raised sites or terrain between clear movement endpoints and places contact at the first crossing. The contact location is then carried with planetary rotation for the rest of the tick. Distant flight skips the sweep using the terrain/site radius bound. The long-step parked optimization now requires negligible surface-relative velocity and stable support; unbraked wheels on an incline stay on physics steps. Touchdown clears both pitch/yaw angular velocity and roll rate.

Nine new tests cover friction energy/sign behavior, shallow/steep slope support, intermediate ridge detection and outward motion, site normals, a full flight step crossing a raised pad, grazing touchdown followed by sliding, sustained ground motion on slopes, high-speed grazing destruction under the reference damage rule, and parked-step eligibility for moving/unbraked craft.

This remains a contact-proxy improvement. The sweep samples every two metres up to 256 samples, then bisects a detected crossing; sufficiently narrow features can still be missed, especially once that cap is reached. It is not continuous mesh or hull collision detection. The craft still uses its existing upright clearance/footprint and snap-to-rest attitude. Contact does not yet generate off-centre angular impulses or distribute reaction forces through the joints. Friction coefficients (0.8 static, 0.6 sliding) are provisional, not imported KSP2 material values. Wheel rolling, EVA walking, wreck handling and ocean-as-ground behavior retain their previous approximations. Tangential sliding does not yet model abrasion damage/heating, and the remaining fraction of a touchdown tick carries the surface with the planet but defers sliding advance to the next tick.

## Integrated with current main; completed: rotational airflow

Integration onto upstream `fec09e4` preserves its manual steering over SAS holds, trim, restored craft orientation, aero overlays, directional crash destruction and debris. An added regression verifies the manual override, trim and restored orientation with part-backed inertia; the shielding test also checks overlay forces against the actual applied forces. Upstream also supplies EVA ladders, resource/crew transfers and chained maneuvers that were absent from the original research baseline.

Each aerodynamic application point now sees `v_air + (omega_craft - omega_atmosphere) x (point - current mass centre)`. Landed craft follow the rotating surface. Drag cubes, parachutes, fairing bases and lifting surfaces use the local flow direction, dynamic pressure, Mach number and pseudo-Reynolds input. This produces passive aerodynamic damping during rotation, including with zero translational airspeed. The summed vector drag reaches translation directly; it is no longer reconstructed from one scalar area and the centre's velocity. Moments, structural loads, the aero overlay and wing effects share these forces. Airless transitions clear retained part loads and overlays.

Reference evidence: `KSP.Modules/Module_LiftingSurface.cs::CalculateLiftDragForces` uses `PartBehavior.VelocityRelativeToSOIBodyFrameInPhysicsSpace`; `PartBehavior.GetVelocityRelativeToSOIBodyFrame` obtains the part's motion relative to the rotating body frame. This increment uses rigid-body point kinematics instead of independent per-part motion. **Local dynamic pressure and Mach are a physical refinement, not a claim of exact reference arithmetic:** `PartComponent.OnFixedUpdate` shares vessel dynamic pressure and Mach for vessel-owned parts. Authored bounds centres remain approximate force points, density/sound speed are uniform over the craft, structural flex velocity and wind are omitted, and heating/control-authority estimates still use vessel-level airflow. Imported centre-of-pressure offsets, lifting-surface options and actual surface deflection forces remain open.

Four new tests verify analytical wing damping and effect strength, local Mach/drag and nonpositive mechanical power, rotation covariance, vacuum/shielding removal, co-rotating atmosphere behavior, and monotonically decreasing free-tumble energy across 500 integration steps. Several assertions share tests.

## Remaining physics priorities

1. Import engine/nozzle transforms and weights, mass offsets and usable inertia geometry; replace scalar control authority with per-axis wheel, gimbal, RCS and control-surface limits, and keep structural mass properties synchronized with flight.
2. Import centre-of-pressure offsets and full lifting-surface options; implement force-based control-surface deflection and extend local airflow to heating and control-authority estimates.
3. Replace the sampled ground proxy with per-part contact geometry for terrain, facilities, debris and nearby vessels; resolve off-centre angular impulses and connect reaction forces to joint loads. Add imported contact materials and sliding damage/heat.
4. Implement displacement-based buoyancy and richer wheel/suspension/traction behavior, then docking attraction/capture and physical EVA ladder/hatch contact (upstream now includes ladder commands and climbing).

The next contact acceptance cases should cover off-centre impulses that turn the craft, continuous hull contacts that prevent tunnelling, and equal-and-opposite impulses for vessel/debris collisions. Contact forces should reach the same joint-load path as aerodynamic and engine forces. Retain the new slope, normal-speed and sliding-friction regressions while extending the model.

Per-engine mode switching, imported nozzle transforms, thrust response curves/limits, full intake accounting, and the complete KSP2 engine modifier set are still open. The existing jet curve/intake approximation and structural thrust-lag filter remain. The resource allocator also retains its existing ordered-request semantics. This change closes specific propulsion errors; it does not establish complete engine or flight parity.

## Validation, 2026-10-27

- Before integration: `cargo test -p ose-sim --lib` passed all 114 tests. Integration with upstream adds its regressions and the manual-control check; the rotational-airflow follow-up passes all 130 tests.
- `cargo test --workspace`: passed on both the integrated aerodynamic/contact implementation (`c328583`) and the rotational-airflow follow-up, including the fresh-server bot campaign through Mun landing and the server WebSocket tests. The existing ignored test remains ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo check -p ose-client --target wasm32-unknown-unknown`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.

These are analytical and automated gameplay regressions, not a measured comparison of flight traces against the KSP2 executable.
