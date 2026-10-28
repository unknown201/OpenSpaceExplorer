# Physics and flight work

Updated 2026-10-28. Implementation following the [feature gap audit](../ksp2-feature-gap-analysis.md).

## Completed: fuel-resolved per-engine propulsion

The previous flight path derived one thrust and Isp estimate from the stage report, interpolated between vacuum and one atmosphere, advanced motion, then split the burned mass between engines by rated thrust. Resource shortfalls were discovered after thrust had already been applied. One jet's air multiplier could affect the whole stage. Structural loads also rechecked fuel after consumption, so a final partial burn could disappear from the load calculation.

The server now resolves the engines independently before integrating motion:

1. Evaluate each attached, activated stage engine's vacuum Isp and its full pressure curve at the vessel's atmospheric pressure.
2. Compute its requested mass flow from vacuum thrust / (vacuum Isp Ã— standard gravity), throttle, and that engine's air multiplier.
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

The existing cubic-Hermite curve evaluator is used, without the old 0â€“1 atmosphere clamp. Solid engines retain their throttle lock. The default imported engine mode remains the selected mode. The legacy stage estimate is retained only for flight states without a part structure, such as simplified probes/EVA and old state.

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

The second increment removes the assumption that every engine pushes along the vessel axis. Each engine's delivered thrust is rotated from its part-local +Y axis into the right-handed craft frame. Net force is the vector sum; net torque is the sum of `(part origin - mass centre) Ã— force`. Craft roll participates in the conversion to inertial space. On the pad, an attitude command no longer redirects part-backed thrust before the craft has physically turned.

Mass properties are rebuilt from attached dry masses and current tank contents before each propulsion step. The flight inertia tensor includes oriented box inertia and the parallel-axis contribution from each part. Fuel drainage and detachment therefore change both the mass centre and resistance to rotation.

Part-backed attitude integration advances inertial angular momentum with a midpoint orientation solve and angular substeps. A torque-free asymmetric tumble conserves angular momentum while angular velocity can change direction and magnitude. SAS and pilot inputs request torque within the existing control-authority budget; external torque can exceed that budget and spin a craft faster than the commanded turn-rate limit. Pitch, yaw and roll share this update. Moving along an orbit changes the local roll reference without rotating the craft's inertial orientation.

Structural engine loads use the mounted directions. Actuator torque is passed separately from total angular acceleration, so an engine's external moment is not recreated as reaction-wheel or gimbal torque.

Six additional analytical tests cover rotated-box inertia, balanced and angled engine mounts, mass-centre shifts from fuel drainage and detachment, roll-dependent force and angular impulse, limited SAS authority against roll torque, and conservation of momentum/energy during asymmetric tumbling. The existing SAS-free integration test now checks angular momentum with engines off; the maneuver-hold fixture explicitly provides wheel torque instead of depending on zero-inertia instant steering.

This is not a complete rigid-body/contact replacement. The force application point remains the part origin and direction remains local +Y, pending import of the original `Module_Engine` thrust transforms and weights. Flight inertia uses the structure's existing box proxies (dimensions clamped to 0.1â€“5 m), with fuel distributed across its tank box; exact collider inertia and internal mass offsets are unavailable. Control authority is still a scalar stage estimate, including approximate gimbal authority on each axis. The structural solver still has its original mass/inertia approximations and thrust lag. These limits require further work before claiming reference parity.

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

## Completed increment: part impacts, actuator bounds, buoyancy and suspension

Part-backed airborne craft now sweep the eight corners of each attached part's current bounds against terrain and built-site height fields. Contacts use the rolled attitude and current fuel-weighted mass centre. A sequential accumulated-impulse solve includes point velocity from spin, rotational effective mass, inelastic normal response and Coulomb friction. Simultaneous corners share the solve so a centered touchdown does not acquire artificial spin. An edge impact retains its angular impulse and continues tumbling; only slow supported craft enter the existing parked/rolling path. The remainder of a touchdown step advances with the post-contact velocity and spin, followed by a bounds penetration correction.

Damage checks use the struck part's own tolerance rather than the upright clearance proxy's selected feet. Extended gear retains the normal-speed exception; other bounds use full point-relative speed. A broken contact absorbs only a tolerance-based share of normal impact energy, allowing the surviving hull to hit the next layer instead of losing all incoming speed. Reaction impulses become per-part average forces for that tick, with application offsets, and enter the joint-load path even on a craft that has just become landed. Buoyancy and suspension use that same load path.

The controller now allocates its requested torque among bounded part channels. Wheel pitch/roll/yaw limits are imported independently, rotated through their mounts and scaled by power; 29 wheel-bearing entries in the committed catalog now carry those authored axes. Three parts have unequal axes, including the 2v M2 probe's 15/3/3 kN·m pitch/roll/yaw limits. Old catalogs retain a scalar fallback. Each gimbal shares a transverse input disc bounded by its angle and the engine's delivered thrust, including a final partial burn. Its axial thrust decreases under deflection to preserve the fuel-resolved thrust magnitude. A centered axial engine therefore cannot borrow gimbal authority for roll. RCS channels have a shared force bound at their mount, and control surfaces provide bounded mounted force channels in air. Structural forces and wheel torques use the allocated parts rather than synthesizing a stage-wide wheel/gimbal split. Wheel charge demand follows control effort, including braking, rather than spin speed.

On ocean bodies, the solid height query uses the actual seabed. The waterline is the body's nominal radius: the imported `oceanAltitude` field is the PQS terrain datum, already included in terrain-map offsets, and must not be added again. Each part's bounds are clipped against the local water plane to obtain submerged volume and centroid. Density × displaced volume × gravity gives buoyancy at that centroid, including its turning moment. Quadratic water drag is limited implicitly per part. Light hulls settle at a displacement-dependent depth; hulls heavier than their maximum displaced water mass sink. Water cannot enter the parked long-step optimization.

Extended gear now develops a non-tensile spring/damper force as its bounds compress into the support surface, with a hard stop after the imported suspension travel. The provisional spring reaches twice its distributed support weight at full travel and uses critical damping with an implicit force update. Compression is retained per part for inspection. Retracting a leg removes its support force. Parked/rolling craft still use their existing support/drive approximation. Enabling real bounds also exposed an aircraft layout mismatch: hangar layouts now place the belly, gear, and bounds in the same right-handed frame as the level flight basis.

Thirteen new simulation regressions and an importer regression cover symmetric and edge impacts, energy loss, joint reaction loading, per-part damage and residual motion, fast falls and intermediate ridges, exact clipped volume/centroid, floating versus sinking, suspension drop recovery and retraction, shared gimbal limits and conserved thrust magnitude, mounted wheel axes/power/detachment, and final-partial-burn gimbal availability. The roll gameplay regression checks commanded roll rate because the stored roll angle wraps; the tilted-landing regression now permits the physical tumble before rest.

This is a bounds-based increment, not mesh-contact or original-game parity. Corner paths still use the bounded two-metre/256-sample sweep and linear interpolation of the endpoint pose; thin terrain features, edges/faces between corners, and rapid rotational arcs can be missed. No nearby-vessel/debris pair solver or equal-and-opposite inter-vessel impulse exchange is implemented. Bounds can overlap and overestimate displaced volume, especially for rotated or hollow parts; there are no authored displacement multipliers, flooding, waves, planing or water-impact damage. Ground friction, water drag and spring/damper tuning are provisional. Wheel collider transforms, spring/damper ratios, target positions, individual supported-mass distribution, tyre slip, load-dependent grip and suspension animation are not imported. RCS nozzle geometry remains approximate; same-tick resources and combined translation/rotation budgeting are covered by the later RCS increment. Reaction-wheel electrical delivery is covered by the later increment below. Control-surface channels are a linear normal-force approximation without deflected drag/stall modeling. Engine nozzle transforms remain open. Structural wet-mass/inertia synchronization is covered by the following increment; the thrust-lag filter remains.

## Completed increment: structural wet mass and stable small-angle SAS

Structural bodies now take each attached part's dry mass plus its current tank contents during flight-state synchronization and again before structural loading. Rapier bodies receive updated local box inertia in place; joints, poses, velocities, bends and settling timers are preserved. Fuel leaving at a part's velocity carries away its own momentum instead of accelerating the remaining structure through an artificial velocity rescale. Resource changes from engines, RCS and converters therefore reach the structural solver before joint loads are evaluated.

Rapier 0.36.1 leaves body velocity readbacks at the beginning of its final substep while its generalized multibody velocities contain the completed step. The structural solver now maps those completed velocities through each body's Jacobian before Newton-Euler differencing. This removes the apparent extra impulse when transitioning from thrust to coast and makes engine-mount loads agree with the analytical current-mass share.

Changing structural mass exposed a small-angle SAS limit cycle in the K1 launch regression. The requested pointing rate now stops at the remaining angle divided by the step duration, and an aligned craft brakes its spin without selecting an arbitrary correction axis. The antiparallel fallback remains available for a 180-degree turn.

Three new regressions cover repeated asymmetric fuel removal from a translating multibody, no artificial coast acceleration or joint loads, analytical acceleration and mount force after depletion, spin preservation on mass loss, updated inertia after refilling, and high-authority SAS convergence at 0.01/0.02/0.08-second timesteps. The existing fuel/detachment regression now checks structural tank mass and centre synchronization too.

The solver retains its 1 kg minimum per body; synthetic massless parts can therefore differ from flight mass properties. Fuel still occupies the existing part-origin box proxy. Imported internal offsets, collider inertia, structural thrust response and resource-before-actuation remain open. This increment does not import new geometry or establish original-game parity.

## Completed increment: same-tick per-wheel electrical delivery

The part-backed flight loop now advances electrical producers, converters, probe cores and drive motors before resolving propulsion and control. It plans actuator inputs without integrating motion, requests each reaction wheel's charge at its own mount using the magnitude of that wheel's requested torque vector, and scales that wheel's planned inputs by the delivered fraction. Integration and structural loading consume the same frozen allocation. There is no second post-flight electrical draw.

Dry wheels are removed from the control request before allocation so they cannot take priority over a usable gimbal or control surface. A wheel that receives a last partial charge supplies the corresponding partial angular impulse in that tick. A newly powered wheel can act immediately, without waiting for an old vessel-wide power fraction to recover. Idle spin draws no wheel charge; SAS braking draws according to applied control effort. Electrical flow modes and attachment state are honored independently at each wheel. Converters' mass changes are synchronized before propulsion; drive and probe power eligibility also reach the current flight step.

Three new regressions check the full universe loop with zero, quarter and full-step charge, no extra impulse on the subsequent empty tick, isolated per-mount delivery, detached wheels, same-tick generator recovery, allocation handoff to a non-electric force channel, analytical multi-axis charge use, idle spin and SAS braking.

This closes reaction-wheel charge timing, not all actuator-resource timing. Partial electrical delivery limits the existing planned wheel inputs; the remaining demand is not reallocated in a second optimization. Shared charge requests retain ordered resource-graph semantics. Alternator output still follows the existing throttle/fuel-availability estimate, and solar output retains its existing orientation approximation. RCS resource timing and the shared mount budget are covered by the following increment. Imported RCS nozzle geometry, wheel dynamics/saturation and physical reaction-wheel momentum storage are not implemented.

## Completed increment: same-tick per-mount RCS fuel and combined demand

Part-backed RCS now resolves its propellant before integration. Each attached mount uses its own propellant recipe, vacuum mass flow and full pressure/Isp curve, with resource access rooted at that mount. A dry mount contributes no force or torque; its final partial fuel produces a proportional force and moment immediately. Stale vessel-wide availability no longer gates a newly supplied mount. Consumed RCS mass joins the engine burn total and is subtracted once; the post-flight RCS request has been removed.

The current mounted force-sphere proxy now combines translation and rotational channel inputs and normalizes their sum to one shared unit sphere per mount. The resulting vector supplies both translational force and its moment about the mass centre, and its magnitude determines one resource request. Opposing requests may cancel; simultaneous demands cannot independently claim full thrust or fuel. Flight integration and structural loading use the same delivered channel inputs. Landed translation also passes through this allocation. Actuator force integration now uses the same pre-burn mass as engine force integration.

Four analytical regressions cover empty/quarter/full-step fuel and the next empty tick through the universe loop, independent propellants and Isp at separate mounts at five atmospheres, local NoFlow access and detachment, combined translation/rotation saturation and cancellation, and a mixture shortfall scaling force and torque together without overdraining the other ingredient. The universe-loop fixture also checks that pure translation loads joints even with no engine thrust or angular motion. The existing stock K2 RCS gameplay check remains in place.

This closes the structured-vessel RCS resource timing and shared mount budget gap. It retains approximate bidirectional force spheres at part origins, not imported individual nozzle geometry. Translation is combined after torque allocation, so saturation can reduce the commanded turn and asymmetric translation can introduce an uncompensated moment. There is no second allocation pass after partial delivery. Requests retain the resource graph's ordered semantics. EVA and other unstructured flight states retain their existing simplified path.

## Remaining physics priorities

1. Import engine/nozzle transforms and weights, internal mass offsets and usable inertia geometry. Extend the combined RCS allocation to authored individual nozzles and compensate translation-induced moments where authority permits.
2. Add local airflow from angular motion, aerodynamic damping, centre-of-pressure offsets, full lifting-surface options, and deflected control-surface lift/drag.
3. Replace sampled corner paths with continuous hull contacts; add nearby vessel/debris pair contacts and equal-and-opposite impulses. Import contact materials and sliding damage/heat.
4. Import displacement and wheel-collider data, suspension tuning and target positions; add load-dependent tyre grip and wheel slip. Extend water behavior, then docking attraction/capture and EVA ladder/hatch contact.

The next actuator acceptance cases should check authored RCS nozzle directions and transforms, translation-induced moment compensation, and allocation recovery after partial resource delivery. Retain the mixed-propellant, empty/final-partial-fuel, combined-budget, battery, per-mount delivery and wheel-impulse cases. Plan control demand before integration, resolve requests once through the resource graph, and apply only the delivered channels; do not reconsume resources after flight.

The next contact acceptance cases should cover continuous hull contacts that prevent tunnelling between vertices, equal-and-opposite vessel/debris impulses, and stable support/traction over uneven terrain. Retain the existing slope, normal-speed, sliding-friction, angular-impulse and joint-reaction regressions.

Per-engine mode switching, imported nozzle transforms, thrust response curves/limits, full intake accounting, and the complete KSP2 engine modifier set are still open. The existing jet curve/intake approximation and structural thrust-lag filter remain. The resource allocator also retains its existing ordered-request semantics. This change closes specific propulsion errors; it does not establish complete engine or flight parity.

## Validation, 2026-10-27

- `cargo test -p ose-sim --lib`: 133 passed.
- `cargo test --workspace`: passed on the same-tick per-wheel electrical continuation, including the fresh-server bot campaign through Mun landing (462 seconds) and the server WebSocket tests. The existing ignored test remains ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo check -p ose-client --target wasm32-unknown-unknown`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.

These are analytical and automated gameplay regressions, not a measured comparison of flight traces against the KSP2 executable.
