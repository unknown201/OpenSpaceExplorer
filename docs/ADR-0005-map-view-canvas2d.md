# ADR-0005: Map view drawn with Canvas 2D first, WebGPU for the flight view

Date: 2026-10-23. Status: accepted (client rendering thread).

## Context

ADR-0002 makes the browser client a thin renderer. The first thing a player needs to see is the map view: the
Kerbol system with bodies and orbits, universal time, and time warp. A WebGPU pipeline (surface, shaders, a
camera, text) is days of work before anything shows; the e2e suite (ADR-0004) also records that headless
Chromium on Linux runners only has a SwiftShader adapter and Firefox has none, so a WebGPU-only map would be
untestable in CI.

## Decision

- The map view is drawn with the Canvas 2D API: a full-window `<canvas id="map">`, orbits as sampled
  polylines, bodies as discs, SOI as faint circles. Labels are positioned DOM elements over the canvas
  (`#labels .body`), which doubles as the `[data-ose="body"]` hook and gives click-to-focus for free.
- The client keeps the server's `BodyCatalog` and places bodies from their Keplerian elements at the
  client's time estimate (`ose-client::scene`). Time is resynced on every `TimeState` and extrapolated at
  the current warp between messages, capped at 500 ms so a stalled link freezes instead of running away.
  `Snapshot` positions are kept as the fallback for bodies without elements. This is the "interpolation
  for smooth presentation" ADR-0002 allows; nothing else from the sim is in the client.
- The camera is a top-down projection of star space (X right, Y up) centred on a focus body; drag pans,
  wheel zooms about the cursor, a label click focuses, `F` reframes the focused body's system. Moons whose
  orbit is under 6 px are hidden so they do not pile onto their planet at system scale.
- The HUD shows UT in Kerbin calendar time (6 h days, 426 d years, both 1-based) and the warp ladder
  1, 2, 3, 4, 5, 10, 50, 100, 1000, 10000, 100000. The warp button sends `RequestWarp` and shows the
  requested value at once; the next `TimeState` confirms or corrects it.
- Vessels (protocol v3 `Vessels`) are drawn as triangles with a label, placed from their orbit around the
  reference body so they move smoothly between messages; the player's own vessel is followed by the camera
  until a body label is clicked. Each vessel's `Trajectory` patches are drawn as time-sampled conics around
  their body, coloured by how the patch ends (final, encounter, escape, collision, maneuver).
- A maneuver node is one planned impulsive burn on the player's vessel: `Add node` creates it two minutes
  ahead, sliders set the burn time (up to one orbit ahead) and prograde delta-v, every change sends
  `ScheduleBurn`, and the marker sits on the current orbit at the burn time. The server's `BurnState`
  reason shows in the panel; its `Trajectory` with `Maneuver` patches is what draws the result.
- The Vehicle Assembly Building starts as a 2D side-view editor on the same Canvas 2D stack
  (`ose-client::vab` for the part tree, `ose-client::thumb` for the boxes): parts attach on matching stack
  nodes from `/parts.json` geometry, the server stays authoritative for masses and delta-v through
  `SaveCraft`. A 3D editor with drag, symmetry and rotation is the WebGPU flight-view work.
- Superseded in part by ADR-0006: with WebGPU the map renders in 3D from the game's assets; this Canvas 2D path remains the fallback.
- WebGPU stays the plan for the flight view (3D vessel, terrain, scaled space), where Canvas 2D cannot
  go. The map can move to the same pipeline later; its scene model and camera do not depend on how it is
  drawn.

## Consequences

- The e2e frontier moves: `render_canvas`, `websocket_session`, `time_display`, `map_view_bodies` and
  `time_warp_control`, `player_vessel` and `maneuver_node` pass in Chromium and Firefox; only the WebGPU
  adapter step is skipped where the browser has none.
- The canvas exposes `data-ose-scale` and `data-ose-focus` for tests and debugging.
- No new dependencies beyond `js-sys` and more `web-sys` features; the client stays about 100 KB of WASM.
