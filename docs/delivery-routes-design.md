# Delivery routes: design draft

Status: **draft for discussion** (2026-11-14). Nothing here is built yet. Open questions are at the end and
marked **Q1-Q6**; the draft assumes the recommended answer to each until Josh decides.

## 1. What KSP2 had

Delivery routes never shipped in KSP2 (0.2.2 has no colonies and no UI entry; see
[03-ui-screens-and-flows.md](03-ui-screens-and-flows.md) line 424 and
[ksp2-feature-gap-analysis.md](ksp2-feature-gap-analysis.md) item 9). The decompiled
`KSP.Game.Colonies` namespace still holds a small, working prototype of the idea, and it is a good fit for
us. Studied from `reference/ksp2/Assembly-CSharp/KSP.Game.Colonies/` (not copied):

- **Record a real flight.** `ColonyManager` starts an `ActiveFlight` for every vessel created at an
  origin (a colony, or Kerbin when there is none). The flight snapshots the vessel's stored resources
  (`ResourceSnapshot`, a name to units map) at four moments: launch, docking at the destination,
  undocking from it, and recovery back at the origin. The times of each are kept.
- **Lifecycle.** `ActiveFlight.FlightStatus` = ProceedingToDestination, DockedAtDestination,
  ReturningToOrigin, Complete, Failed. Docking to another tracked flight cancels both; reverting to the
  VAB, destroying the vessel or deploying it as a colony drops the flight; revert to launch restarts it.
- **Convert.** Only a Complete flight can become a `DeliveryRoute` ("This flight must be complete before
  it can be turned into a delivery route."). The route stores:
  - `OriginCost` = resources at launch (what each run consumes at the origin),
  - `DeliveryToDestination` = resources before docking minus after undocking (what was unloaded),
  - `DeliveryToOrigin` = launch minus recovery (what came back), plus
  - `MissionTimeToDestination` and `TotalMissionTime`.
- **Replay without flying.** `RouteStatus` = ReadyToLaunch, ProceedingToDestination, ReturningToOrigin,
  Paused. Each update, if `UT - launchTime > timeToDestination` the destination is credited, and after
  `TotalMissionTime` the origin is credited and the route relaunches by itself, unless a pause was
  requested, in which case it stops at the origin. `ApplyResourcesToColony` is an empty stub.
- **UI.** A debug `ColonyManagerDialog` lists flights and routes with one action button that walks the
  state machine (Dock / Undock / Recover / Convert, then Launch / Pause) and Delete. The map and tracking
  station already reserve `MapItemType.DeliveryRoute` and a `DeliveryRoute` filter.

So the intended loop is: **fly the supply run once by hand, prove it works, then let it repeat on its
own** at the recorded cost and cadence. No route ever simulates a vessel again.

## 2. What OSE has to build on

- Docking, undocking and crew/resource transfer between docked vessels (`ose-sim` `docking.rs`,
  `crew.rs`), resource flow (`resources.rs`), recovery on Kerbin (`RecoverVessel`).
- On-rails vessels with saved stage and propellant state (`SavedVessel`), so an unloaded vessel's tanks
  can be edited without loading it.
- A shared co-op campaign on one authoritative server tick, with time warp agreed between players.
- **No colonies, no funds** (science mode only). This is the main gap: KSP2's routes ran between colonies.

## 3. Proposed design (MVP)

### 3.1 Endpoints: depots instead of colonies

Until colonies exist, a route runs between **KSC** and a **depot**. A depot is any existing vessel a
player marks as one (an orbital station, a landed base, a fuel tank parked in orbit). Marking is a
toggle in the vessel's info panel in map / tracking station. Depots keep working when colonies arrive:
a colony simply becomes another kind of endpoint (`Endpoint::Ksc | Vessel(id) | Colony(id)`).

KSC is an infinite source in science mode, so the origin cost of a KSC route is free (Q3). A route may
also start at a depot (depot to depot); then the origin cost is taken from that depot's tanks.

### 3.2 Creating a route: record, then convert (KSP2's model)

1. Launch normally. The server opens a **route recording** for the new vessel with origin KSC (or the
   depot it undocked from).
2. Fly to a depot and **dock** with it. Transfer resources as usual (fuel, ore, monoprop, EC, later
   cargo). Undock.
3. Fly back and **recover** at KSC (or dock back at the origin depot). A one-way run, where the vessel is
   left docked at the destination as part of the station, is allowed too (Q2).
4. The recording is now Complete. In the **Delivery Routes** panel any member of the campaign can press
   **Create route**, name it, and see what one run costs and delivers and how long it takes.

What counts as delivered is exactly what changed in the vessel's tanks while docked, so fuel ferries,
ore haulers and EC top-ups all fall out of the same rule. Crew is not delivered in the MVP.

A recording is dropped when the vessel is destroyed, reverted to the VAB, or docks with another recording
vessel; revert to launch restarts it (same rules as KSP2).

### 3.3 Running a route

Server-side, in `ose-sim`, as a bevy_ecs component on a route entity, ticked with universe time so it is
correct through time warp and while nobody is online:

- **Launch**: take `origin_cost` from the origin (nothing for KSC), stamp `launch_ut`.
- **Arrive** at `launch_ut + time_to_destination`: add `delivery` to the depot's tanks, capped at
  their capacity. Any overflow is reported, not lost silently (it stays "on board" and comes back as
  part of the return leg).
- **Return** at `launch_ut + total_time`: credit the origin with the return snapshot, then relaunch, or
  stop if a pause is pending.
- **Blocked**: if the depot no longer exists, or the origin depot cannot pay the cost, the route goes
  to Blocked with a reason instead of running. It resumes by itself when the cause clears.

A delivery into a depot that a player is currently flying is applied on the next tick like any resource
transfer, and shown with a toast ("Kerbin fuel run delivered 2,400 LF / 2,930 Ox to Station Alpha").

Catch-up: if the server was down or a long warp skipped several cycles, all completed cycles are applied
in order in one step (capped by tank space each time), so warp speed never changes the outcome.

### 3.4 UI

- **Delivery Routes panel** (KSC and tracking station, and a header button in flight/map): recordings
  in progress with their stage, and routes with status, next event countdown, per-run cost and
  delivery, totals delivered so far, Pause / Resume / Delete, and Rename.
- **Map / tracking station**: the `DeliveryRoute` filter and an icon on each depot with routes; a dashed
  line between origin and destination body while a run is in flight (no ghost vessel in the MVP, Q5).
- **Depot toggle** in the vessel info panel.

### 3.5 Multiplayer

Routes, recordings and depots belong to the shared campaign. Any member can create, pause or delete a
route; the panel shows who created it. Two routes into the same depot are fine; arrivals are applied in
UT order. Everything runs on the server; the client only shows state it receives.

### 3.6 Save format

`Campaign.routes: Vec<SavedRoute>` and `Campaign.route_recordings: Vec<SavedRecording>`, plus
`SavedVessel.depot: bool`, all `#[serde(default)]` so old saves load unchanged. Resources are stored by
name, as in KSP2's `ResourceSnapshot`.

## 4. Out of scope for the MVP

Colonies and the base editor, resource extraction and production, crew rotation, a funds economy,
failure chances on routes, and visible ghost vessels flying the recorded path. Each can be added on top of
the same route entity later.

## 5. Build plan (after the design is agreed)

1. `ose-sim`: `ResourceSnapshot`, recording state machine hooked into launch, dock, undock, recover,
   destroy and revert; route component and tick with catch-up; unit tests on a scripted fuel run
   including warp, overflow and blocked depots.
2. `ose-server` + `ose-net`: depot flag, create/pause/resume/delete/rename messages, save fields,
   toast events.
3. `ose-client`: Delivery Routes panel, depot toggle, map filter and icons.
4. Browser e2e against a local server: fly a scripted fuel run with the bot, convert it, warp, and
   check the depot's tanks.

## 6. Open questions for Josh

- **Q1 Endpoints.** Start with KSC-plus-depot routes now (recommended), or build colonies first and
  only allow colony-to-colony routes like KSP2 intended?
- **Q2 Creating routes.** Record a real flight and convert it (KSP2's model, recommended), or let
  players plan a route on paper (pick craft, endpoints, the game computes delta-v and time)? And must the
  vessel come back, or are one-way runs allowed?
- **Q3 Cost.** In science mode KSC has no funds. Should KSC runs be free, cost science, or cost a
  per-run "launch" time only? (Draft: free; the limit is the time per run and one vessel's capacity.)
- **Q4 Realism.** Always succeed (draft), or a small failure chance, or require the depot to still be
  reachable (e.g. same orbit) at each arrival?
- **Q5 Visuals.** Is a dashed line and an icon enough, or do you want to see a ghost vessel flying the
  recorded trajectory in map view?
- **Q6 Anything else** you have in mind for this feature that the draft is missing.
