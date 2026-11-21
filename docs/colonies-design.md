# Colonies: design and plan

Status: **step 1, the colony panel and routes to colonies built** (2026-11-21, protocols 75 and 78), with Josh's
answers to the first questions (section 5).
Later steps follow the agreed order in section 4.

## 1. Why colonies, and what KSP2 had

KSP2's published roadmap after early access was reentry heating, science ("For Science!", shipped in
0.2.0), **colonies**, interstellar, then multiplayer. OSE already has reentry heating, science and co-op, and
delivery routes are waiting on colonies for landed depots ([delivery-routes-design.md](delivery-routes-design.md),
decision 12). Colonies are therefore the next promised feature.

KSP2 never shipped colonies. What the 0.2.2 install has, studied from `reference/ksp2/Assembly-CSharp/`:

- **Colony trunk parts**: `transport_3v_colony_trunk_small` (2 t) and `transport_3v_colony_trunk_large`
  (10 t), placeholder models in the Utility category, unlocked by no tech node.
- **`KSP.Game.Colonies`**: `ColonyManager` listens for `ColonyDeployedMessage` and
  `ColonyEstablishedMessage` (a vessel deploys into a colony and stops being a flight), tracks flights from a
  colony as their origin, and replays delivery routes on a timer.
- **`IConstructionResourceProvider`** (`CanAffordCost`, `DeductCost`, `GetCurrentResources`): what a
  colony builds is paid for in resources from the colony's store. Only a debug provider (99 999 of
  everything) exists.
- **`PredefinedColonyObject`**: a colony object can have a population component (fill limits, vacancy
  delay, new or reused veterans), telemetry, and be a CommNet source.
- **`AbundanceRequest` / `PlanetaryResource`**: resource abundance by body, latitude, longitude, altitude
  and biome, with harvest types: in-situ resources were planned.
- **UI**: a `ColonyView` game state, a colony variant of the editor (`OABVariant.BAE`, environments
  `GroundColony*`, `Orbital`), a colony breadcrumb, and a colony action-bar variant. None is reachable.
- Resources already in the data that only colonies and interstellar would use: Uranium, Xenon, Hydrogen.

## 2. Principle

As with delivery routes, **nothing is abstract**. A colony is a **site** on a body. The vessels landed
round it belong to it:

- what their tanks hold is the colony's **stock**;
- the Kerbals aboard them are its **residents**;
- a tanker that lands next to it adds its fuel; a lander that takes off takes its fuel away.

So a colony needs no simulation of its own, and everything players already do (landing, docking,
transferring fuel, EVA, boarding) works on it unchanged.

## 3. Step 1 (built)

- **Research**: a colony path in the tech tree (OSE's own, `ose_data::colony::NODES`), after the 3.75 m
  payload node: **Colony Foundations** (2900 science, the small trunk) then **Colony Logistics** (4500, the
  large trunk). The importer adds them, and a test keeps `assets/own/tech-tree.json` in step.
- **Metal**: a new resource, the building material, one unit a kilogram. The small trunk holds 10 t of it
  and the large one 40 t, filled on Kerbin like a fuel tank (the VAB can fill less). Like fuel, it is part of
  the stock once its trunk is landed at the colony, and it can be pumped between docked vessels.
- **Found** (`FoundColony { vessel, name }`): a player's vessel founds a colony where it stands when it is
  landed (not afloat, not wrecked), carries a colony trunk, and has a Kerbal aboard. Colonies stand at least
  2 km apart on a body and at least 5 km from the KSC. The colony is saved with the campaign
  (`save::Campaign::colonies`), so saves, quicksaves and reverts carry it. In flight the trunk's part window
  has **Found colony here**.
- **Members**: every vessel landed within 300 m of the site (Kerbals on EVA and flags left out).
- **Launch pad**: each colony adds a launch site `Colony_<id>`, 400 m east of its site, outside the colony,
  so a craft waiting on the pad is not part of the stock. It appears in the VAB's launch-site list as
  "<name> pad (<body>)", with the usual "in use" check.
- **Paying for a launch**: a craft rolled out on a colony pad takes every resource its tanks hold from the
  colony's stock, except electric charge (batteries charge themselves) and intake air, **and its parts' dry
  mass in Metal** (1 kg per unit). Each tank gives in proportion to what it holds. If anything is short the
  launch is refused and says what is missing; nothing is taken.
- **Crew**: the craft is crewed by the colony's residents (chosen names must live there; otherwise residents
  fill the seats in order). They leave their colony vessel.
- **Abandon** (`AbandonColony { id }`, founder only): the record goes; the vessels stay.
- `ColoniesInfo` answers `Colonies { colonies, reason }`, which lists each colony's vessels, residents and
  stock. Founding or abandoning a colony tells everyone in the campaign.

Code: `crates/ose-colonies` (rules, tested on the Mun in `tests/found_and_launch.rs`),
`crates/ose-server/src/colonies.rs` (messages, launch sites), `launch_on_pad_now` (colony pads).

## 4. Proposed next steps

1. ~~**Colony panel**~~ (built): the tracking station's **Colonies** tab lists the campaign's colonies; the
   card of the one picked shows its body and site, residents, vessels, launches and stock (Metal first, in
   units and tonnes), and its founder can abandon it there. Still to come: a colony icon on the map and in
   the flight view.
2. ~~**Delivery routes to colonies**~~ (built, protocol 78): see section 6.
3. **Colony CommNet**: a colony with an antenna is a ground station (KSP2's `IsCommNetSource`).
4. ~~Building costs parts too~~: done in step 1 (Metal).
5. **In-situ resources**: an abundance map per body (KSP2's `AbundanceRequest`), drills and converters
   (Methane, Uranium). KSP2 ships no drill parts, so this needs our own parts.
6. **Population**: KSP2's vacancy fill: new Kerbals join a colony with habitation over time.
7. **Orbital colonies**: KSP2 planned an orbital editor environment. Depots already cover stations for
   routes; an orbital colony would add building in orbit.

## 5. Decisions (Josh, 2026-11-21)

1. **A colony trunk is required** to found a colony (with a Kerbal aboard, landed).
2. **Parts cost Metal shipped from Kerbin, like fuel.**
3. (Follows from 2: the construction resource is Metal.)
4. **The order of section 4 is agreed**: colony panel, routes to colonies, CommNet, then resources and
   population.
5. **Colony research has its own path in the tech tree.**

## 6. Routes to colonies (built)

A route's endpoint is a depot or a colony (`ose_routes::route::Endpoint`, saved with the route; routes saved
before it load as depot routes).

- **Proving flight**: the player records a flight from the pad as for a depot route, lands within a colony
  and stops the recording there (`StopRecording`). The server sees the vessel is landed and one of the
  colony's members, and ends the recording **Landed** (`Routes::finish_at_colony`): what the vessel still
  holds (all but electric charge and intake air) is the cargo. Only airless bodies yet, where the route
  pilot can land; a colony under an atmosphere refuses with a reason.
- **Route**: `CreateRoute` from a Landed recording makes a colony route (`RouteInfo::colony`). Its runs
  always stay (`SetRouteEnd` refuses anything else): the run is the delivery.
- **Runs**: no launch window is needed (the trip phases its own transfer), so a run launches 30 s after the
  last one ends. The route pilot (`ose_pilot::trip`) climbs to orbit, moves sphere of influence by sphere of
  influence to the colony's body into a low circular orbit, then lands (`ose_pilot::landing`): it rounds the
  orbit out if the capture left it eccentric, turns the plane over the aim point, lowers periapsis just
  over it, brakes under closed-loop guidance and settles. Each run aims 120 m from the site, a golden angle
  further round than the last, so the runs do not land on each other. A run that lands within the colony's
  300 m is **Delivered** (or **Short** if it holds less than the proving flight did) and its vessel joins
  the colony's stock; one that lands further away **Missed**.
- **Restart**: a run in flight is found by its name and its trip is planned again from where it is (on the
  ground at KSC it climbs, in orbit it carries on); one that landed in the colony meanwhile is Delivered.
- **Abandon**: abandoning a colony pauses its routes.

Tested on the Mun: `crates/ose-pilot/tests/landing.rs` (landings within 30 m of two sites from a 25 km
orbit, and from low Kerbin orbit to a Mun site, 12 m off) and `crates/ose-routes/tests/colony_route.rs`
(record, create, restart, land, abandon).
