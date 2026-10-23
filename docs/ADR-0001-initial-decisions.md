# ADR-0001: Initial scope and technology decisions

Date: 2026-10-23. Status: accepted (decided by Josh in the project thread).

## Decisions

1. **Co-op scope: one shared campaign save.** All players progress one agency together in one universe.
2. **Hosting: LAN / self-hosted, with lightweight accounts.** The server runs on Josh's machine or LAN. Players still have accounts (name + token) so a player can disconnect and resume the same identity, focus and settings later. No third-party auth, no internet-facing hardening required at first.
3. **Rendering: most modern path.** WebGPU only via `wgpu`. No WebGL2 fallback. Target browsers are current Chrome/Edge (and Firefox/Safari as their WebGPU ships).
4. **Reference depth: full decompile for study.** `Assembly-CSharp.dll` and the in-house support assemblies are decompiled locally with ILSpy into the gitignored `reference/` folder. Vessel interaction (part graph, joints, staging, resource flow) and physics (bubble, drag, gravity, joints) are the two areas we must match closely, so they get the deepest reading and dedicated comparison tests against values from the original. Additional tooling (dnSpy/ILSpy GUI, UnityPy/AssetRipper for prefabs and serialized component data, `strings`-level scans) is used freely for insight. Nothing decompiled is committed.
5. **History: push routinely.** Small commits, pushed to `origin/main` at every milestone or at least every working session, so the history is a complete record.
6. **ECS for simulation data.** Simulation state in `ose-sim` is stored in an ECS (bevy_ecs chosen over hecs for its scheduling, change detection and query ergonomics; it runs without the rest of Bevy and compiles to WASM). Snapshots and deltas are generated from ECS change detection.

## Consequences

- `ose-client` drops the WebGL2 backend feature; simpler shaders (compute allowed).
- `ose-server` gains a tiny `accounts` module: SQLite via `rusqlite` (bundled) storing player id, display name, token hash, last focus.
- Phase 1 adds a `reference/` decompile step and a `docs/reference-notes/` folder where we write our own notes on the original algorithms (notes are ours and are committed; decompiled code is not).
- Phase 6 (physics) and Phase 5 (vessel interaction) each get an explicit "parity" checkpoint: measured numbers from the original game (delta-v readouts, stock craft masses, drag at known speeds) recorded as test fixtures.
