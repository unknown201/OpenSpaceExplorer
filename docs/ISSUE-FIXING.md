# Fixing issues through reference comparison

This is the project standard for fixing gameplay, physics, controls, and presentation
differences from KSP2. The expected result is a working change backed by a detailed
comparison and reproducible evidence. The [space EVA exit fix](reference-notes/eva-space-exit.md)
demonstrates the level of investigation and reporting we want.

## 1. Reproduce the player experience

Turn the report into a short sequence of player actions with a concrete expected result.
Reproduce it in OSE and, when available, the installed KSP2 reference. Follow the interaction
through its useful end: for an EVA exit, that includes waiting at the hatch, releasing,
moving, returning, and boarding.

Record the starting craft, relevant part and crew, environment, altitude or orbit, attitude,
velocity, resource state, time warp, and input sequence. Match conditions that affect the
behavior being investigated. Record differences that cannot be matched; explain what
they prevent us from concluding.

Capture the OSE failure before changing it when practical. Preserve the existing worktree's
unrelated changes and use an isolated local session for experiments.

## 2. Establish what KSP2 actually does

Observe the original game's behavior and inspect the corresponding local reference code,
prefab data, authored transforms, animation clips, and tuning values. Use the evidence
appropriate to the issue: screenshots for appearance, video for transitions, and timed
telemetry for motion, forces, or response rates.

Name the source classes, methods, assets, and reference build or capture conditions in the
issue's notes. Distinguish direct observations, facts read from code or data, and inferences.
An authored constant alone does not prove the runtime result. If the original cannot be
run, state that limitation and describe the evidence used instead.

Prefer the original's authored geometry and state transitions where available. For example,
an airlock's transform is stronger evidence for an exit location than a guessed offset
from the spacecraft's centre of mass.

## 3. Write the comparison before deciding the fix

Use a table organized around player actions or observable behavior:

| Action or condition | KSP2 reference | OSE before | Required OSE behavior |
|---|---|---|---|
| Trigger the interaction | Observed state and transition | Reproduced failure | Concrete expected outcome |
| Wait or release input | Motion, timing, resources | Current result | Expected continuation |
| Return, switch, or reload | Relevant reference behavior | Current result | Expected recovery or persistence |

Include related differences revealed by the reproduction when they affect the same
experience. The EVA investigation found independent problems in attachment physics,
controls, orientation, animation, nearby craft rendering, camera distance, and boarding
reach. Fixing only the spawn offset would have left the reported experience broken.

Keep the comparison current as the investigation changes the scope. It should describe
the final result clearly enough for someone who did not follow the conversation.

## 4. Fix the cause across the affected systems

Trace the complete path from player input through protocol, authoritative simulation,
snapshots, rendering, animation, and feedback. Check persistence and vessel identity when
the interaction creates, removes, attaches, or switches entities.

Use explicit state for explicit gameplay relationships. A Kerbal holding a hatch needs a
supporting-vessel/part relationship; an independently integrated orbit cannot represent
that relationship reliably. Check coordinate conventions and units at every boundary,
especially Unity, simulation, exported mesh, and rendering axes.

Resolve additional defects necessary to complete the same player sequence. Track broader
systems separately with explicit remaining differences. Label approximations and their
constants; avoid describing a partial match as complete parity.

## 5. Verify behavior and presentation

Choose checks that can detect the original failure and meaningful regressions:

- **Simulation:** verify state transitions and relevant physical invariants, such as relative
  drift, velocity inheritance, resource use, and loss of a supporting object.
- **Protocol and persistence:** exercise real client/server messages, ownership checks, and
  save/restore where affected. Resolve saved entity relationships using stable identities.
- **Browser:** perform the player sequence through the actual UI and controls, including the
  return path. Check errors and visible feedback.
- **Visual inspection:** inspect the rendered result against the reference. A passing state
  assertion cannot establish that a character faces the hatch or that the pod remains visible.

Build the affected native and WASM targets as appropriate. Rebuild both ends of a protocol
change and regenerate changed assets. Compare the code and assets actually served by the
test server. For a rendering fix, require the intended renderer and real assets; a Canvas
fallback or placeholder mesh does not verify the WebGPU experience.

Run the relevant existing regression checks. Repeat or broaden testing when new changes,
failures, or unresolved concerns justify it. Report passes, failures, and checks that could
not run accurately; a successful build is not a successful gameplay comparison.

## 6. Deliver the comparison with the fix

Create an issue-specific note under `docs/reference-notes/` containing:

1. The reproduction and reference conditions.
2. The KSP2 / OSE-before / OSE-fixed comparison table.
3. The root causes and resulting behavior, with relevant source and asset references.
4. Verification results and exact reproduction commands or scripts.
5. Side-by-side captures and any motion or timing evidence needed to assess the change.
6. Remaining differences, approximations, and build, asset, or protocol requirements.

Keep a readable side-by-side artifact when visual comparison matters. Label each capture,
identify before/after builds, and state differences in camera, lighting, craft, or environment.
Claim pixel or timing parity only when the capture conditions and measurements support it.

Store generated captures in the existing ignored `reference/` or `.cache/` directories;
keep durable findings, capture instructions, and verification scripts in the repository.
Label links to local artifacts as local so a clean checkout does not imply they are present.
Keep decompiled reference source and game binaries out of commits.

The final handoff should state what changed, link the comparison, summarize verification,
and disclose material limitations. Completion means the reported player sequence works
and the available evidence supports the claimed improvement.
