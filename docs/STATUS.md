# Status

Updated 2026-10-23.

## What works

- Cargo workspace with seven crates (`ose-math`, `ose-data`, `ose-sim`, `ose-net`, `ose-server`, `ose-client`, `ose-importer`). `cargo test --workspace`, `cargo clippy -D warnings` and the wasm check all pass. `trunk build` in `crates/ose-client` produces a loadable page that ticks the shared sim.
- `ose-sim` has an ECS `Universe` with integer `Tick` time and a `WarpFactor` resource. `ose-math` has a Kepler solver. `ose-net` has a first protocol enum with postcard round-trip tests.
- `tools/ksp2_catalog.py` parses the Addressables catalog (keys, buckets, entries) and resolves any key to its bundle.
- `tools/extract_bundles.py` extracts every JSON data asset from the install into `assets/ksp2/data/` (gitignored): 798 files, including 400 part definitions, 22 celestial bodies, 14 resources, 87 tech nodes, 9 stock assemblies, missions and tutorials. Format is `{version, useExternal, data: {...}}`.
- `reference/ksp2/` (gitignored) holds the ILSpy decompile of `Assembly-CSharp` (6414 files) and the in-house support assemblies.

## How to run

```
cargo test --workspace
cargo run -p ose-server            # http://127.0.0.1:7777/health
cd crates/ose-client && trunk serve
.venv/Scripts/python tools/extract_bundles.py
```

Tooling installed locally: rust 1.99 with `wasm32-unknown-unknown`, trunk 0.22, wasm-bindgen-cli, dotnet 9, ilspycmd 11.1, Python venv in `.venv` with UnityPy.

## Documents

- `docs/01-ksp2-install-survey.md`: what is in the Steam install and how to read it.
- `docs/02-architecture-and-plan.md`: architecture and nine-phase roadmap.
- `docs/ADR-0001-initial-decisions.md`: scope decisions.

## Next (Phase 1)

- Map the part JSON (`data.*` fields and module list), the celestial body JSON and the resource JSON into `ose-data` with a coverage report of unmapped fields.
- Write `docs/reference-notes/` from reading `KSP.Sim` and `KSP.Navigation` in the decompile: orbit propagation, SOI handling, resource flow, part joints, drag.
- Convert the scaled-space planet meshes and textures to glTF/KTX2 for Phase 3.

## Known problems

- None blocking. The server has no WebSocket endpoint yet and the client has no renderer; both are Phase 3 and 4 work.
