# Delivery routes: design draft

Status: **agreed** (2026-11-14). Nothing is built yet. Section 2 records what Josh decided. The
first step is [rendezvous v2](rendezvous-v2-plan.md), which routes are built on.

## 1. What KSP2 had

Delivery routes never shipped in KSP2. 0.2.2 has no colonies and no UI entry for them (see
[03-ui-screens-and-flows.md](03-ui-screens-and-flows.md) line 424 and
[ksp2-feature-gap-analysis.md](ksp2-feature-gap-analysis.md) item 9). The decompiled `KSP.Game.Colonies`
namespace has a small prototype, studied from `reference/ksp2/Assembly-CSharp/KSP.Game.Colonies/`:

- `ActiveFlight` records a flight from an origin (a colony, or Kerbin). It snapshots the vessel's tanks
  (`ResourceSnapshot`, name to units) at launch, at docking with the destination, at undocking, and at
  recovery back at the origin, along with the time of each.
- Only a complete flight can be converted into a `DeliveryRoute`. A route keeps the origin cost, what was
  unloaded at the destination, what came back, and the two durations.
- `ColonyManager.OnUpdate` then replays the route **abstractly**: a timer credits the destination and the
  origin and relaunches, with no vessel in the world. A debug dialog walks the state machine, and the map
  reserves a `DeliveryRoute` item type.

We keep KSP2's core idea, which is that **you prove a route by flying it yourself**. We drop the abstract
timer: in OSE every run is a real vessel.

## 2. Decisions so far (Josh, 2026-11-14)

1. **Depots first.** Routes run between KSC and depots, which are existing vessels. Colonies come later
   as another kind of endpoint.
2. **Record a real flight first.** A player flies the route once with the craft. That proves it can be
   done and measures how much fuel it takes. Later runs don't have to match the recording exactly.
3. **Free for now.** A run from KSC costs nothing.
4. **Runs succeed by default, but they collide.** Route vessels are real physics objects. If several craft
   go for a station's single docking port at the same moment, they bump and push each other around.
   There is no traffic control. Timing is everything, and failure is fun.
5. **A real vessel flies each run on server time, with no abstract fast-forward.** Whether a route
   vessel should follow the players' time warp, or do something else, is still open (Q7).
6. **Fuel versus payload.** Fuel goes through a docking port, which is easy. Payload may need a feature of
   its own (section 3.6, decision 9).

7. **Warp: route vessels never block warp** (3.5 as drafted).
8. **A run may stay docked at the depot.** When every port is taken, no new runs are scheduled. How a
   player chooses between staying, coming home, or being left as space junk is designed in 3.10.
9. **Payload is "whatever stays docked"** for now (3.6).
10. **Players can take control of a route vessel mid-run** (3.11).
11. **Depot to depot is in the first version.** It is built by expanding the rendezvous system to work
    from launch to dock, from KSC, from stations and depots, and from the Mun, including orbital
    windows. That comes first and must be well tested ([rendezvous-v2-plan.md](rendezvous-v2-plan.md)).
12. **Landed depots wait for colonies.**
13. **End-of-run modes** (3.10) are agreed. Rotate is the default for routes that stay docked.

## 3. Design

### 3.1 Depots

A depot is any vessel a player marks as one in its info panel (map, tracking station, or flight), such
as an orbital station or a parked tank. Endpoints are `Endpoint::Ksc | Vessel(id)`, and later
`Colony(id)`. A route can also start at a depot (depot to depot); see 3.12.

### 3.2 The proving flight (recording)

Recording starts when a craft launches with **Record as delivery route** ticked in the launch
dialog. It can also be started on any vessel that is still on the pad or runway. The server records:

- the **craft** (part tree, as launched) and the launch site;
- **phase markers**: liftoff, orbit reached, each burn, arrival within a few km of the depot,
  docking (which depot, which port, and our port), each resource transfer (resource, amount, from which
  parts), undocking (which parts stayed on the depot), deorbit, and recovery;
- a **trajectory**, sampled at about 1 Hz on rails and 10 Hz in physics. Inside the depot's bubble it is
  stored in the target port's frame, so an approach can be replayed against wherever the station is now;
- the **fuel used per phase** and the **time per phase**.

The flight completes either when it is recovered at KSC (a round trip) or when the vessel stays docked at
the depot (one-way; see 3.10). Then any campaign member can **Create route** from it. The route panel shows
the proving flight's totals: fuel used, delivered, duration and Δv per phase. Those numbers are the
route's **budget**.

A recording is dropped if the vessel is destroyed, reverted to the VAB, or docks with something that is
not its depot. Revert to launch restarts it. These are the same rules as KSP2.

### 3.3 Route runs are real vessels

When a route is running, the server launches a **fresh copy of the craft** at each window and flies it with
a server-side **route pilot**. The pilot is built from parts we already have: the bot's ascent and
landing programs (`ose-bot` `ascent.rs`, `landing.rs`) and the rendezvous planner, with its launch advice
and rolling multi-burn guidance
([station-rendezvous-guidance-design.md](station-rendezvous-guidance-design.md)). The pilot sends the
same control commands a player's client does, so the vessel is simulated, synced and drawn exactly like a
player's vessel. Everyone sees it, and it can hit things.

| Phase | How the pilot flies it | Taken from the recording |
|---|---|---|
| Wait for window | Launches at the next plane and phase opportunity for the depot (launch advice) | Launch site, craft |
| Ascent | Recorded pitch-over and throttle against altitude, with staging at the recorded points | Ascent profile, staging |
| Phasing and transfer | Rendezvous planner, rolling burns | Δv budget per phase |
| Approach | Holds at about 200 m, then follows the recorded approach in the port's frame with RCS. **No collision avoidance**, and it always targets the recorded port | Approach path, port |
| Dock and transfer | Same resources and amounts as recorded, capped by what is on board and the depot's free space | Transfers |
| Undock and drop payload | Undocks the recorded joint, so the recorded parts stay on the depot (3.6) | Undock joint |
| Return | Deorbits at the recorded longitude, then chutes, and is recovered at KSC on touchdown. A one-way run ends docked | Deorbit profile |

"Doesn't need to match exactly" means the pilot replans from the live situation each time, and the
recording only supplies the shape and the budget.

**Fuel budget.** A run carries the full craft, so it can burn more fuel than the proving flight did. It
delivers whatever is left above what it needs to get home. If it needs more than the craft holds, the
run fails the way a player's would.

### 3.4 Collisions, failure and outcomes

No traffic control: route vessels, player vessels and debris all collide in normal physics. If the
recorded port is taken, the pilot still tries it and bumps into whatever is there. That is the "timing is
everything" part. Each run ends with one outcome, which is logged on the route:

- **Delivered**: everything recorded was transferred.
- **Short**: it docked but delivered less (it burned more fuel, or the depot was full).
- **Missed**: it could not dock within its budget or time limit, and it returned or stayed in orbit.
- **Lost**: it was destroyed, or it ran dry and cannot get home. Debris stays in the world.

After a Lost run the route **pauses itself** so it does not keep sending craft into the same wreck. Any
member can resume it. A depot that no longer exists blocks the route with a reason.

A launch never spawns a vessel on top of another one on the pad. The run waits until the pad is clear and
says so in the route panel.

### 3.5 Time and warp

Runs live in universe time, like everything else on the server, so there is no abstract fast-forward. The
open question is how a route vessel interacts with the players' shared warp (Q7). Draft answer:

- **A route vessel never blocks warp.** While players warp, a route vessel coasts on rails, and its burns
  run as on-rails burns (the same path the server uses for warped maneuvers).
- **Physics phases wait for real time.** Ascent, final approach and docking only run while the shared
  warp is at 1x. A run that reaches its window or its hold point during warp waits there, so collisions
  only happen when someone is there to watch.

### 3.6 Fuel and payload

- **Fuel and other resources** (LF, Ox, Mono, EC, Xenon, ore) move through a docked port using the existing
  resource transfer. The recording stores each transfer, and the run repeats it.
- **Payload, MVP: drop-off by undocking.** Whatever parts stay docked to the depot when the route vessel
  undocks are the payload: a new module, a tank, a lander, a probe. Each run brings a fresh copy and
  leaves the same subassembly behind, so stations grow one module per run. This uses only docking and
  undocking, which already exist.
- **Payload, later: cargo.** Inventory items, containers, or colony resources such as construction
  materials need a cargo system of their own (KSP2 never had one). This should be its own design.

### 3.10 End of run: return, stay, rotate, or let go

Every route has a **When delivered** setting, chosen at Create route and changeable at any time:

- **Return home** (the default for round-trip recordings): undock and fly home as recorded.
- **Stay docked**: the vessel becomes part of the depot. Each stay uses up one port.
- **Rotate**: stay docked until the next run arrives. When the next run reaches its hold point, the
  pilot undocks the oldest stayed vessel from this route and sends it home, which frees the port for the
  arriving one. This works like a crew-capsule swap at a station: one port can serve the route forever.
  Recommended default for one-way recordings.
- **Let go**: undock and park about 1 km away, which leaves space junk. Useful for spent tanks, and fun
  to clean up.

**Port check at launch.** Before each launch the scheduler counts the depot's free ports that fit the
recorded port size. Routes set to **Rotate** count their own oldest stayed vessel as freeable. With
nothing free, the route waits in the "Waiting: no free port at Station Alpha" state and launches
nothing. This only looks at ports that are already occupied. Two runs in flight for the same free port
still race for it and collide (decision 4).

**Cleaning up by hand.** The routes panel lists each route's stayed vessels with **Send home**,
**Let go**, and **Keep** (detach it from the route, so it is just part of the station and the route no
longer counts it). Send home and Let go hand the vessel to the route pilot for one last leg.

### 3.11 Taking control

Any member can **Take control** of a route vessel from its map or route-panel entry. The pilot stops,
and the vessel behaves like any of that player's vessels. The run is marked **Manual**. Whatever it
delivers before the player docks counts, and the log shows who flew it. **Hand back** returns it to the
pilot, which replans from wherever the vessel is. If the vessel is nowhere near the route any more, the
pilot gives up and the run ends as Missed. A run under manual control does not hold up the next window.

### 3.12 Depot to depot: what it adds

A depot-to-depot route can't spawn a fresh craft, because there is no launch pad at a station.
It needs a different model:

- **A shuttle, not copies.** One persistent vessel cycles between two depots and refuels at the origin
  each time. That takes a "refuel from origin" step, a stay-at-origin state, and an answer to "the
  shuttle was destroyed, now what?" (the route stops until someone flies a new proving flight or
  launches a replacement from KSC).
- **The origin pays.** Fuel and payload come out of the origin depot's tanks, so the route can run dry
  (the "Waiting: origin short of LF" state).
- **Guidance.** If both depots are in the same SOI, the rendezvous planner covers it with no ascent
  and no landing. If they are in different SOIs (Mun station to LKO), the pilot needs a transfer back
  from the Mun and an encounter that targets a station rather than a planet, which is new work.

Depot to depot is in the first version (decision 11). None of this changes the KSC-route code. The route keeps an `origin: Endpoint` and a
`vessel_source: Spawn | Shuttle(id)` from day one, from day one. The guidance and flying come from rendezvous v2 (R3 for the same SOI, R4 across SOIs).

### 3.7 UI

- **Launch dialog**: a **Record as delivery route** checkbox and a depot picker.
- **Delivery Routes panel** (KSC, tracking station, and a header button in flight and map): proving
  flights in progress, routes with status and the phase of the current run, the next window, a log of
  past runs with their outcome, the budget, totals delivered, and Pause / Resume / Delete / Rename.
- **Map and tracking station**: route vessels show as normal vessels tagged with their route name, plus
  the `DeliveryRoute` filter. Depots get a route badge.
- **Toasts** on dock, delivery, and loss ("Kerbin fuel run #4 delivered 2,400 LF to Station Alpha").

### 3.8 Multiplayer and ownership

Routes, recordings and depots belong to the shared campaign. Any member can create, pause or delete a
route, and the panel shows who created it. Route vessels are owned by the server, not by a player, until someone takes control (3.11).

### 3.9 Save format

`Campaign.routes: Vec<SavedRoute>` (craft, recording, budget, status, run log) and
`Campaign.route_recordings`, plus `SavedVessel.depot: bool` and `SavedVessel.route_run: Option<RouteRunState>`
so a run in progress resumes after a server restart. All are `#[serde(default)]`.

## 4. Out of scope for the MVP

Colonies, cargo inventory, a funds economy, crew rotation, landed depots (decision 12), and traffic control.

## 5. Build plan

0. **Rendezvous v2** first ([rendezvous-v2-plan.md](rendezvous-v2-plan.md), R1-R7): a shared guidance
   crate, a pilot library, departures, cross-SOI windows, launch windows, and return home, each one
   tested headless.
1. **Recording** (`ose-sim`, `ose-server`): proving-flight state machine on launch, dock, transfer,
   undock, recover, destroy and revert; trajectory sampling in the port's frame; depot flag; save fields.
   Tests on a scripted fuel run.
2. **Route scheduler**: spawned runs and shuttles, windows from rendezvous v2, pad and port checks,
   outcomes, auto-pause, end-of-run modes, warp rules (3.5), restart resume, take control.
3. **Client**: launch checkbox, routes panel, depot toggle, map tags, toasts.
4. **Browser e2e** against a local server, never prod: record a run with the bot, create the route,
   and watch two runs. Plus a collision test: two routes into one port at the same window.

## 6. Risks

- **The pilot is the hard part.** Ascent and landing exist in the bot, and rendezvous guidance exists for
  players, but an unattended RCS approach and docking is new. It needs its own tests against the real
  docking code.
- **Server load.** Each run in physics costs as much as a player's vessel. A few routes are fine, but
  dozens are not, so routes might need a cap per campaign.
- **Determinism.** The same route won't play out the same way twice, because of other traffic and the
  station's attitude. That is the intent, but tests should check outcomes, not exact paths.

## 7. Open questions

None. All questions were answered on 2026-11-14 (decisions 1-13).
