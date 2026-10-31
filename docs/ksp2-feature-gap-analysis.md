# KSP2 feature parity and follow-up opportunities

Research date: 2026-10-26; status refreshed 2026-10-31. OSE baseline: working tree at `a28ae38`, including the local changes present during review. Reference: installed KSP2 v0.2.2.0 / build 14669105, its local decompiled implementation, and official release notes.

**Status 2026-10-31 (main `b16b6ab`): the rules, progression and screen gaps are mostly closed; the remaining gaps are physical contact (hulls, wheels, water, docking), the procedural editor tools, and measured parity evidence. Co-op multiplayer is the area where OSE goes beyond shipped KSP2. In physics OSE is approaching parity by porting the original's modules, not surpassing it.**

This status comes from the code and repository notes, not a side-by-side run of both games. The numbered findings further down are the historical audit of `a28ae38` (2026-10-26) and keep their original wording; use this table for work selection. Physics rows are tracked in more detail, with stable IDs, in [PHYSICS-GAPS.md](PHYSICS-GAPS.md).

## Current gap status

| Finding | Status | What is done / what remains | Evidence |
|---|---|---|---|
| G01, maneuver execution | **Closed** | Chained nodes are flown by the engines on delivered fuel (protocol 55); passing a node never changes velocity by itself. A burn timer drains the remaining delta-v and says why a burn stopped short. Remaining: finite-burn *prediction* of the trajectory and physics warp above 4x (P31, P32). | [Burn scheduling](../crates/ose-sim/src/lib.rs), [Mun bot](../crates/ose-bot/tests/mun_run.rs) |
| G02, science transmission | **Closed** | Samples stay aboard; data goes through antenna packets with electricity use and progress. | [Science](../crates/ose-sim/src/science.rs) |
| G03, experiment state and crew | **Closed** | Experiments wait (clock stopped, reason shown) away from their region or short of crew in their own part, and resume by themselves (protocol 56). | [Science](../crates/ose-sim/src/science.rs) |
| G04, procedural wings and fairings | **Open** | Paint and loose assemblies are saved. There is no wing shaping tool and no fairing shaping tool: fairing shells are generated automatically over the payload. | [Craft schema](../crates/ose-data/src/craft.rs), [editor](../crates/ose-client/src/vab.rs) |
| G05, action groups and part controls | **Mostly closed** | Groups are bound to part actions in the VAB and in flight, saved in the craft (protocol 57); per-engine shutdown/restart, thrust/gimbal/RCS limiters in the part window. Remaining: engine mode switching and spool-up/tail-off (P05). | [Groups](../crates/ose-sim/src/lib.rs), [part window](../crates/ose-client/src/hud.rs) |
| G06, fuel lines and struts | **Partial** | Imported craft keep their fuel lines and the flow graph honours them; resource and crew transfers exist. The VAB has no two-anchor tool to place a fuel line or strut, and struts carry no structural link. | [Importer](../crates/ose-importer/src/craft.rs), [resource flow](../crates/ose-sim/src/resources.rs) |
| G07, assemblies and editor tools | **Mostly closed** | Loose assemblies, launch selection, KSP2's move/rotate gizmo, node markers, symmetry sets, staging stack, trash bin, part info, research-locked picker. | [Editor](../crates/ose-client/src/vab.rs), [UI audit](ui-parity-ksp2.md) |
| G08, contacts | **Partial** | Part-to-part vessel/debris collisions with equal and opposite impulses and crash breakage now exist (`collide.rs`). Remaining: continuous hull contact (P18), exact collider shapes (P22), imported materials (P20), contact-based resting (P21). | [Collisions](../crates/ose-sim/src/collide.rs), [surface](../crates/ose-sim/src/surface.rs) |
| G09, water and wheels | **Partial** | Displacement buoyancy, splashed situation and recovery, chute cut on settling. Remaining: authored suspension and tyre grip (P25, P26), hull-accurate displacement (P27), flooding, waves and water-impact damage (P28). | [Surface](../crates/ose-sim/src/surface.rs) |
| G10, docking and EVA | **Partial** | Ladders, boarding, crew transfers, E/Ctrl+E for EVA. Remaining: docking acquisition force and capture (P29), swimming, hatch clearance (P30). | [Docking](../crates/ose-sim/src/docking.rs), [EVA](../crates/ose-sim/src/crew.rs) |
| G11, solar orientation | **Closed** | A flown vessel's fixed panels give what their face catches of the sun (`PanelIncidenceDirection`); on rails they count in full, as KSP2 does. Articulated tracking presentation remains (P09). | [Electrical output](../crates/ose-sim/src/structure.rs) |
| G12, training | **Mostly closed** | Lessons advance on watched conditions (launch, stage, altitude, map, part picked); conditions the client cannot watch still let Continue through. | [Lessons](../crates/ose-client/src/tutorials.rs) |
| G13, animation | **Partial** | Gimbals, wheels, deployables, Kerbal idle/walk/run/ladder/hatch clips. Remaining: full character state machine, expressions, suspension and sun-tracking presentation. | [Animation notes](reference-notes/rigs-and-animation.md) |
| G14, audiovisual | **Ongoing** | Shadows, reflection probes, KSC materials and night lighting, haze per body, volume clouds. Remaining limits as listed in G14 below. | [FX notes](reference-notes/particles.md) |
| G15, localization | **Open** | English only; KSP2 ships 12 languages. | [Client](../crates/ose-client/src/lib.rs) |

Remaining physics gaps by size of player impact: docking acquisition (P29), wheel suspension/grip (P25-P26), continuous hull contact and resting (P18, P21), water flooding/impact (P28), engine modes/spool (P05), jet intakes (P06), extra heat sources and airless cooling (P15), RISKY/IMMEDIATE chute modes (P17), physics warp above 4x (P32). Parity evidence is still mostly analytical; only a few areas have recorded KSP2 traces (P34).

Where OSE goes beyond shipped KSP2: authoritative co-op multiplayer with shared campaign, warp arbitration and input prediction; browser play with server-side saves; automated journeys (Mun bot, browser e2e, every offered mission checked completable). None of the physics areas yet goes beyond the original.

## Baseline: what counts as parity

KSP2 launched with the Kerbol system, a vehicle editor, procedural wings, craft painting, flight, map tools, and tutorials. Its official FAQ distinguishes those launch features from future colonies, interstellar travel, and multiplayer. [Official launch FAQ](https://forum.kerbalspaceprogram.com/topic/214234-ksp2-faqs/).

The December 2023 For Science! release added Exploration progression, a four-tier technology tree, Mission Control, science collection and inventory, points of interest, thermal systems, and buoyancy. Its announced mission count was 11 primary plus 30 secondary missions. [Official v0.2.0 release notes](https://forum.kerbalspaceprogram.com/ksp2-release-notes/v0200-for-science-r12/).

Use the installed **v0.2.2** behavior as the target, including the January science corrections and June parachute, recovery, and save improvements. The store still describes colonies, interstellar travel, and multiplayer as planned features and flags the age of developer updates. Do not treat roadmap promises, unused classes, asset names, or store community tags as shipped functionality. [v0.2.1 notes](https://forum.kerbalspaceprogram.com/ksp2-release-notes/v0210-r13/), [v0.2.2 notes](https://forum.kerbalspaceprogram.com/topic/225089-ksp2-release-notes-update-v0220/), [store description](https://store.steampowered.com/app/954850/Kerbal_Space_Program_2/).

The comparison here is **OSE versus that KSP2 reference**. KSP2 Redux is a separate community project and is not the parity target. KSP1 career funds, astronaut professions/experience, Breaking Ground robotics, and EVA construction should likewise not become accidental KSP2 requirements.

## What already exists

These are implemented capabilities, not claims that every edge case matches KSP2:

| Area | Current OSE coverage | Evidence |
|---|---|---|
| Solar system and navigation | 17-body Kerbol system, Kepler orbits, patched conics, SOI transitions, map trajectories, targeting, maneuver UI, warp | [Orbit/trajectory code](../crates/ose-sim/src/trajectory.rs), [universe](../crates/ose-sim/src/lib.rs) |
| Vehicle construction | 400 catalog definitions, stock/saved craft, stack/surface placement, symmetry, staging, undo/redo, mass markers, engineer report, delta-v and trip planner | [Editor](../crates/ose-client/src/vab.rs), [craft schema](../crates/ose-data/src/craft.rs), [trip planner](../crates/ose-client/src/trip.rs) |
| Flight | Powered ascent/descent, staging, SAS/free rotation, RCS, drag/lift/torque, structural stress/breakup, heat/ablation, parachutes, wheels, gear, jets | [Flight](../crates/ose-sim/src/flight.rs), [structure](../crates/ose-sim/src/structure.rs), [thermal](../crates/ose-sim/src/thermal.rs) |
| Exploration | Science locations and reports, 87 technology nodes, mission acceptance/turn-in, research locks, recovery | [Science](../crates/ose-sim/src/science.rs), [server progression](../crates/ose-server/src/science.rs) |
| Crew and operations | Roster/seating, EVA movement and jetpack, boarding, flags, docking/undocking, resource flow and electrical systems, CommNet | [Crew](../crates/ose-sim/src/crew.rs), [docking](../crates/ose-sim/src/docking.rs), [resources](../crates/ose-sim/src/resources.rs) |
| Persistence and UI | Campaign/session flows, saves, quicksave/load and revert, KSC/VAB/Tracking Station/R&D/Mission Control, rebinding, audio levels, UI/render scale | [Campaigns](../crates/ose-server/src/campaigns.rs), [client integration](../crates/ose-client/src/lib.rs), [input](../crates/ose-client/src/input.rs) |
| Presentation | Terrain packs, atmosphere, ocean/FFT waves, cloud rendering including a volumetric pipeline, grass/scatter/impostors, original asset meshes, extensive flight FX, rig/clip/skinning support | [Planets](../crates/ose-client/src/planet.rs), [FX comparison](reference-notes/particles.md), [animation comparison](reference-notes/rigs-and-animation.md) |
| Co-op | Authoritative server, shared campaign science, identity/resume, shared warp arbitration, snapshot streaming and input prediction | [Multiplayer notes](multiplayer-performance.md), [server](../crates/ose-server/src/main.rs) |

Counts above were read directly from `assets/own/*.json`. A catalog definition is not proof that all its modules work. The [existing Mun bot](../crates/ose-bot/tests/mun_run.rs) is useful coverage of one journey; it does not establish parity for every planet, craft configuration, or science mission.

## Confirmed gaps and completion checks

Priority meanings: **P0** changes the core rules or progression; **P1** blocks substantial reference gameplay; **P2** completes fidelity, onboarding, or breadth. Relative sizes describe engineering scope, not calendar estimates.

### P0: preserve the meaning of missions and flight

**G01 — Separate maneuver plans from physical burn execution. Confirmed; large.**

OSE stores one `ScheduledBurn` per vessel. `apply_due_burn` directly replaces the on-rails orbit with an instantaneous delta-v and removes the plan; it does not consume propellant or check engine capability. `Universe::step` calls it for vessels without `Powered`, including dormant craft. Planning is therefore also a free orbital maneuver on that path. Powered manual burns do use fuel, so this is specifically a discrepancy in scheduled on-rails burns. The protocol exposes scheduling in ordinary play.

KSP2 maintains a list of maneuver nodes with prediction and burn guidance. Add a persistent multi-node plan, finite-duration prediction, remaining delta-v/burn timing, and execution through the real engine/resource model. Keep instantaneous trajectory changes only as an explicitly selected development/cheat operation. Address long low-thrust burns and acceleration during warp as part of this work; OSE currently caps active flight at 4x.

Completion: creating or passing a node never changes velocity by itself; a dry craft cannot execute it; manual burns consume the correct mixture; sequential departure/correction/capture nodes survive saves; long burns predict and execute within an agreed tolerance.

Evidence: [scheduled burn and application](../crates/ose-sim/src/vessels.rs), [scheduling/step/warp](../crates/ose-sim/src/lib.rs), [server command](../crates/ose-server/src/main.rs); reference `KSP.Sim.impl/ManeuverPlanComponent.cs`, `ManeuverPlanSolver.cs`, `OdeManeuverNode.cs`. Acceleration during time warp is explicitly discussed in the [v0.2.0 fixes](https://forum.kerbalspaceprogram.com/ksp2-release-notes/v0200-for-science-r12/).

**G02 — Restore the distinction between transmitted data and returned samples. Confirmed; medium.**

`TransmitScience` and recovery share a handler which calls `take_science` on every carried report, then submits them all. Neither that branch nor `science::submit` filters samples. Transmission is immediate after the CommNet check, without a transmission queue, packet timing, or transmitter electricity consumption. This removes much of the reason to return samples and can unlock technology too easily.

Completion: transmit only eligible data; keep samples aboard until recovery or an allowed physical transfer; advance transmissions using antenna rates and electricity; handle link/power loss, cancellation, save/reload, docking, and duplicate reports without losing or double-crediting science.

Evidence: [request handler](../crates/ose-server/src/main.rs), [submission](../crates/ose-server/src/science.rs), [report storage](../crates/ose-sim/src/science.rs); reference `KSP.UI/ResearchInventoryReportEntry.cs` (`GetCanTransmit` excludes samples), `KSP.Sim.impl/PartComponentModule_DataTransmitter.cs` (packet size, interval, resource requests, progress).

**G03 — Match experiment state and crew requirements. Confirmed; medium.**

OSE removes running experiments when their region/situation changes. Its crew check is a vessel-wide boolean, not the required number of occupants in the experiment's own part. Both differ from documented later KSP2 behavior. Implement suspended experiments that restart automatically on returning to the original region, plus per-part crew counts and persistent state. [Official v0.2.1 science corrections](https://forum.kerbalspaceprogram.com/ksp2-release-notes/v0210-r13/).

Completion: test leaving/returning to a region, insufficient crew in the correct compartment despite crew elsewhere, loss/restoration of power, and save/reload in each experiment state.

Evidence: [run/advance experiments](../crates/ose-sim/src/science.rs); reference `KSP.Sim.impl/PartComponentModule_ScienceExperiment.cs`, including its part-scoped roster query.

### P1: construction and ship operations

**G04 — Procedural wings, fairings, and persistent customization. Confirmed; large.**

`CraftPart` stores placement, stage, crossfeed, resources, and an optional fuel-line target. It has no procedural wing dimensions, fairing shell profile, per-part paint, or general saved module state. The importer drops those instance settings. Fixed imported models cannot reproduce arbitrary authored craft, even when the correct part ID exists. Fairing protection currently uses the top-node subtree, and shell drag uses a generic cone approximation.

Completion: import/build/save/reload a custom swept wing and shaped fairing; geometry, mass, lift/drag, shielding, symmetry, paint, and server/client state agree. Persist the settings through launch, docking, staging, and recovery to the editor. Wings and painting are explicitly shipped features in the [official launch FAQ](https://forum.kerbalspaceprogram.com/topic/214234-ksp2-faqs/).

Evidence: [schema](../crates/ose-data/src/craft.rs), [importer](../crates/ose-importer/src/craft.rs), [editor](../crates/ose-client/src/vab.rs), [shielding](../crates/ose-sim/src/structure.rs); reference `KSP.Modules/Module_ProceduralPart.cs`, `Module_Fairing.cs`, `KSP.OAB/ObjectAssemblyColorTool.cs`.

**G05 — Functional custom action groups and individual part controls. Confirmed; medium–large.**

Custom groups currently toggle ten booleans with no bound part actions. Abort has a fixed throttle-cut/chute behavior. Several commands operate on all panels, bays, gear, or converters together. There is no complete per-instance command/configuration surface for engine mode, thrust limiter, gimbal controls, and action assignments.

Completion: bind one group to selected engines and panels, leave other parts unaffected, save the assignment, and use it after staging/docking. Add per-part activation and mode/limiter controls with authoritative validation. Evaluate full authored pressure curves: `Powered::thrust_kn` currently interpolates between vacuum and sea-level thrust with pressure clamped to 0–1 atm, which cannot represent the full curve in denser atmospheres.

Evidence: [action-group implementation](../crates/ose-sim/src/lib.rs), [protocol commands](../crates/ose-net/src/lib.rs), [thrust model](../crates/ose-sim/src/flight.rs); reference `KSP.Modules/Data_Engine.cs`, `Data_Gimbal.cs`, and their part modules.

**G06 — Fuel-line/strut authoring and in-flight resource transfer. Confirmed; medium–large.**

Fuel-line flow already works when the `fuel_line_target` field is populated; the importer sets it to `None`, and the editor has no complete two-anchor placement flow. Strut links also need a persistent structural representation. Resource totals and normal engine consumption exist, but no player-directed tank-transfer command appears in the protocol. KSP2's resource manager has explicit start/stop transfer operations.

Completion: build/import an asparagus fuel layout and a strut-braced assembly; verify the intended drain order and structural loads; dock a tanker, transfer selected propellant without creating mass, then undock. Transfers must obey the resource's transfer rules and persist across reconnects.

Evidence: [craft importer](../crates/ose-importer/src/craft.rs), [flow graph](../crates/ose-sim/src/resources.rs), [craft schema](../crates/ose-data/src/craft.rs), [protocol](../crates/ose-net/src/lib.rs); reference `KSP.OAB/ObjectAssemblyCompoundPartTool.cs`, `KSP.Modules/Module_Strut.cs`, `Module_FuelLine.cs`, `KSP.UI/ResourceManagerUI.cs`.

**G07 — Multiple assemblies and complete assembly tools. Confirmed schema gap; medium–large.**

OSE saves one rooted `Craft` tree. KSP2's workspace can hold assemblies and choose an assembly to launch; it also has anchor, rotation, and compound-part tools. OSE already has drag placement, subtree movement, symmetry, and keyboard rotation. The remaining work is a workspace model for disconnected assemblies/subassemblies and the corresponding selection/launch/edit operations, followed by a tool-by-tool comparison.

Completion: work on a launcher and lander separately in one workspace, join them, change the anchor, save/reload, and launch the intended assembly without losing unattached work.

Evidence: [craft tree/schema](../crates/ose-data/src/craft.rs), [editor](../crates/ose-client/src/vab.rs); reference `KSP.OAB/ObjectAssemblyWorkspace.cs`, `ObjectAssemblySelectAnchorTool.cs`, `ObjectAssemblyRotationTool.cs`.

### P1: physical interactions

**G08 — Couple vessel motion to physical contacts and part forces. Confirmed architectural gap; large.**

OSE has real structural load calculation, flexible joints, breakup, aero torque, and part-impact handling. However, `structure.rs` explicitly keeps trajectory/attitude in the separate flight model rather than using the structure's motion. Rapier's collider set starts empty, and flight contact is handled against a sampled surface/sea radius. This is not a complete shared physical world for vessel–vessel, debris, facility, and terrain contacts. Engine forces are also reduced along the vessel axis in the structure model.

Completion: compare off-axis thrust, asymmetric staging, vessel/debris impacts, landing on uneven ground, and contact with a facility against reference fixtures. Contacts must conserve momentum plausibly and remain stable through vessel switching, SOI transitions, save/reload, and two nearby players. Include deployed geometry and reinforced-joint settings in the comparison. Follow the project's [explicit physics-parity decision](ADR-0001-initial-decisions.md).

Evidence: [structure design and impact handling](../crates/ose-sim/src/structure.rs), [rigid representation](../crates/ose-sim/src/rigid.rs), [flight contact](../crates/ose-sim/src/flight.rs); reference `KSP.Sim.impl/PartBehavior.cs`, `PartOwnerBehavior.cs` and the [joint/drag notes](reference-notes/physics-joints-drag.md).

**G09 — Buoyancy and richer wheel/suspension behavior. Confirmed; large.**

Water currently acts as a surface for landing rather than applying displacement-based buoyancy and water forces. Rover/plane motion and suspension are implemented in simplified form, rather than the reference's wheel contact, suspension, traction, and damage subsystems. Do not list rovers or landing damage as entirely absent: both have working code.

Completion: a capsule bobs and settles according to mass/displacement; a boat can float and move; an asymmetric hull can roll; wheels maintain contact on slopes with braking, slip, steering, and damage behavior measured against reference craft. KSP2's buoyancy was a shipped For Science! feature. [Official release notes](https://forum.kerbalspaceprogram.com/ksp2-release-notes/v0200-for-science-r12/).

Evidence: [ground/sea contact and driving](../crates/ose-sim/src/flight.rs), [suspension/impact handling](../crates/ose-sim/src/structure.rs); reference `KSP.Sim.impl/PartBehavior.cs` (`BuoyancyUpdateJob`), `KSP.Modules/Module_WheelSuspension.cs`, `Module_WheelDamage.cs`, `Module_WheelMotor.cs`.

**G10 — Docking acquisition, EVA contact, and crew operations. Confirmed partial implementation; large.**

Docking currently latches immediately at acquire range once orientation and speed pass the test, instead of attracting/alignment followed by capture. EVA has walking, running, jumping, jetpack flight, and flags, but boarding uses proximity within 6 m rather than a full hatch/ladder interaction. Complete ladders/grabbing/climbing, hatch access, swimming, and in-vessel crew transfers alongside the contact system.

Completion: approach and magnetically acquire a port without teleport-like snapping; collide safely when misaligned; undock without immediate recapture; climb to a hatch and board; transfer crew into the experiment compartment; save/reload during these operations.

Evidence: [docking simplification](../crates/ose-sim/src/docking.rs), [EVA/boarding](../crates/ose-sim/src/crew.rs), [protocol](../crates/ose-net/src/lib.rs); reference `KSP.Modules/Module_DockingNode.cs` and `KSP.Sim.impl/LadderStartClimbKerbalCharacterMessage.cs` / associated character states.

**G11 — Solar orientation and deployment-dependent physical state. Confirmed; medium.**

Fixed panels generate at half their rating regardless of orientation; tracking panels use full rating scaled by sunlight. Folding and celestial shadowing already work. Intermediate rig poses do not yet alter collision/drag, and procedural sun tracking is not wired through the full presentation/simulation path.

Completion: rotate a fixed panel edge-on and observe its output fall; a tracking panel follows the sun within its limits; shadowing, partial deployment, damage, and time warp produce consistent energy. Validate deployed drag and contact geometry against the actual part state.

Evidence: [electrical generation](../crates/ose-sim/src/structure.rs), [animation limits](reference-notes/rigs-and-animation.md); reference `KSP.Modules/Module_Deployable.cs`, solar-panel modules.

### P2: complete the experience

**G12 — Interactive training. Confirmed; medium–large.** `tutorials.rs` explicitly advances messages with Continue without evaluating lesson conditions. Recreate scenario setup, action/flight checks, objective prompts, reset/retry, and completion persistence. Completion means a novice can build, launch, orbit, maneuver, and reenter through lessons that only advance when their objectives are met. [Tutorial code](../crates/ose-client/src/tutorials.rs), [first-time guidance](../crates/ose-client/src/ftue.rs).

**G13 — Kerbal and actuator animation. Confirmed; medium–large.** Rig import, skinning, animation blending APIs, and panel/gear deployment exist. Character retargeting/state machines, locomotion/expressions/blend shapes, gimbal/sun-tracking controllers, and wheel spin/steering/suspension animation remain incomplete. Completion means the rendered state follows gameplay per instance, including late joins, reversing an animation, and warp. [Detailed reference comparison and reproduction](reference-notes/rigs-and-animation.md).

**G14 — Full audiovisual fidelity. Confirmed remaining limits; ongoing.** Engine and reentry shader comparisons already provide strong narrow evidence. Remaining documented FX gaps include particle lighting/probes/shadows/lights, random/noise/ribbon behavior, explosion distortion, full post-processing, and procedural wing vortex anchors. Audio uses imported music and effects but needs an event-by-event/spatial comparison before claiming parity. Preserve existing terrain, clouds, ocean, grass, and scatter work; these are not missing systems. [FX limits](reference-notes/particles.md), [audio implementation](../crates/ose-client/src/audio.rs), [planet renderer](../crates/ose-client/src/planet.rs).

**G15 — Localization breadth. Confirmed; medium.** The client loads the exported English localization file and includes hardcoded English UI. KSP2 lists 12 supported interface/subtitle languages. Complete locale selection, text coverage, fonts/layout, and subtitles if full language parity is in scope. Rebinding, music/SFX levels, and UI scaling already exist. [Client localization loader](../crates/ose-client/src/lib.rs), [official language list](https://store.steampowered.com/app/954850/Kerbal_Space_Program_2/).

## Areas requiring a focused parity audit

These should become bounded investigations rather than unverified “missing feature” tickets:

| Area | Established finding | Next evidence needed |
|---|---|---|
| Mission coverage and ordering | 57 imported definitions; 40 pass `playable()`. Exactly 17 have no granter; only two contain unsupported conditions/events, and both are in that excluded set. The server explicitly chains the first three primaries, then opens the remaining primaries together. | Map the 40 offered entries to the shipped 11+30 mission list; inspect source activation actions, narrative prerequisites, and point-of-interest missions. **Do not report 17 missing playable missions or a 70% completion rate.** |
| Discoverable objects | Science region/discoverable data are imported. | Inventory visible landmark models, colliders, discovery notifications, authored mission scripts, and persistence at each location. Science coordinates alone do not prove the landmark experience is complete. |
| Thermal and parachute parity | Heating, ablation, shielding, chute deployment and breakage exist. The June patch specifically changed safety/destruction and default deployment behavior. | Run matched craft through Kerbin/Eve/Duna conditions using the installed constants, including fairings, high-speed deployment and low pressure. Avoid claiming missing heating or parachutes. |
| Assembly and part controls | G04–G07 identify schema and command gaps. | Inventory every shipped module and saved field, classify implemented/approximate/unused, and check structural reinforcement, variant/configuration controls, symmetry persistence and editor tools. |
| Save fidelity | Craft structures, crew, science, docking, and many deployables are saved. Targets are documented as not saved. | Round-trip every runtime part setting and mission state; verify schema migration and restoration mid-experiment/burn/docking. Do not repeat the obsolete claim that all restored vessels lose their structure. |
| Performance and visual quality | Local ascent and multiplayer measurements exist, with specific hardware/scenario limits. | Set budgets for large craft, multiple players, long saves, dense scenes, weaker WebGPU hardware, asset load/reconnect, and long-duration warp. A shader RMSE or one ascent test is not a whole-game parity percentage. |

Evidence: [mission predicate/data model](../crates/ose-data/src/missions.rs), [mission availability](../crates/ose-server/src/science.rs), [imported missions](../assets/own/missions.json), [science regions](../crates/ose-data/src/science.rs), [saves](../crates/ose-server/src/save.rs), [ascent measurements](ascent-performance.md), [multiplayer measurements](multiplayer-performance.md), [v0.2.2 notes](https://forum.kerbalspaceprogram.com/topic/225089-ksp2-release-notes-update-v0220/).

## Recommended sequence and parity exit gates

1. **Correct the rules:** G01–G03. Separate planning from execution, restore sample-return incentives, and fix experiment semantics. These affect every later balancing or campaign test.
2. **Make craft definitions complete:** G04–G07. Extend and version the craft schema before adding UI that cannot save its settings. Coordinate geometry, aero, mass, resources, and protocol changes.
3. **Finish physical interactions:** G08–G11. Shared contacts underpin credible docking, wheels, buoyancy, ladders, and deployed parts. Build small reference fixtures before changing the whole physics stack.
4. **Complete the campaign journey:** mission/landmark audit plus G12. Validate a fresh Exploration save all the way through the shipped progression, using ordinary player commands.
5. **Finish presentation and breadth:** G13–G15, measured audiovisual comparisons, and performance budgets. Localization can be prioritized by the actual audience without blocking a private English-only playthrough.

Require these player journeys before declaring gameplay parity:

- Build a customized rocket from researched parts; launch, orbit, plan and execute a fuel-limited burn; reenter and recover.
- Return a sample from the Mun while separately transmitting data; obtain the correct science once; complete the intended mission and unlock sequence.
- Build/fly a procedural-wing aircraft with mode switching and part controls; land and brake on the runway.
- Rendezvous with a station, dock, transfer crew and fuel, undock, and repeat after a save/reload.
- Drive a rover, climb a ladder, board a hatch, and recover a floating capsule.
- Repeat representative journeys with another player present, then disconnect/rejoin and restart the server.

Use agreed numerical tolerances and observable outcomes, not identical reproduction of known KSP2 bugs. Separate game-rule parity, content coverage, visuals, and performance in the checklist.

## Nice-to-have features after the gaps close

These are proposed OSE improvements, not claims about missing shipped KSP2 features.

| Order | Opportunity | Player benefit | Prerequisites / relative size |
|---|---|---|---|
| 1 | Transfer-window planner, mission alarms, and a richer science archive | Less repetitive timing and easier planning of unexplored regions; extends the existing trip budget and inventory | Correct maneuver/science model; medium |
| 2 | Shared mission-control roles, vessel permissions, annotations and spectating | One player pilots while another plans, tracks science or operates station systems | Existing multiplayer plus per-part authority; medium–large |
| 3 | Craft sharing with version history, tags and design comparison | Easier cooperative design, safer iteration, reusable subassemblies | Complete craft schema and workspace; medium |
| 4 | Flight recorder, telemetry plots, deterministic replay and crash explanation | Turns failures into useful engineering feedback; improves debugging | Stable simulation events and save versions; medium–large |
| 5 | Optional flight assistance: docking cues, landing predictions, maneuver execution | Makes complex missions more approachable while allowing manual flight | Real fuel-limited execution and reliable contacts; medium–large |
| 6 | Richer communications: occlusion, relay planning, optional signal delay | Adds a satellite-network design challenge | Existing CommNet; medium. Body occlusion is an enhancement: the inspected KSP2 connection model also lacks it. |
| 7 | Co-op warp voting, scheduled rendezvous, and agreed rollback checkpoints | Makes shared time and recovery less disruptive | Existing lowest-request warp and reload safeguards; medium. Current reload already refuses while another player is playing. |
| 8 | Documented data/mod API, mission authoring and scenario packs | Community content without editing engine code | Versioned part/mission schemas, validation and packaging; large |
| 9 | Colonies, resource extraction/production, orbital construction and supply routes | A sustained reason to build infrastructure beyond individual missions | Cargo/resource operations, background simulation, UI and economy design; very large expansion |
| 10 | Interstellar systems and advanced propulsion | New destinations and long-term engineering goals | Long-burn/warp accuracy, scalable streaming, original content and colony logistics; very large expansion |
| Optional | Robotics, life support, deeper reliability/economy rules | New engineering styles for players who want more constraints | Separate gameplay design; large. Do not gate KSP2 parity on these. |

The highest-value early additions are planning/alarms, cooperative mission control, and craft sharing. They build on OSE's strengths. Colonies and interstellar should each have a separate design and acceptance plan; neither is a small polish feature.

Replacing reference assets with original assets remains the project's own long-term deliverable ([architecture phase 9](02-architecture-and-plan.md)). It is separate from matching the installed game's behavior.

## Documentation corrections exposed by this audit

- `STATUS.md` mixes chronological implementation notes with an obsolete “Next” list and “Known problems” section. Those still list already-implemented campaign content, 3D UI, structure persistence, and ground interaction work.
- `ui-parity-ksp2.md` marks custom action groups and the resource manager done based on UI/state presence. Functional binding and resource transfer remain open; “UI present” needs a separate state from “behavior matches.”
- The mission note's “40 of 57” should be accompanied by the granter/unsupported-condition breakdown above.
- Existing graphics comparisons explicitly limit their claims. Retain those limits rather than converting narrow numerical matches into a blanket parity claim.

This report adds a current backlog without rewriting historical status or modifying the implementation. Reference class names refer to the local gitignored `reference/ksp2/Assembly-CSharp/` tree; no decompiled implementation is reproduced here.
