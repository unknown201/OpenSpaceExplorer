# ADR-0006: 3D rendering with wgpu, fed by the original game's assets

Date: 2026-10-24. Status: accepted (Josh, client rendering thread: "The web client render should be in 3D
like Kerbal is. It should also use the Kerbal assets").

## Decision

- The client renders in 3D with WebGPU through `wgpu` (`ose-client::render`), compiled for `wasm32` only.
  The Canvas 2D map and editor (ADR-0005) stay as the fallback when the browser has no WebGPU adapter
  (`navigator.gpu` missing or `requestAdapter` returning null): the page switches to the `#gpu` canvas
  when the renderer comes up and keeps every `data-ose` hook, so the e2e session runs on either path.
- Meshes and textures come from the player's own KSP2 install. `tools/export_web_assets.py` reads the
  Addressables bundles with UnityPy and writes, under `assets/web/` (gitignored): one binary glTF per part
  (`parts/<part id>.glb`, the prefab's mesh hierarchy with node transforms, collision and re-entry LOD
  objects skipped) and per celestial body (`bodies/<body id>.glb`, the scaled-space sphere), a PBR material
  per renderer pointing at PNG textures in `textures/` (base colour, normal, emissive, metallic-gloss,
  occlusion; downscaled to 1024), and `index.json`. Unity's left-handed Y-up frame becomes glTF's by
  negating X and the quaternion's y and z; Unity's clockwise front faces then come out counter-clockwise.
- The server serves that directory at `/assets/` (`--assets-dir`, server thread). GitHub builds have no game
  install, so `tools/push_assets.py` copies the export from a machine that has one into the container's
  `/var/lib/ose/assets` through piper, the same route as `tools/deploy.py`.
- World units are kilometres and every draw is camera-relative: positions are subtracted from the camera
  in f64 and only the difference reaches the GPU, so the system's 1e8 km extent and a 600 km planet share
  one f32 scene. The map camera orbits the focus body or followed vessel (drag rotates, wheel zooms, F
  frames). Bodies draw at true scale from their GLB, scaled by `radius_m / mesh_radius`, lit from the star
  (the star itself is emissive). Orbits are cached line strips in the parent's frame; trajectory patches
  are strips keyed by a version that changes with each new prediction. DOM labels, vessel markers and the
  maneuver node are placed through the same view-projection.

## Consequences

- `wgpu`, `glam` and `bytemuck` are wasm-only dependencies; native builds and CI tests stay light, and CI's
  `cargo clippy --target wasm32-unknown-unknown` covers the renderer.
- The exported assets are about 900 MB at 1024-pixel textures and live outside the repository; a fresh
  machine needs the export and push once. Phase 9 (own assets) replaces them.
- Not yet: body rotation, atmosphere and star glow, body icons at system scale, normal and emissive maps in
  the shader, part meshes in the VAB and on the map (the next step), terrain. Back-face culling is on; the
  authored winding was verified against Kerbin after the re-export.
