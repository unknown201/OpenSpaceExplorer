# Water compatibility work

2026-11-12. The water implementation now imports authored part buoyancy,
`coBuoyancy` and the optional displacement-cube name. Existing catalogs without
these fields still deserialize with defaults. Native drag-cube projected areas
correct displacement; the reference's linear submerged-bounds fraction and shallow
entry ramp replace geometric clipped-box volume in the force path. Each part keeps
its rotated bounds and depth-weighted buoyancy force point.

Liquid drag now retains entry history, decays after the initial splash interval,
and switches to low-speed rigidbody damping. Damping acts at the part mass centre;
buoyancy acts at its separate authored/depth-dependent point. Slow uncontrolled
water motion receives angular damping. The existing solid-contact and suspension
paths remain in place.

Entry destruction compares the greater of downward surface speed and 10% of total
surface speed with 1.2 times that part's crash tolerance. This catches high-speed
horizontal impacts, uses each newly entering part's tolerance, preserves survivor
motion, and is checked again after a part leaves and re-enters the water. The
normal struck-part destruction path handles the result.

## Evidence boundary

Source-derived formula and production integration tests pass. They cover floating
equilibrium and sinking, projected-area displacement, the shallow entry ramp,
buoyancy force point, drag history, strict vertical/horizontal destruction
thresholds, survivor motion, and re-entry. Import tests cover authored fields and
old-catalog deserialization.

The [live capture and comparison tools](../../tools/water-parity/README.md) provide
eight fresh-capsule scenarios, standalone plots, fixed tolerances, provenance hashes,
and separate failure statuses for measured gaps and missing evidence. The OSE
production traces are captured locally in `.cache/water-parity`.

**Live compatibility is not yet verified.** Initial native setup attempts are
excluded. A conflicting docking recorder required an isolated launch; Windows
Security subsequently presented a network-access prompt requiring user dismissal.
No unfinished or failed capture is accepted as parity evidence. Run the analyzer
after a complete native capture before claiming matching splashdowns, equilibrium,
bobbing, drag, righting or destruction.

## Validation

- `cargo test --offline --workspace --exclude ose-bot`: passed, including 171
  simulation tests at the time of the run.
- `cargo clippy --offline -p ose-sim -p ose-importer -p ose-data --all-targets -- -D warnings`: passed.
- `cargo check --offline -p ose-client --target wasm32-unknown-unknown`: passed.
- Python comparison tests: passed.
- Full workspace test: failed in the existing timeline path
  (`timelines.rs:28`, missing entity) during the bot Mun flight, followed by poisoned
  server locks. This failure occurred during ascent and did not exercise water.

Animated drag-cube weighting, arbitrary compound/hollow-hull runtime validation,
flooding, waves, planing, Kerbal swimming, save/load continuity of drag history and
browser splash presentation remain outside this increment.
