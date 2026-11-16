# Reentry coverage and heat evaluation

Investigated 2026-11-15 after feedback that heating and deceleration during reentry
still felt different from KSP2. The original working checkout's corrections are
committed as `f745021`. Production base `fc3d4f1` already includes neighboring-cube
cross-sections, full projected heating areas, shipped aerodynamic settings, body
lift and attachment-driven updates. This release preserves those implementations
and fixes the remaining coverage edge cases below.

Reference: local KSP2 `KSP.Modules/Module_Drag.cs::SetExposedArea`,
`KSP.Modules/Data_Drag.cs::GetAeroDataForDirection` and
`KSP.Sim.Util/ThermalComponentJob.cs::UpdateReentryFlux`.

| Condition | Reference behavior | Production before | Corrected behavior |
| --- | --- | --- | --- |
| Non-axial or zero node orientation | Covers a face only when the node orientation matches its normal | Chooses the nearest face, including for invalid/zero directions | Only matching axial faces are covered; node position no longer substitutes for a zero orientation |
| Middle of a thin stack separates while a farther part survives | Coverage is derived from the remaining attachment chain | Farther cube remains a blocker even through the missing middle part | Look-through coverage requires both the farther cube and the intermediate part |
| Attachment identities change with unchanged part count | Recalculate for the actual neighbors | Count-only cache can retain stale coverage | Compares the full presence mask |
| Drag or heat evaluated directly after separation | Use current exposed faces | Relies on a preceding flight-state synchronization | Refreshes before drag or thermal evaluation as well as during synchronization |

Mach-7 analytical regressions use 2,100 m/s, density 0.01 kg/m³, unit cube drag
coefficients and controlled outlines. A 2 m² face covered by a 0.25 m² neighbor
retains 1.75 m² for drag independently of node size; rotating the neighbor changes
coverage to its 0.75 m² X face. A quarter-shadowed part uses incoming heat
`500 * 7^3 * 0.75 * 2 * 0.01^0.53`, with a temperature increment equal to flux
divided by thermal mass times the step duration. These tests retain production's
existing distinction between full heating cross-section and covered drag area.

Additional regressions cover non-axial and zero nodes, thin intermediates, changes
of part identity without a count change, staging, direct part loss, resolved drag
force and repeated evaluation without accumulating face coverage. Verification
commands include `cargo test --workspace`, `cargo clippy --workspace --all-targets
-- -D warnings`, `cargo fmt --all --check`, `python -B -m unittest discover -s
tools/tests`, `cargo check -p ose-client --target wasm32-unknown-unknown`, and
`cargo test --release -p ose-pilot -p ose-routes`.

These are source-derived and analytical corrections, not a matching live KSP2
altitude/speed/temperature trace. Thermal occlusion retains projected box outlines
rather than reference shock cones; full runtime reentry matching remains unverified.
No save or protocol change is required. Deploy from the current production lineage,
which supports save format 2; the original working checkout uses save format 1.
