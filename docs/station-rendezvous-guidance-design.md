# Station rendezvous guidance

Status: implemented first version on `codex/station-rendezvous-feature`. See
[implementation and verification](reference-notes/station-rendezvous-guidance.md).
Launch advice currently predicts orbital-plane opportunities; learned ascent profiles and
launch-to-intercept forecasts remain future work. The intercept planner uses two-body
estimates, with explicit correction replanning after finite burns.

## Intent

Make repeated station assembly flights easier to plan while preserving manual craft
construction, launch, burns, translation, and docking. Reduce waiting and orbital
guesswork; keep the satisfaction of flying the delivery.

The station core is built and put into orbit through the existing flow. Guidance becomes
available whenever another orbiting vessel can be selected, rather than on a literal
second-launch counter. Any suitable vessel can be a destination; registering a special
station is optional organization, not an unlock requirement.

## Entry and configuration

- Add an optional **Rendezvous destination** to pad launch preparation. Also expose
  **Plan rendezvous** beside the map's existing vessel target action, allowing guidance
  to be enabled after launch.
- Reuse the current target relationship. Planning a rendezvous explicitly selects that
  vessel; canceling guidance leaves ordinary vessel targeting available.
- Offer **Enable rendezvous guidance** when the player chooses a destination for the
  first time. The existing flight experience stays the default until enabled. Remember
  the player's guidance preference; do not silently choose a destination for later flights.
- Levels: **Off** (existing target instruments), **Minimal** (cues, timing, essential
  readouts), **Guided** (same instruments plus one current instruction and explanations).
- Advanced settings: world-space approach cues, instruction text, and audible timing
  prompts. Guidance level never changes physics or applies thrust.
- Selecting a destination previews compatible, free docking ports and the delivery
  craft's own port. Show occupied and incompatible ports with an explanation. Highlight
  the selected port on the station. Support port renaming as a later convenience.
- Show RCS, propellant, and control-authority limitations before launch. Unknown flight
  performance is an estimate, not a reason to claim the planned encounter is guaranteed.

## Interface

Place a compact **Rendezvous** director adjacent to the existing navball/core flight
instruments, in a user-collapsible panel. The expanded panel in the concept preview
demonstrates the content hierarchy. Its final desktop placement must account for the
existing crew portraits, app bar, and staging stack.

The director shows destination and port, current phase, one immediate action, and a few
phase-specific readouts. Only display the values needed for the current decision.
Keep the navball, speed mode, and burn countdown authoritative instead of creating a
second contradictory instrument set.

Distinguish three references with shapes and text as well as color:

1. **Station target:** existing target brackets / target and anti-target markers. This
   points to the actual station and is useful for looking at it.
2. **Flight guidance:** reuse the maneuver cue for a calculated orbital burn; provide a
   distinct labelled ascent cue if ascent guidance is available. It points where the
   craft should orient for the current task. It is often different from the station.
3. **Port approach:** port-axis reticle and a staging point outside the station's occupied
   volume. In docking, orientation comes from the chosen pair of ports, including an
   off-axis delivery port, rather than the vessel nose or station center.

A world-space meeting point is a map/planning annotation, not a direction to thrust
toward. On the navball, guidance always represents an attitude/burn direction.

## Flight sequence

| Phase | Player's decision | Guidance and feedback | Exit condition |
| --- | --- | --- | --- |
| Prepare | Select destination and compatible port pair | Station orbit, free ports, craft capabilities, approximate launch opportunity | Player confirms target |
| Launch | Wait or launch; fly ascent and circularization | Window countdown, target plane/heading, attainable insertion goal; optional ascent cue when a validated prediction is available | Stable orbit suitable for a computed transfer |
| Intercept | Align, start and stop each burn manually | Next burn direction and start time, remaining delta-v, predicted closest pass and encounter time; optional plane correction or phasing orbit as needed | A usable encounter with enough braking time becomes imminent |
| Approach | Match velocity, then move toward the staging point | Relative-velocity braking cue, range, closing speed, total relative speed, braking lead time | Low relative motion near a valid staging point |
| Dock | Align ports, translate, and close gently | Port distance, closing speed, lateral drift, port-axis error; optional roll reference | Authoritative simulation reports docking |

The intercept phase can contain several maneuvers. A missed launch window can cost an
extra orbit without becoming a failed mission. Do not imply that one perfect launch or
one burn is always required or sufficient.

**Warp to window** and **Warp to burn** are explicit player actions, with a lead-in and
automatic return to real time. Respect the game's existing powered-flight and shared
time limits. If another player prevents the requested warp, explain it in place and keep
the countdown visible. Never warp automatically because guidance advanced a phase.

During approach, compute braking lead time from available control authority and the
current relative motion. Distance alone cannot determine when to brake. Label closing
speed separately from total relative speed; lateral motion matters even when range is
steady. Use port-relative motion, including rotation, for final alignment guidance when
available. Label any prediction limits rather than inventing precision.

## Lock-on behavior

Make the action phase-specific: **Hold burn direction**, **Hold braking direction**, or
**Hold port alignment**. Tooltip: “SAS controls attitude. You control throttle and RCS.”
The existing **Maneuver** SAS radial selects the same active rendezvous cue. When no
rendezvous aiming cue is active, it follows the ordinary planned maneuver.
Use the existing attitude-hold interaction vocabulary rather than an ambiguous generic
“Lock” action that could imply automatic rendezvous.

- Holding guidance affects orientation only. It does not stage, throttle, translate,
  change warp, or capture the docking port.
- Show the active hold beside the existing SAS indicator. Manual steering disengages
  the hold using the established input behavior; **Release guidance hold** is also visible.
- Phase changes can update the suggested cue, but never silently swing the vessel into
  a new hold. Return to stability hold at the current attitude and ask the player to engage
  the newly suggested direction. Surface the handoff without changing translation keys.
- Let the player enter the existing docking control mode explicitly; do not remap controls
  because a distance threshold was crossed.
- Recalculate predictions while coasting and after burns. While a burn is being flown,
  avoid erratic cue jumps: retain the active burn intent, show residual error, and offer a
  new correction when required. Invalid predictions visibly suspend the cue/hold.

## Large station behavior

- Guide toward a staging point outside the selected port along its outward axis before
  asking the player to move down that axis. Size the stand-off to station and delivery
  geometry; the center of a large station is not a safe approach destination.
- Compatibility and occupancy do not prove a collision-free path. Initially use a
  conservative station bounds check and require visual piloting. Only label an approach
  “clear” when an obstruction check supports it. Re-evaluate as the station rotates.
- Preserve the station's name and stable logical identity through vessel merges. Remap
  destination and port references after docking, undocking, staging, or a save/load.
- A blocked or newly occupied port pauses final-port guidance and offers another
  compatible free port. Never redirect a live attitude hold to a different port silently.
- On docking, collapse the director and show **Module attached to Horizon Station**.
  Future deliveries can select the same station and another free port. No compulsory
  station management dashboard is needed for the first version.

## Recovery and unavailable states

| Condition | Interface response |
| --- | --- |
| Launch window missed | “Next window in …” plus **Find next window**; permit manual launch and later replanning |
| Intercept burn missed or materially off course | “Encounter needs a correction” plus **Replan intercept**; retain the target |
| No feasible prediction or insufficient known delta-v | Explain the limiting condition; show no fabricated burn cue or precise arrival estimate |
| Closing too fast | Prioritize “Brake now” and the relative-motion braking cue; remove encouragement to close |
| No RCS or exhausted propellant | Explain the unavailable capability and keep relevant target data visible |
| Port becomes occupied, obstructed, or incompatible | Suspend that port's final-approach cue and allow an explicit new choice |
| Target lost, removed, or leaves the relevant reference frame | Suspend the plan and active guidance hold; provide a new target action |
| Guidance turned off | Remove assistance cues, preserve the selected vessel, and return to ordinary SAS stability at the current attitude |
| Docking complete | Only report success after the server confirms capture |

Warnings use text and shape, not color alone. Use transition hysteresis so phases do not
flicker. A short successful capture message is enough; do not interrupt the flight with
a modal or a score screen.

## Recommended implementation sequence

1. **Orbital rendezvous first:** pad/map destination selection, explicit launch-window
   estimates, user-triggered warp, computed maneuver suggestions, manual burn guidance,
   and relative-speed approach instruments. A countdown or launch profile should ship
   only when backed by a planner that accounts for the destination orbit and the craft's
   estimated ascent; otherwise offer plane guidance and plan the intercept after orbit.
2. **Port guidance:** own/target port selection, geometry-aware staging point, alignment
   instruments, stable station/port references, and multiplayer occupancy updates.
3. **Repeat-flight refinement:** optionally record an actually flown ascent profile for
   the same craft and launch site to improve timing estimates. Invalidate it when craft,
   payload, staging, site, or relevant conditions change. This improves planning, not
   automatic launch execution.

Automatic launch, automatic orbital burns, automatic docking, teleporting cargo, and
delivery route scheduling are outside this proposal's initial scope.

## Fit with the current code

Existing integration points:

- `crates/ose-client/src/navball.rs`: existing maneuver, target, and anti-target vectors.
- `crates/ose-client/src/hud.rs`: existing target/anti-target and other SAS mode buttons.
- `crates/ose-client/src/lib.rs`: selected vessel target, target speed mode, docking input
  mode, and server docking feedback.
- `crates/ose-client/index.html`: navball, SAS, burn timer, target readout, and settings UI.
- `crates/ose-sim/src/docking.rs`: free-port discovery, port size compatibility, acquire
  range and facing checks, relative speed gate, vessel merge and undock operations.
- `crates/ose-server/src/crew_docking.rs`: authoritative docking notification and vessel
  registry updates.

The inspected docking implementation captures when its conditions are met; the design
therefore does not add a misleading **Dock now** button. The suggested operating speeds
and preview numbers are illustrative, not new capture thresholds.

## Experience validation

Walk through an initial manual station launch, then at least two different payload
deliveries. Check an on-time launch, a missed window, an imperfect burn, high sideways
relative motion, a fast approach, an off-axis delivery port, an occupied/obstructed port,
and a larger station after docking. Exercise Off/Minimal/Guided midflight, manual hold
override, multiplayer warp restrictions, save/load, merges, and undocking.

Success means the player always knows the destination, the next useful flight action,
which direction cue they are seeing, and which controls they still own. Evaluate reduced
planning/waiting effort and successful manual deliveries without hiding the flying.
