# Authored flight effects

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
looping emission, sphere/hemisphere/cone/box/donut shapes, local/world space, inherited motion, force/velocity modules,
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

Protocol **48** carries atmospheric density, Mach, temperature, gravity, per-surface lift and
fuel-fed RCS translation/rotation alongside pressure and firing engine indices. Client and server
must ship together. Solid boosters honor the source locked-throttle setting until their fuel is
spent. A throttle cut stops liquid engines in a mixed stage without consuming their propellant.
Independent spool and alternate engine modes remain limited by the existing engine simulation.

Crash transitions latch the pre-impact resources and select the source dry, methalox, solid fuel,
pressurized or metallic hydrogen explosion in atmosphere or vacuum. The burst stays at the impact
position, fires once, and does not replay when loading a wreck or jumping the simulation clock.
Wrecks co-rotate with the surface after impact, keeping the burst and wreck at the same site.
Contextual prefab preview translations/rotations are removed as in the source event instantiator;
child transforms and root scale remain authored. Source explosion potential defaults to 0.5.
The automatic flight report waits 2.5 wall-clock seconds so it does not cover the initial explosion.

Staging, fairing/clamp release, parachute deployment, docking/undocking, solid/liquid surface impact,
launch fire, wheel dust and EVA footsteps/poof use their source recipes. RCS and EVA use all 46
independently controlled nozzle groups and their original transforms rather than a shared plume.
Parameter mappings preserve the source distinction between multiplying constant ranges and
replacing curve multipliers; mapped burst counts replace the original counts.

Reentry uses 606 original authored shockwave groups across the 400 inspected parts and the source
`Shockwave` shader, with source heat gradients for Kerbin/default, Eve, Jool and Laythe. A separate
R16Float radial-distance occlusion pass uses the actual body meshes. Activation follows the source
Mach/density thermal curve. The old generic cone is no longer loaded in flight, including burnout.

Vortices use original TrailRenderer width/color/lifetime data and material, with world-space history.
Their source humidity, dewpoint, pressure and wing-lift conditions determine emission; dry/vacuum
conditions do not emit. Existing OSE wing meshes do not expose the original procedural vortex
anchors, so trails originate at the static wing bounds' outer trailing corner. The shipped science
prefabs have emission disabled and no bursts; the export preserves their silence. Effects disabled
by the source HighQuality settings (including contrails, shock cone and fuel ice) are not invented.

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
| RCS / EVA prefabs | 3 / 1 |
| Contextual prefabs | 34, including 10 explosion variants and vapor trails |
| Nozzle transforms | 99: 53 engine, 6 RCS, 40 EVA |
| Meshes | 1,194, including shockwaves and body occluders |
| Mesh materials / shader families | 29 / 3 |
| Particle materials | 31 |
| Full-resolution textures with source mips | 61 |

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
existing Canvas fallback.

Additional browser checks exercise dry/fueled crashes in atmosphere/vacuum, all exported contextual
recipes and RCS/EVA groups, particle expiry, clock jumps, wet/dry vapor trails, and actual server/UI
flight through a fueled crash and solid-booster burnout:

```powershell
node e2e/scripts/check-crash-fx.mjs
node e2e/scripts/check-flight-fx.mjs
node e2e/scripts/check-flight-impact.mjs
node e2e/scripts/check-reentry-fx.mjs
.venv/Scripts/python tools/check_reentry_parity.py
```

The reentry comparison feeds the independently executed original DXBC the captured structured
buffers and radial-depth map. Six frames cover a pod, Swivel and long fuel tank. Its separate
0.5% effect-relative RMSE budget exposes differences at discontinuous point-sampled shadow edges;
the measured worst frame was 0.28281%, with five frames exactly matching. Cold and vacuum
fixtures also verify that plasma remains off. These checks measure shader output, not whole-game
parity of atmospheric activation, occlusion rasterization or post-processing.

Remaining fidelity gaps include RenderKit particle lighting/probes/shadows, particle lights,
Unity's random particle/noise and particle ribbon modules, explosion-only distortion materials,
the complete game post-processing stack, procedural wing vortex anchors, and exact terrain/facility
collision queries. Explosion distortion is currently omitted rather than drawn as an opaque quad.
The implementation and these measurements do not establish equal-or-higher fidelity for every
effect in the original game.
