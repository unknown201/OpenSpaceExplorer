# Physics and flight work

Updated 2026-10-27. Implementation following the [feature gap audit](../ksp2-feature-gap-analysis.md).

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

## Integrated with current main; completed: rotational airflow

Integration onto upstream `fec09e4` preserves its manual steering over SAS holds, trim, restored craft orientation, aero overlays, directional crash destruction and debris. An added regression verifies the manual override, trim and restored orientation with part-backed inertia; the shielding test also checks overlay forces against the actual applied forces. Upstream also supplies EVA ladders, resource/crew transfers and chained maneuvers that were absent from the original research baseline.

Each aerodynamic application point now sees `v_air + (omega_craft - omega_atmosphere) x (point - current mass centre)`. Landed craft follow the rotating surface. Drag cubes, parachutes, fairing bases and lifting surfaces use the local flow direction, dynamic pressure, Mach number and pseudo-Reynolds input. This produces passive aerodynamic damping during rotation, including with zero translational airspeed. The summed vector drag reaches translation directly; it is no longer reconstructed from one scalar area and the centre's velocity. Moments, structural loads, the aero overlay and wing effects share these forces. Airless transitions clear retained part loads and overlays.

Reference evidence: `KSP.Modules/Module_LiftingSurface.cs::CalculateLiftDragForces` uses `PartBehavior.VelocityRelativeToSOIBodyFrameInPhysicsSpace`; `PartBehavior.GetVelocityRelativeToSOIBodyFrame` obtains the part's motion relative to the rotating body frame. This increment uses rigid-body point kinematics instead of independent per-part motion. **Local dynamic pressure and Mach are a physical refinement, not a claim of exact reference arithmetic:** `PartComponent.OnFixedUpdate` shares vessel dynamic pressure and Mach for vessel-owned parts. Authored bounds centres remain approximate force points, density/sound speed are uniform over the craft, structural flex velocity and wind are omitted, and heating/control-authority estimates still use vessel-level airflow. Imported centre-of-pressure offsets, lifting-surface options and actual surface deflection forces remain open.

Four new tests verify analytical wing damping and effect strength, local Mach/drag and nonpositive mechanical power, rotation covariance, vacuum/shielding removal, co-rotating atmosphere behavior, and monotonically decreasing free-tumble energy across 500 integration steps. Several assertions share tests.

## Completed increment: part impacts, actuator bounds, buoyancy and suspension

Part-backed airborne craft now sweep the eight corners of each attached part's current bounds against terrain and built-site height fields. Contacts use the rolled attitude and current fuel-weighted mass centre. A sequential accumulated-impulse solve includes point velocity from spin, rotational effective mass, inelastic normal response and Coulomb friction. Simultaneous corners share the solve so a centered touchdown does not acquire artificial spin. An edge impact retains its angular impulse and continues tumbling; only slow supported craft enter the existing parked/rolling path. The remainder of a touchdown step advances with the post-contact velocity and spin, followed by a bounds penetration correction.

Damage checks use the struck part's own tolerance rather than the upright clearance proxy's selected feet. Extended gear retains the normal-speed exception; other bounds use full point-relative speed. A broken contact absorbs only a tolerance-based share of normal impact energy, allowing the surviving hull to hit the next layer instead of losing all incoming speed. Reaction impulses become per-part average forces for that tick, with application offsets, and enter the joint-load path even on a craft that has just become landed. Buoyancy and suspension use that same load path.

The controller now allocates its requested torque among bounded part channels. Wheel pitch/roll/yaw limits are imported independently, rotated through their mounts and scaled by power; 29 wheel-bearing entries in the committed catalog now carry those authored axes. Three parts have unequal axes, including the 2v M2 probe's 15/3/3 kN·m pitch/roll/yaw limits. Old catalogs retain a scalar fallback. Each gimbal shares a transverse input disc bounded by its angle and the engine's delivered thrust, including a final partial burn. Its axial thrust decreases under deflection to preserve the fuel-resolved thrust magnitude. A centered axial engine therefore cannot borrow gimbal authority for roll. RCS channels have a shared force bound at their mount, and control surfaces provide bounded mounted force channels in air. Structural forces and wheel torques use the allocated parts rather than synthesizing a stage-wide wheel/gimbal split. Wheel charge demand follows control effort, including braking, rather than spin speed.

On ocean bodies, the solid height query uses the actual seabed. The waterline is the body's nominal radius: the imported `oceanAltitude` field is the PQS terrain datum, already included in terrain-map offsets, and must not be added again. Each part's bounds are clipped against the local water plane to obtain submerged volume and centroid. Density × displaced volume × gravity gives buoyancy at that centroid, including its turning moment. Quadratic water drag is limited implicitly per part. Light hulls settle at a displacement-dependent depth; hulls heavier than their maximum displaced water mass sink. Water cannot enter the parked long-step optimization.

Extended gear now develops a non-tensile spring/damper force as its bounds compress into the support surface, with a hard stop after the imported suspension travel. The provisional spring reaches twice its distributed support weight at full travel and uses critical damping with an implicit force update. Compression is retained per part for inspection. Retracting a leg removes its support force. Parked/rolling craft still use their existing support/drive approximation. Enabling real bounds also exposed an aircraft layout mismatch: hangar layouts now place the belly, gear, and bounds in the same right-handed frame as the level flight basis.

Thirteen new simulation regressions and an importer regression cover symmetric and edge impacts, energy loss, joint reaction loading, per-part damage and residual motion, fast falls and intermediate ridges, exact clipped volume/centroid, floating versus sinking, suspension drop recovery and retraction, shared gimbal limits and conserved thrust magnitude, mounted wheel axes/power/detachment, and final-partial-burn gimbal availability. The roll gameplay regression checks commanded roll rate because the stored roll angle wraps; the tilted-landing regression now permits the physical tumble before rest.

This is a bounds-based increment, not mesh-contact or original-game parity. Corner paths still use the bounded two-metre/256-sample sweep and linear interpolation of the endpoint pose; thin terrain features, edges/faces between corners, and rapid rotational arcs can be missed. No nearby-vessel/debris pair solver or equal-and-opposite inter-vessel impulse exchange is implemented. Bounds can overlap and overestimate displaced volume, especially for rotated or hollow parts; there are no authored displacement multipliers, flooding, waves, planing or water-impact damage. Ground friction, water drag and spring/damper tuning are provisional. Wheel collider transforms, spring/damper ratios, target positions, individual supported-mass distribution, tyre slip, load-dependent grip and suspension animation are not imported. RCS nozzle geometry and joint translation/rotation resource budgeting still use prior approximations; RCS availability is updated after flight and can lag a resource change by one tick. Reaction-wheel electrical delivery is covered by the later increment below. Control-surface channels are a linear normal-force approximation without deflected drag/stall modeling. Engine nozzle transforms remain open. Structural wet-mass/inertia synchronization is covered by the following increment; the thrust-lag filter remains.

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

This closes reaction-wheel charge timing, not all actuator-resource timing. Partial electrical delivery limits the existing planned wheel inputs; the remaining demand is not reallocated in a second optimization. Shared charge requests retain ordered resource-graph semantics. Alternator output still follows the existing throttle/fuel-availability estimate, and solar output retains its existing orientation approximation. The subsequent RCS increment below closes part-backed RCS timing and shared mount budgets. Imported RCS nozzle geometry, wheel dynamics/saturation and physical reaction-wheel momentum storage are not implemented.

## Completed increment: fuel-resolved RCS mount budgets

RCS translation and rotation now share each mount's bounded three-axis force vector. Translation is added to the torque allocation in mounted axes and the combined vector is projected to that mount's force bound. Each mount requests its own propellant recipe at its own resource-graph node before integration. Vacuum thrust / (vacuum Isp * standard gravity) sets mass flow; the full pressure curve changes thrust, including above one atmosphere. Delivered fuel scales the same force and moment used by flight and structural loading. The post-flight aggregate RCS request is removed. A dry mount cannot borrow fuel or Isp from a different RCS definition, and a last partial burn cannot grant an extra empty-tank tick. Balanced translational forces now keep the structural solver active even with zero net torque.

The reference equations come from `KSP.Modules/Module_RCS.cs::GetMaxFuelFlow` and `CalculateThrust`, and `Data_RCS.cs::OABGetMaxFuelFlow`. The reference resource broker reads its previous delivery; OSE deliberately resolves delivery before applying motion to avoid granting an unbudgeted impulse. This is a resource-accounting correction rather than reproduction of the reference's asynchronous scheduling.

Five added regressions cover independent propellants/Isp, full pressure curves, missing and final partial fuel, proportional recipe shortfall at an isolated mount, the shared translation/rotation bound, group disabling/detachment, analytical linear impulse and mass accounting, the universe loop and structural translation loads. The existing K2 gameplay test remains in the focused RCS suite.

Nozzle geometry is still a three-axis mount proxy. Its shared sphere is a conservative approximation; the reference fires individual directional nozzles with separate lever arms. Partial delivery is not reallocated to other actuators, and ordered graph requests still determine access to shared supplies. RCS effects retain the existing vessel-level directional readout. EVA jetpacks retain their separate resource path.

## Completed increment: authored engine thrust transforms

`tools/export_physics_geometry.py` reads the installed part prefabs and the extracted `Data_Engine.engineModes` definitions. Its committed `assets/own/physics-geometry.json` covers 36 parts, 41 modes and 63 thrust points. `Module_Engine.SetThrustTransforms` defines the selection: default transform names share thrust equally across matching transforms; named weighted entries use their first matching transform and authored multiplier. Force points opposite the transform's forward axis. Export removes the prefab root's editor position/rotation, preserves scale in positions, and composes normalized rotations independently of scale for force directions. Missing or ambiguous geometry fails the export.

The importer validates and attaches the geometry to each engine mode, and the slim catalog carries it. Flight sums each delivered engine's weighted nozzle forces and their individual moments. Structural engine loads retain those same individual application offsets. Thus the Twitch/Puff's canted thrust, clustered engines, and reverse-thrust jet mode geometry are represented in data. Only the existing selected/default mode is simulated; this does not add mode switching. Older catalogs retain the +Y/origin fallback.

The existing bounded gimbal proxy now uses the engine's resultant direction and force-weighted nozzle centroid rather than the part origin. This is exact for its static parallel-nozzle force distribution, but remains an approximation for divergent nozzles, independent gimbal pivots and articulated nozzle motion. Structural thrust still passes through the existing lag filter. The bounds-based mass/contact geometry has not been replaced by exact colliders.

Two simulation tests verify independently calculated canted/weighted forces and moments after mounting rotation, unchanged fuel flow, and complete committed-mode coverage with Cornet/Twitch prefab fixtures. An importer regression rejects stale mode counts and invalid directions and checks older catalogs. Two exporter tests cover editor-pose removal, scale-independent direction, default equal shares, explicit first-match weights and missing transforms. Aircraft/truck support regression checks now use the full Unity-to-craft-to-world frame, including Z mirroring; the level layout retains the corresponding belly correction.

Reproduce the extraction and import with:

```powershell
.venv/Scripts/python tools/export_physics_geometry.py --data assets/ksp2/data/parts
cargo run -p ose-importer -- --data assets/ksp2/data --physics-geometry assets/own/physics-geometry.json
```

## Completed increment: directional RCS nozzle geometry

The geometry exporter also reads each prefab's serialized `Module_RCS.ThrusterTransforms` references, preserving their order and individual positions. It uses negative transform up for force, as in `Module_RCS.ProcessThrust`, rather than guessing by node name. The committed `assets/own/rcs-geometry.json` covers all five authored modules and 22 nozzles, including eight on each crew pod, four on the block and one on each linear thruster. The importer validates unit directions and per-nozzle weights and keeps legacy catalogs readable.

Authored nozzles get separate force channels with inputs bounded to 0..1. A nozzle cannot reverse or provide an absent axis. Its translation and rotation demands share that one input, clamped at full thrust. Resource demand sums the individual firing strengths at each part before delivery, including opposing forces whose translation cancels. Delivered fractions scale those same forces and moments and their individual structural offsets. Only catalogs without geometry retain the previous per-mount sphere proxy. Rated RCS usage now includes all nozzle ratings.

Four new simulation tests cover one-way torque allocation, one-way translation with zero/quarter/full fuel, application moments, shared rotation/translation saturation, fuel for an opposing-nozzle couple, and complete catalog coverage. An importer regression checks the 22 nozzle definitions and legacy fallback. The exporter reference-selection regression verifies serialized pointer order, negative-up direction and failure on unresolved external references. Existing mixed-propellant, full pressure-curve and universe impulse tests remain green.

This closes static RCS geometry and force/fuel bounds. The torque allocator is still OSE's bounded optimizer rather than the reference's rotational-direction/lever projection and precision-mode logic. Per-part pitch/yaw/roll/translation switches, thrust percentage, shielding/underwater activation rules, dynamic nozzle poses and per-nozzle visual readouts remain to be matched. Shared-resource requests remain ordered; partial delivery is not reallocated to different actuators. EVA propulsion is covered by the later increment below.

## Completed increment: authored part mass centres

The catalog now carries each part's `coMassOffset` as `mass_offset_m`, with an origin default for older catalogs. All 400 entries reproduce the installed source; 14 are nonzero. `PartComponent` initializes its centre from this offset, and `PartBehavior.StartPhysX` gives that centre and total mass to its rigid body. Accordingly, the offset applies to each part's total wet mass, rather than creating a separate dry-mass offset and leaving fuel at the geometry origin.

Flight mass centres and parallel-axis inertia use the mounted offset. Structural bodies use the same local centre when built and when resource changes update their masses. Geometry origins, joint anchors and contact bounds remain at their authored placements. Structural centre/velocity aggregation and Newton-Euler loads now distinguish mass centres from geometry origins. Force moments subtract the actual mounted local centre; applying a force through that centre cannot create a false spin.

Two simulation regressions independently calculate a rotated part's centre and force lever, preserve its pose/velocity through mass updates, and check analytical multi-part centre/inertia changes under added fuel and detachment. An importer regression checks coordinate import and old-catalog fallback. Existing analytical box-contact fixtures explicitly reset inherited real-part mass offsets to zero. The stock aircraft shallow-impact fixture now starts just above support: a release from ten metres adds a hard gravity-driven sink and does not isolate a two-metre-per-second touchdown. Ground control checks require visible deflection and verify that reversing every input reverses its surface response; they do not assume a fixed lever-dependent angle.

Inertia still uses the existing oriented-box proxies and size clamps rather than authored collider tensors. Structural motion still uses the 1 kg minimum for synthetic massless parts. Internal resource locations, changing deployed mass distribution and correction of the vessel reference position as its mass centre shifts remain open. This increment supplies authored centres and consistent levers, not complete mass/inertia parity.

## Completed increment: authored EVA propulsion and fuel delivery

EVA settings are imported from `Data_Kerbal` into the slim catalog, passed into each campaign, and preserved on the flight state. The installed values are 0.25 kN move force, 0.125 kN*m rotation torque, a 0.25 m rotation lever, a 280-degree/second angular-rate cap and 200-degree/second reset speed. These last two values differ from the class defaults. The full Isp curve is 240/100/0.001 seconds at 0/1/4 atmospheres, with its authored Hermite tangents. The existing dry-mass and pack-capacity constants match the installed 0.07 t and 0.025 t values.

`JetpackResourceManager.Initialize` sets move flow to `move force / (Isp(vacuum) * g0)` and rotate flow to `(rotation torque / rotation lever) / (Isp(vacuum) * g0)`. Its pressure-dependent exhaust velocity scales force and torque. OSE now resolves both requests from the one pack before motion. A final partial delivery scales both channels, including automatic braking; an empty pack cannot provide a free force or arrest a spin. Walking and jumping do not run the pack. Throttle and fore translation combine into one bounded axis input rather than paying for/applying the same move twice. There is no second post-flight drain for configured EVA packs.

Rotation uses the reference's angular-rate controller rather than pretending the pack is a reaction wheel: requested torque divided by the pre-step mass sets the rate, then delivered fuel and the authored rate cap bound it. The existing attitude-hold UI requests bounded pointing; manual override updates a stability hold. Inertial orientation is retained as the orbital/local roll reference changes. Old EVA saves acquire the configured settings and corrected Isp metadata without refilling or changing their saved fuel/mass.

Eight added simulation regressions cover full pressure interpolation and pressure-independent mass flow, combined translation/rotation delivery at zero/quarter/full fuel, subsequent dry ticks, fuel-limited braking, idle spin with reset disabled, the authored rate cap, shared throttle/translation input, jumping suppression, manual hold override, inertial frame continuity, universe-level mass/impulse accounting and old-save migration. The related grounded-actuator check catches a separate error: landed part-backed craft must apply their already-resolved control forces before deciding whether to lift off. One importer test pins the installed jetpack definition and checks authored field conversion.

This improves source/data parity and removes unbudgeted actuation. The camera-relative character input, camera alignment, separate upright/forward character controllers, water drag, pack-equipment/thrust-percentage controls and pack refill/transfer behavior remain incomplete. Partial fuel is shared proportionally between movement and rotation, rather than reproducing the original asynchronous broker's previous-tick scheduling and its all-or-nothing reset behavior. The older unconfigured walker fallback remains for callers that construct a bare flight state directly; spawned/restored EVA Kerbals use the configured path.

## Completed increment: authored aerodynamic settings, force points and body lift

`tools/export_aero_settings.py` now reads the reference's loaded `Global/PhysicsSettings.json` (settings version 0.1.16). Its committed `assets/own/aero-settings.json` contains the source SHA-256, five aerodynamic multipliers, seven drag curves and all four named lifting-surface families, including authored Hermite tangents. Runtime drag/lift calculations load this data rather than duplicating class-default tables. Weighted, non-clamped, invalid or unordered curves are rejected by the exporter. Joint and thermal settings are outside this export.

All 400 part definitions now carry core pressure/lift offsets and drag/body-lift enabling and multiplier fields. The 13 lifting-surface modules retain their own pressure offset, perpendicular-only option and internal-drag switch; control surfaces retain their authored pressure offset. Regeneration from the original JSON reproduces every field within 1e-14. Named SpeedBrake and CapsuleBottom families no longer silently fall back to Default; Custom families retain their own curves.

Body drag, lifting-surface force and drag-cube body lift retain separate application points and structural loads. Each samples its own rotational airflow, pressure and Mach. Body lift uses exposed face areas, facing-dot lift coefficients, authored drag factors and the global BodyLift Mach curve, projected perpendicular to travel. Core pressure/lift and module offsets follow the source's distinct force paths. Control allocation, deflection readouts and authority use the imported control point. Disabled, shielded and detached parts cannot contribute the corresponding force.

Six additional simulation regressions cover independently calculated rotated force points and moments, zero-centre-airspeed rotational flow at different points, perpendicular lift/internal-drag options, asymmetric-cube body lift with Mach/multiplier/visibility checks, loaded settings and named speedbrake behavior. Two importer regressions cover offsets, flags and legacy defaults; two exporter tests preserve reference tangents and reject unsupported curves.

Procedural surfaces still generate a runtime CoP/CoM that static imports do not reproduce. Actual deflected-surface lift/drag, stall, deployed drag-cube geometry and node-enabled attachment occlusion remain open. The existing actuator model supplies a bounded surface-force proxy. No matched executable flight traces are claimed.

## Completed increment: RCS module activation and thrust limits

The five authored RCS modules now import their serialized enable state, thrust percentage and shielded-activation permission. `Module_RCS.ProcessThrust` gates operation on module enable and shielding; `CalculateThrust` multiplies fuel flow by thrust percentage. OSE applies those same gates before allocation and scales both the force channels and fuel requests by the bounded module thrust fraction. A disabled, zero-limit or impermissibly shielded module cannot consume fuel, contribute torque authority or supply translation. Limited thrust also reaches the aggregate rating, delivered-power and usage readouts. Older catalogs default to enabled/full thrust and disallow shielded operation.

A parameterized analytical simulation regression checks both authored one-way nozzles and legacy mount proxies: full/quarter/zero limits, enable toggling, shield blocking and explicit permission, bounded out-of-range limits, final partial fuel, per-mount isolation and fuel/readout consistency. Importer coverage checks serialized property values and older catalog defaults. The settings are currently simulation/catalog configuration; player-facing per-part editing and persistence of changed flight settings remain open, alongside axis switches, fore-by-throttle, full-thrust threshold and reference precision/lever selection.

## Completed increment: consistent mounted quaternion normalization

The rigid-body path already normalized craft rotations, but force axes, aerodynamic frame transforms, rotated contact boxes and hangar layout used raw imported quaternions. 300 of the 352 committed stock part rotations differ from unit length by more than 1e-12; the largest squared-norm error is 5.364424335851936e-7. These rounding differences previously gave force/contact calculations a slightly different pose from their structural body. Scaled uploaded quaternions could cause much larger disagreement.

A shared craft-quaternion normalization method now supplies force mounts, contact boxes, hangar rotation and structural orientation. Existing zero rotations retain the identity fallback; nonfinite rotations also fall back to identity. A data regression covers normalization and invalid legacy values. An analytical simulation regression checks scaled, sign-flipped and rounded representations of a 90-degree mount: expected unit axes, engine forces and offsets, contact bounds, wet mass centre and inertia, rotational aerodynamic force/torque, hangar layout and rigid rotation of craft positions agree. This makes existing geometry paths consistent; it does not replace their bounding-box/contact or inertia approximations.

## Completed increment: RCS per-module command switches

All five RCS definitions now retain the serialized pitch/yaw/roll and X/Y/Z translation switches, fore-by-throttle setting, full-thrust setting and threshold. Rotation switches filter allocation requests in craft control axes while retaining the complete force and incidental physical torque of each fired nozzle. Disabling yaw cannot erase a yaw moment caused by a nozzle requested for another enabled axis. Translation switches map the reference's X/Z/Y controls into OSE's right/fore/top craft axes before mounted-nozzle projection.

The source 0.05 translation deadband and explicit fore/aft priority over throttle are honored. Authored nozzle inputs retain the source's per-axis command bounds before projection; diagonal translation is not reduced by an extra vessel-wide normalization. Full-thrust mode promotes a combined nozzle request at/above the authored threshold before resource delivery. The boundary uses source single-precision comparison semantics. Final partial fuel still scales the resulting full-thrust request, and the legacy sphere fallback retains its shared mount bound. Planned and direct integration allocations use the same options.

Three additional simulation tests cover each rotation switch and analytical force/moment/fuel, throttle priority/deadband and translation disabling, full-thrust threshold and partial delivery, and preservation of incidental physical torque. Importer coverage checks switch mapping and old-catalog defaults. All five imported definitions match regenerated installed data. Player-facing per-part controls and persistence of changed runtime settings remain open, as do precision/lever-based reference nozzle selection and the original SAS controller. The bounded optimizer is retained; these switches do not establish complete RCS control parity.

## Completed increment: loaded joint settings and authored full-strength wing connections

`tools/export_joint_settings.py` exports ten joint stiffness, damping and break multipliers from the same loaded reference settings JSON as the aerodynamic export, preserving its version and SHA-256. Runtime joints now consume this committed data. Stack and surface attachment stiffness remain independent, and rigid-connection break multipliers are applied independently to force and torque.

The importer now retains `MultiJointFullBreakStrength` on all attachment nodes. Nine authored size-2 surface nodes set it: the three procedural wing, stabilizer and control-surface families. The stock aircraft use these nodes. Their aggregate connection strength now avoids the ordinary large-node divisor of three, matching `PartOwnerBehavior.GetJointBreakLimits`; previously those connections were three times too weak. Old catalogs default to the ordinary size divisor.

The multibody solver still represents each connection with one aggregate link. Splitting equal capacity across N source joints and summing cancels N; this change does not reproduce the source's separate off-axis joint anchors or unequal individual loads. Translational constraint stiffness, exact multi-joint geometry, launch-clamp exceptions and all collider/inertia details remain open. The existing zero-authored-strength convention remains unchanged.

Two simulation regressions independently exercise distinct stack/surface/rigid multipliers and verify real stock aircraft wing limits. Importer coverage pins the nine authored nodes and legacy defaults. Two exporter tests check source identity, separate multipliers and invalid/missing settings. All 986 regenerated node flags match the committed definitions. Strict workspace Clippy and the browser build pass; the complete workspace/Mun run is in progress.

## Completed increment: same-tick resolved structural thrust

Part-backed structural loading now receives the exact engine force already delivered for the current flight step. The previous independent 0.4-second structural filter suppressed the first impulse and could retain a decaying engine load during subsequent rotating/aerodynamic steps after fuel exhaustion. Nozzle response belongs in the physical propulsion calculation, applied consistently to flight and structure; it cannot be simulated only by delaying joint loads.

An isolated-body analytical regression checks the complete structural momentum change from a last quarter-step burn against delivered propellant times exhaust velocity. The multibody depletion regression now requires the exact delivered engine load and verifies zero engine load on the next dry tick while structural stepping remains active. All 184 simulation tests, strict workspace Clippy and the browser build pass.

Legacy stage-only callers without resolved per-engine delivery retain their response proxy. This change does not implement authored engine spool/tail-off or thrust curves; those still need to affect the physical propulsion result before both consumers. Structural contacts, body inertia and joint geometry retain their documented approximations.

## Remaining physics priorities

1. Import usable collider inertia geometry and procedural/deployed mass changes; complete articulated engine gimbals. Match RCS precision/lever allocation and part controls, and complete EVA character/camera/water behavior.
2. Generate procedural surface CoP/CoM and deflected control-surface lift/drag; match stall, deployed drag cubes and node-enabled occlusion.
3. Replace sampled corner paths with continuous hull contacts; add nearby vessel/debris pair contacts and equal-and-opposite impulses. Import contact materials and sliding damage/heat.
4. Import displacement and wheel-collider data, suspension tuning and target positions; add load-dependent tyre grip and wheel slip. Extend water behavior, then docking attraction/capture and EVA ladder/hatch contact.

The next actuator acceptance cases should compare the reference's lever/precision and axis-enabling rules for directional nozzle selection. Retain the mixed-propellant, empty/partial-fuel, shared-budget, battery and per-mount delivery cases. Retain the EVA shared-pack, braking and migration acceptance cases while adding character/camera/water behavior.

The next contact acceptance cases should cover continuous hull contacts that prevent tunnelling between vertices, equal-and-opposite vessel/debris impulses, and stable support/traction over uneven terrain. Retain the existing slope, normal-speed, sliding-friction, angular-impulse and joint-reaction regressions.

Per-engine mode switching, articulated nozzle transforms, thrust response curves/limits, full intake accounting, and the complete KSP2 engine modifier set are still open. The existing jet curve/intake approximation remains. Resolved part-backed structural thrust is now immediate; the separate lag survives only for legacy stage-only callers. The resource allocator also retains its existing ordered-request semantics. This change closes specific propulsion errors; it does not establish complete engine or flight parity.

## Validation, 2026-10-28

- RCS command switches: `cargo test --workspace --exclude ose-bot` passes with 181 simulation tests, 18 importer tests and WebSocket coverage. Browser build passes. Installed-data regeneration matches all five modules. Strict workspace Clippy, formatting and whitespace checks pass. The full Mun journey was last run on the aerodynamic milestone.


- Mounted-rotation follow-up: `cargo test --workspace --exclude ose-bot` passes with 178 simulation tests, 18 data tests, 17 importer tests and the WebSocket suite (19 passed/one existing ignored). Strict workspace Clippy, browser build, formatting and whitespace checks pass. The full Mun journey was last run on the aerodynamic milestone.


- RCS activation follow-up: strict workspace Clippy passes; focused RCS tests pass (10 simulation, two importer). `cargo test --workspace --exclude ose-bot` passes (177 simulation, 17 importer, WebSocket suite 19 passed/one existing ignored). The browser check passes and all five RCS module defaults match a fresh installed-data regeneration. The full Mun journey passed on the preceding aerodynamic milestone.


- Aerodynamic milestone: full `cargo test --workspace` passed, including the fresh-server Mun journey (504.80 s), 174 simulation tests and 16 importer tests. The settings-loader follow-up adds two simulation tests (176 total); focused simulation/importer tests pass. Strict workspace Clippy and the browser-client check pass. The final `cargo test --workspace --exclude ose-bot` run also passes with all 176 simulation tests, 16 importer tests and the WebSocket suite (19 passed, one existing ignored test).
- Both aerodynamic-exporter tests pass; all 400 regenerated aerodynamic definitions match the committed catalog within 1e-14.


- `cargo test --workspace`: passed on the authored-mass-centre follow-up, including the fresh-server Mun journey (508.60 s), server WebSocket tests and 162 simulation regressions. The existing ignored test remains ignored.
- EVA follow-up: `cargo test --workspace --exclude ose-bot` passed, including 170 simulation tests, 14 importer tests and the server WebSocket suite. The existing ignored test remains ignored. The full Mun journey was last run on the mass-centre milestone; the bot was compiled/linted but that long journey was not repeated for this EVA-only continuation.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo check -p ose-client --target wasm32-unknown-unknown`: passed.
- `python -m unittest discover -s tools/tests -p test_export_physics_geometry.py`: 3 passed.
- Real installed-data regeneration: 41 engine modes and 22 RCS nozzle transforms reproduce the committed geometry; maximum JSON round-trip difference is below 5e-16. All 400 mass offsets also match the original JSON. Jetpack curve/tangents, rotation settings, mass and fuel capacity match the installed `eva_kerbal` definition in both committed and regenerated catalogs. The existing malformed `Tutorial1.3_Save.json` is warned and skipped by the importer; this does not affect part geometry.
- `cargo fmt --all --check` and `git diff --check`: passed.

These are source/data comparisons, analytical fixtures and automated gameplay regressions. They do not certify matched flight/contact traces against the KSP2 executable. The remaining priorities above still prevent a complete physics parity claim.
