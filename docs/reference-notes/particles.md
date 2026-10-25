# Authored engine effects

The flight renderer imports the shipped KSP2 engine effects. The earlier shared billboard plume,
atlas and mesh-bounds flame approximation have been replaced by the original per-engine mesh layers,
morph targets, shader arithmetic, texture data, material curves and particle recipes.

## Reference reviewed

Sources are under `reference/ksp2/Assembly-CSharp/` (local, ignored):

- `KSP.VFX/ThrottleVFXManager.cs`: mode/event ownership, ignition, running, shutdown and flameout.
- `KSP.VFX/ThrottleBlendshapeData.cs`: zero-throttle/atmosphere/vacuum morph weights, material
  properties, pressure curves and the atmosphere handoff.
- `KSP.VFX/ThrottleParticleSystemData.cs`: emission, duration, lifetime, speed, size and gradients;
  activation threshold 0.001. Authored curves retain their original tangents/weights.
- `KSP.Sim.impl/AtmosphereTransitionVFXHandling.cs`: static pressure / 101.325 kPa and Hermite keys
  (0, 0, slope 3), (0.4, 0.8, slope 0.8), (1, 1, slope 0).
- `KSP.VFX/TrailingParticleEmitter.cs`: world-space trails and moving emission positions.
- `KSP.VFX/VesselSurfaceInteractionVFXManager.cs`, `FXGroundBlastContextualEvent.cs`,
  `FXKSCLaunchContextualEvent.cs`, and `VFXParameterMapping.cs`: contextual effects, impact angle,
  throttle and normalized thrust/distance mappings.
- `KSP.Modules/Module_Engine.cs`: thrust is negative local Z, exhaust positive local Z.

## Rendering and simulation

`engine_fx.rs` evaluates the source control data. `engine_fx_gpu.rs` loads each engine's authored
hierarchy and draws the original layered meshes, including three morph channels and animated vertex
displacement. Part rotation, translation, flex and mirrored-X asset coordinates share the craft's
actual part transform. Engine plume dimensions are not inferred from model bounds.

`tools/port_shader.py` translates the original `VFX_Exhaust` and `VFX_Exhaust_HeatRefraction` D3D
arithmetic to WGSL. Export fails on unsupported instructions. Register masks/swizzles, integer
bit patterns, fused multiply-add, front-face handling, interpolation and implicit texture LOD are
preserved. Export records hashes of the original bytecode alongside its disassembly. Material color
properties use linear rendering conversions; texture sRGB flags are retained.

Runtime textures contain the original BC blocks and all source mip levels, including HDR BC6 data.
There is no texture downscale or atlas repacking. PNGs are inspection previews; they are not the
rendering source. This path requires WebGPU block compression support and reports missing assets or
unsupported formats as errors rather than substituting generic flames.

The scene uses an RGBA16F target. Heat refraction samples a scene snapshot and read-only reversed
depth; luminous layers then use their source additive blend. `fx_post.rs` adds HDR bloom before
presentation. Bloom is OSE's implementation, not a port of the game's complete post-processing stack.

`authored_particles.rs` simulates source ignition/shutdown bursts, delays, lifetime/size/color curves,
looping emission, cone/box/donut shapes, local/world space, inherited motion, force/velocity modules,
velocity limits, rotations and flipbook frames. Zero burst cycle counts repeat indefinitely, as in
[Unity's burst API](https://docs.unity3d.com/ScriptReference/ParticleSystem.Burst-cycleCount.html).
Each source keeps its original `maxNumParticles`;
there is no shared 4,096-particle reduction. Sources stop emitting while existing particles finish.
The simulation clock tolerates network-clock jitter, freezes on pause and resets on large time jumps
or a reference-body/vessel change. World positions remain double precision until camera subtraction.
`authored_sprite_gpu.rs` batches sorted billboards, stretched particles and spark meshes.

Ground blast and launch smoke use their source prefabs and exported `VFXParticleParameterizer`
mappings. Original global settings normalize thrust by 1,500 kN and distance by 40 m. Launch smoke
stays at the launch origin. Ground ring/line selection uses impact angle. OSE currently estimates
the hit against a surface anchored at the last landed radius; it does not reproduce Unity's terrain
and facility collision queries.

Protocol **25** adds `static_pressure_kpa` and `firing_engines` to `FlightReadout`. The server sends
attached, fuel-fed engine indices only while producing thrust. Client and server must ship together.
The current simulator supplies aggregate throttle/thrust; independent spool, booster throttle and
alternate engine modes remain limited by the existing engine simulation.

## Assets and reproduction

From the repository root, with the existing exporter Python environment and a local game install:

```powershell
.venv/Scripts/python tools/export_particles.py
```

`--install` overrides the install directory; `--out` overrides the web asset directory. The exporter
reads the game without modifying it. The current export contains:

| Asset | Coverage |
|---|---|
| Engine/booster prefabs | 35 |
| Contextual prefabs | Ground blast and KSC launch smoke |
| Thrust transforms | 53 |
| Meshes | 16, including the source spark mesh |
| Plume materials / shader families | 28 / 2 |
| Particle materials | 10 |
| Full-resolution textures with source mips | 23 |

Generated files are under ignored `assets/web/fx/`: `effects.json` indexes resources; `engines/*.json`
contains each hierarchy and recipe; `meshes/*.bin` stores original vertices/morphs/indices;
`shaders/` contains WGSL, original DXBC and disassembly; `textures/*.texture` contains GPU texture
data. `engines.json` retains the nozzle transforms used for ground interaction. `coverage.json`
lists layer coverage for every prefab. Export scripts and OSE's own renderer are tracked.

Serve with `--assets-dir assets/web`. For an authorized deployment, `python tools/push_assets.py fx`
uploads these assets using the repository's existing asset deployment workflow. Code deployment
alone does not upload the game-derived files. `tools/make_fx.py` generates a historical cone that is
no longer used in flight.

## Fidelity checks

The shader comparison runs the **original DXBC in an independent native D3D11 renderer**, then
compares those HDR pixels with the production WebGPU renderer using identical captured uniforms,
meshes, morph weights and textures. It does not compare two implementations of the WGSL port.

With Visual Studio C++ tools and the Windows SDK installed:

```powershell
tools/build_fx_reference.ps1
.venv/Scripts/python tools/check_fx_parity.py --prepare
# Build/serve the WASM client and assets, then point this at that local server:
$env:OSE_BASE_URL = 'http://127.0.0.1:7794'
# Set OSE_CHROMIUM_EXECUTABLE if a different installed Chromium is needed for WebGPU.
node e2e/scripts/check-fx-parity.mjs
.venv/Scripts/python tools/check_fx_parity.py
```

The 36 cases cover Swivel, Terrier, SWERV, Flea, Dawn and Panther at 101.325, 1 and 0 kPa, at full and
35% throttle. A deterministic patterned background exercises refraction; separate baseline captures
must match exactly. The error metric subtracts that background and uses only affected pixels, so
empty/background pixels cannot dilute a plume error. All values must be finite and effect-relative
RMSE must be below 0.1%. Captures, original/OSE/difference PNGs and `results.json` go to
`.cache/fx-parity/`.

On the tested NVIDIA Blackwell WebGPU/D3D11 adapter, all 36 cases passed. Worst effect-relative RMSE
was **0.00406%**; 24 frames matched exactly in the captured RGB half-float output. The largest
absolute channel difference was 0.0009765625. These measurements cover mesh exhaust and heat
refraction before bloom. They do **not** establish whole-game visual parity, random Unity particle
simulation parity, RenderKit's complete particle lighting/probes/shadows, particle/engine lights, or
the original post-processing pipeline. The sprite shader preserves source maps/material controls
but recreates its lighting in OSE; those systems require their own comparison before claiming parity.

Native tests cover source curves/morph handoff, emission clocks, original caps, start delay, shutdown
bursts and throttle curves. Server WebSocket tests check idle/burning engine lists and pressure.
The WebGPU gameplay test verifies authored layers, particle emission, liftoff, complete expiry after
cutoff and restart, with a screenshot and no console errors. Firefox without WebGPU retains the
existing Canvas fallback. Reentry, RCS, explosions and other contextual events need separate event
sources and are outside this flight-engine integration.
