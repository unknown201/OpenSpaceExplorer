# Remaining physics gaps

Updated 2026-10-28. This is the checklist of **known** physics limitations found in the current simulation, importer, save path and [physics progress notes](reference-notes/physics-flight-progress.md). It is not a claim that an executable-to-executable parity audit has found every possible difference. Closed increments and their regression evidence remain in the progress notes. Scope follows [ADR-0001](ADR-0001-initial-decisions.md).

Status: **open** = missing behavior or unverified approximation; **partial** = some listed behavior is implemented; **implemented** = the named gap has analytical regression coverage, with remaining fidelity limits stated separately. Rows describe observable completion criteria, not percentage estimates. Stable IDs retain completed rows so follow-up work can refer to them. The order below is the working order; geometry and force consistency support the later contact, wheel and docking work.

## Actuators, resources and mass

| ID | Status | Remaining gap | Acceptance evidence required |
| --- | --- | --- | --- |
| P01 | Partial | Authored engine force-point positions, directions and weights are imported. Articulated gimbal transforms and complete runtime nozzle response remain open. | Retain 41-mode/63-point geometry fixtures; compare articulated off-axis gimbal force and moment with reference limits under mounting rotation and roll. |
| P02 | Partial | Authored RCS nozzle geometry and module axis/enable/limit/full-thrust switches are implemented. Reference precision/lever selection and runtime part controls remain open. | Retain 22-nozzle and five-module fixtures; validate reciprocal perpendicular-lever scaling, precision factors and runtime setting persistence. |
| P03 | Open | RCS translation is combined after torque allocation. Saturation can reduce turning and translation-induced moments are not compensated. Partial resource delivery is not followed by reallocation. | Feasible translation holds attitude on an asymmetric craft; infeasible commands saturate deterministically, conserve force/fuel budgets and recover when another actuator is available. |
| P04 | Partial | Internal mass offsets are imported and update wet centres/inertia. Usable collider inertia, procedural/deployed mass changes and fuel distribution remain approximate. | Retain offset/depletion/refill fixtures; validate compound collider inertia, articulated mass changes and nonuniform tank distribution. |
| P05 | Open | Engine mode switching, response curves, thrust limits and the full engine modifier set are incomplete. Resolved part-backed structural thrust is immediate; authored spool/tail-off must still affect propulsion and structure together. | Mode-specific propellant/Isp/thrust, ignition/cutoff response and joint loads agree with reference fixtures, including partial final burns. |
| P06 | Open | Jet/intake flow is approximate rather than a complete per-intake supply/request model. | Independent intake supply, atmosphere composition, speed/pressure curves, starvation and mixed rocket/jet operation consume only delivered resources. |
| P07 | Open | Resource requests retain ordered graph semantics rather than demonstrated reference allocation parity. Wheel partial-charge requests are scaled without reallocating the residual control demand. | Shared scarce resources, priorities, crossfeed, fuel lines and multiple consumers match reference fixtures; no double consumption or free impulse. |
| P08 | Open | Reaction wheels have torque bounds but no physical rotor momentum storage, saturation or desaturation. | Wheel acceleration exchanges equal/opposite momentum, saturation limits sustained torque, and desaturation transfers momentum through a physical external actuator. |
| P09 | Open | Alternator output uses throttle/fuel eligibility; solar orientation remains approximate. Producer power is not fully tied to delivered propulsion and articulated panel normals. | A starved engine cannot provide free alternator output; rolled/deployed/tracking panels and shadows give expected same-tick power. |
| P10 | Partial | EVA now resolves shared-pack movement/rotation and braking before motion. Character/camera/water behavior and legacy-state fidelity remain incomplete. | Retain shared-pack partial-fuel, braking and migration fixtures; compare character motion, cameras, swimming and contact transitions with reference behavior. |

The structured-vessel per-engine fuel path, same-tick wheel electricity, authored combined RCS nozzle budgets and EVA shared-pack resolution are implemented. P01-P10 describe their remaining geometry, allocation and fidelity limits, not absence of those systems.

## Aerodynamics and thermal behavior

| ID | Status | Remaining gap | Acceptance evidence required |
| --- | --- | --- | --- |
| P11 | Implemented | Per-part `v + omega_relative cross r` drives drag/lift, local Mach/dynamic pressure, aerodynamic damping and structural loads. Authored static force points are retained; procedural geometry remains P12/P22. | Regressions cover a zero-resultant damping couple at zero bulk speed, angular impulse/energy loss and joint load, local Mach, roll covariance, fuel-dependent levers, shielding/detachment, vacuum and atmospheric co-rotation. |
| P12 | Partial | Authored force offsets, lift options, body lift, named-node gating and static named lifting-reference frames are implemented. Runtime procedural CoP/CoM and animated reference-frame updates remain open. | Retain rotated/scaled reference-frame, missing-name fallback, independent cube-offset, projection and node-gate regressions. Next validate per-frame procedural/deployed geometry and mass/pressure-centre changes. |
| P13 | Open | Control surfaces remain bounded linear normal-force channels rather than deflected lift/drag/stall geometry. | Deflection changes local aerodynamic force and drag with the correct axis, range, moving share and stall behavior; authority/readouts agree with forces. |
| P14 | Implemented | Heating, condensation and control authority now evaluate each part's local flow. Rotating thermal occlusion uses conservative projected box outlines; full thermal/geometry fidelity remains P15/P16/P22. | Regressions check local-Mach heating at zero bulk speed, roll equivalence, shielding, wing FX/force agreement and nonzero control authority on spinning surfaces. |
| P15 | Open | Thermal exchange, reentry heating, conduction, radiation and ablation remain approximations, without measured reference flight traces. | Energy accounting and material/area scaling pass controlled fixtures; reentry and heat-shield traces are compared with the original executable. |
| P16 | Partial | Loaded aerodynamic settings/curves are imported. Shielding/occlusion and deployed-shape behavior still need full geometry and executable comparisons. | Retain version/hash and curve-tangent exporter fixtures; compare bays, fairings, deployments and drag traces across Mach/Reynolds regimes. |
| P17 | Open | Parachute opening, safe deployment and tear-off are approximate shape blending and threshold behavior. | Imported deployment/safety parameters, multi-chute loading and partial inflation produce matched loads and recovery traces. |

## Contacts, structures and interacting vessels

| ID | Status | Remaining gap | Acceptance evidence required |
| --- | --- | --- | --- |
| P18 | Open | Continuous hull contacts: the present two-metre/256-sample corner sweep can miss thin terrain, faces/edges between corners and rapid rotational arcs. | Translating and spinning hulls cannot tunnel through terrain, pads or facilities between vertices; first-contact time and normal remain stable under timestep subdivision. |
| P19 | Open | Nearby vessel/debris collision pairs and equal/opposite inter-vessel impulses are absent. | Head-on, grazing and spinning encounters conserve pair momentum, dissipate energy within material rules and load/damage the struck parts. |
| P20 | Open | Imported friction/restitution/material data and sliding abrasion damage/heating are missing; ground coefficients are provisional. | Materials produce expected static/sliding transitions, restitution and frictional work; persistent sliding damages/heats the correct contact parts. |
| P21 | Open | Parked/rolling support still uses clearance/footprint and snap-to-rest attitude approximations after slow support. | Uneven ground supports or tips a hull from its actual contacts, with stable resting attitude and no energy-adding snap. |
| P22 | Open | Collision shape coverage and deployed geometry are bounds-based; exact collider meshes/compound hulls and deployment collision transitions are incomplete. | Rotated/deployed wings, panels, legs and fairings collide at authored geometry without phantom volume or sudden penetration impulses. |
| P23 | Partial | Joint settings and authored full-strength wing connections are imported; resolved structural thrust is immediate. Aggregate multibody links, constraint stiffness, minimum body mass, box inertia and structural/flight coupling remain approximate. | Retain loaded-settings, wing-strength, exact structural-impulse and wet-mass regressions; compare individual joint anchors, constraint response and failure traces. |
| P24 | Open | Contact and structural fidelity across vessel switching, SOI changes, save/reload and multiple nearby players needs dedicated regression coverage. | The same physical interaction produces continuous position, spin, resources and contact state through these lifecycle transitions. |

## Wheels, suspension and water

| ID | Status | Remaining gap | Acceptance evidence required |
| --- | --- | --- | --- |
| P25 | Open | Wheel collider transforms, spring/damper ratios, targets and supported-mass distribution are not imported. Current extended-gear springs and damping are provisional. | Individual suspension travel/load/compression follows authored parameters through asymmetric drops, slopes, retract/deploy and changing vessel mass. |
| P26 | Open | Load-dependent tyre grip, slip, rolling resistance, drive/brake torque and suspension animation remain simplified. | Acceleration, braking, steering, wheel unloading and skids behave consistently on flat/uneven ground and across step sizes. |
| P27 | Open | Buoyancy uses clipped boxes that can overlap or overestimate hollow/rotated displacement; authored displacement multipliers are missing. | Compound/hollow hulls have the correct displaced volume, centroid, equilibrium depth and righting moment with no double counting. |
| P28 | Open | Flooding, waves, planing and water-impact damage are missing; water density/drag tuning is provisional. | Float/sink, asymmetric flooding, splashdown damage and high-speed surface motion match controlled material and reference fixtures. |

## Docking, EVA, orbital integration and continuity

| ID | Status | Remaining gap | Acceptance evidence required |
| --- | --- | --- | --- |
| P29 | Open | Docking attraction/capture needs physical port geometry, collision and momentum behavior beyond current capture/merge rules. | Misaligned/fast contacts fail or bounce, valid capture exchanges momentum consistently, and undocking preserves assembly mass/resources/spin. |
| P30 | Open | EVA ladder, hatch and boarding contact are simplified; walking retains its proxy path. | Walking on slopes, ladder attachment, hatch clearance, boarding and ejection work with nearby moving hulls without teleporting through geometry. |
| P31 | Open | Finite burns and maneuver prediction/execution still need a full physics-based path; scheduled burns use impulsive changes. | Low-thrust/long burns consume real resources over time, predict the finite trajectory and report remaining burn/delta-v accurately. |
| P32 | Open | Active physics is limited to 4x; higher-rate propulsion, contact and orbital/physics transitions need stability and conservation evidence. | Documented supported warp regimes remain stable and continuous for burns, free rotation, contacts and transitions to/from rails. |
| P33 | Open | Full transient structural/contact state restoration and lifecycle continuity are not established. Saving the craft, current tanks, crew and docked joints already exists. | Save/reload preserves physical attitude/spin, deployed state, damage, thermal state, suspension and joint/contact behavior or explicitly reconstructs a stable equivalent. Do not report structure saving itself as absent. |
| P34 | Open | Reference parity evidence is primarily analytical and automated gameplay, not measured original-executable force/contact/flight traces. | Record reproducible original-game fixtures and compare toleranced traces for propulsion, spin, aero, joints, landing, tyres, water and docking. |

## Working validation gates

Every increment should state which rows it closes and which approximations remain. Preserve the fuel-limited engine, wheel and RCS regressions; banked lift/shielding; angular momentum and mass synchronization; slope/friction/contact impulses; suspension and float/sink cases. Add analytical fixtures before broad gameplay runs, then run the simulation suite and relevant integration checks. The Mun bot is the long flight continuity gate. A passing test suite is not by itself a reference-parity claim.

Implementation entry points: [structure](../crates/ose-sim/src/structure.rs), [flight](../crates/ose-sim/src/flight.rs), [actuators](../crates/ose-sim/src/actuators.rs), [surface contacts](../crates/ose-sim/src/surface.rs), [resources](../crates/ose-sim/src/resources.rs), [thermal](../crates/ose-sim/src/thermal.rs), [docking](../crates/ose-sim/src/docking.rs), [rigid structure](../crates/ose-sim/src/rigid.rs), [save state](../crates/ose-server/src/save.rs), and [gap audit](ksp2-feature-gap-analysis.md).
