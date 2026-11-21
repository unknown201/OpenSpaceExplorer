# Remaining KSP2 comparison work

Updated 2026-11-13. Evidence baseline: locally available `origin/main` at
`fcd614ed02fe803e0b8236933be41391a04676f2`, plus the accepted local capture artifacts.
This is an evidence review; no new gameplay captures or test runs were performed.
The current `wip/gridfin-surface-deployment` checkout includes older code and
uncommitted work. Findings about the integration baseline do not certify this
checkout or the public deployment. No remote fetch was performed for this review.

Use this list for current priorities. Older feature and side-by-side reports mix
historical findings with later updates; several open labels are superseded by
implementation, tests and newer comparison notes.

## Changes since the previous list

| Area | Updated evidence | What remains open |
| --- | --- | --- |
| Water, T43 | PR #143 passes all eight matched live Mk1 capsule fixtures at 50 Hz. Maximum equilibrium error 0.0205 m; lateral velocity RMSE 0.0840 m/s. Accepted local `final-20261112c/comparison.json` reports `compatible`. | Compound hulls, different locations/UTs/timesteps, swimming, saved drag history and browser splash presentation. |
| Solar, CommNet, transmission and flags, T22–T25 | PR #144 adds live-backed service fixes: transmission timing/EC, sample retention, explicit retry after interruption, graph cadence, eclipse response, solar presentation and flag inventory/pickup. Save/load, reconnect and docking have focused checks. | Matched tracking transients, full-rating incidence calibration, exact obstruction geometry, native inventory queue behavior and flag pointer/reach interaction. |
| Camera/input equations, T27–T34 | The interaction note reports 130/130 source/configuration cases aligned after the November 8 changes. Older camera-rate, rotation-key and maneuver-handle failure labels are stale. | Synchronized raw input, wheel units, DPI/display scaling, frame cadence, smoothing and collision-limited camera motion in both running games. |
| Precision/trim, T7 | PR #146 implements left-Alt trim, sign-based 0.1/s adjustment, and Alt+X reset/throttle cut, with source-derived tests. | Matched live precision/trim response; full-input latency coverage does not establish these cases. |
| Camera modes, T26 | PR #148 implements the six reference modes, Auto switching, frame behavior and default key distinctions. | Live transitions and feel; Capture photo-mode behavior remains incomplete. |
| Aircraft/control surfaces, T10/T38 | PR #149 corrects procedural hinges, saved authority/axis settings and force positions using native runs 22/23. | A post-fix matched whole-aircraft flight replay. The old approximately 2x elevator discrepancy is a historical result pending that rerun. |
| Chutes and heating, T42 | Deployment modes and engine exhaust heating are implemented. | Live edge-case coverage for chute safety modes and omitted fuel-cell/reactor heat. |
| Joints, T13 | PR #140 removes false breaks and matches reinforcement connection multiplicities in eight designs; 128 benchmark and 16 live torque histories survive. | Exact stiffness/damping, high deformation, collision cascades and other enhancement profiles. |
| Localization/tutorials | Integration code has 12-language selection and lesson condition evaluation. English-only and Continue-only blanket descriptions are stale. | Translation/layout breadth, unsupported lesson conditions, complete lesson setup/reset/completion and novice playthroughs. |

## Recommended focus, in order

### 1. Validate one complete Exploration journey

Start a fresh campaign and use ordinary UI commands to research/build a craft,
launch, collect Mun data and a sample, transmit only data, return/recover the
sample, turn in the mission and unlock the expected technology. Compare the
mission availability/order and science totals with the installed KSP2 reference.

Exercise leaving and returning to an experiment region, insufficient crew in the
experiment's own part, power/link loss, explicit transmit retry, and save/reload
while progress is paused. Per-part crew checks and experiment pause/resume already
exist in the integration code; this task establishes their combined player behavior.

Acceptance: correct reports remain aboard, rewards are credited exactly once,
mission/technology ordering agrees, and reload/reconnect preserves the journey.
Then extend the audit to later primary missions and landmarks. Imported definition
counts alone cannot establish playable progression or a completion percentage.

### 2. Finish live flight-control comparisons

Record gimbal press/release/reversal, precision/trim, SAS pointing modes, and
resource-starved RCS with matched craft, control frame, mass, input schedule and
sample phase. Repeat the stock jet flight after PR #149, checking deflection,
angular response, climb attitude and force locations.

Acceptance: attach toleranced before/after traces to each bounded scenario. Existing
source-derived regressions establish equations; a full aircraft replay checks the
combined geometry, authority, resources and flight model.

### 3. Stress docking, landing and lifecycle transitions

Compare docking above 0.3 m/s and with sideways motion, misalignment, spin and
obstructed ports. Use flat-ground leg drops for settling, then slopes and tip-over
cases. A runway-capture scenario was merged; the old checklist still lacks an
accepted matched flat-ground result, so inspect its artifacts before recapturing.

Repeat representative operations through vessel switching, save/load, docking,
undocking and a nearby second player. Check identity, momentum, crew, resources,
part settings and in-progress states. The station assembly and rolling-plan browser
checks are useful OSE evidence, but do not establish matched KSP2 contact behavior
or a repeat of every delivery with the newest planner.

### 4. Calibrate camera and editor feel live

Capture flight/map/VAB drags and wheel movement, camera-mode changes and maneuver
handles with raw events, frame timing, DPI and before/after values. Include fine
modifiers, zoom bounds, UI blocking and smoothing. Treat the documented 130/130
numerical result as completed equation coverage, not a live hardware calibration.

### 5. Expand breadth after those journeys pass

- Water: compound/asymmetric hulls, other locations/UTs/steps and saved drag history.
- Services: tracking transients, full-rating illumination and flag interaction reach.
- EVA: full climbing, obstructed hatches, external seats, a KSP2 trace of swimming speeds (T47) and camera alignment.
- Construction: fairing/shroud geometry, authoring and persistent settings across the full journey.
- Presentation: animation/audio timing, spatial mix, remaining FX and deployment continuity.
- Content/accessibility: landmarks, tutorial completion, language/font/layout coverage.
- Performance: large craft, long saves, asset loading, weaker GPUs and multiplayer endurance.

## Evidence and reproduction entry points

Read these paths at the pinned integration revision with
`git show fcd614ed:path/to/file`; local copies in this checkout can be older:

- `docs/reference-notes/water-live-parity.md` and `tools/water-parity/`.
- `docs/reference-notes/live-services-alignment.md` and `crates/ose-sim/tests/live_services.rs`.
- `docs/reference-notes/interaction-parity.md` and `crates/ose-client/src/interaction.rs`.
- `docs/reference-notes/joint-stress-validation.md`.
- `docs/reference-notes/station-rendezvous-guidance.md`.
- `crates/ose-sim/src/science.rs`, `crates/ose-client/src/tutorials.rs` and `crates/ose-client/src/i18n.rs`.

The broad feature audit remains useful for investigation scope. Recheck each
missing-feature claim against current code and accepted evidence before assigning
work. Record implementation status, automated regression coverage, live reference
coverage and remaining scenario limits separately.
