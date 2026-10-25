# Reference notes: planet rendering (terrain surface, ocean, atmosphere, clouds, LOD, sunlight)

From the decompile (`KSP.Rendering.Planets`: `PQS`, `PQJob`, `PQSGlobalSettings`, `PQSData`, `PQSRenderer`,
`WaterManager`, `VolumeCloudManager`; `KSP.Rendering.AtmosphereModel`, `KSP.Rendering.Utility.AtmosphereConstants`,
`KSP.AtmosphereScattering.*`; `KSP.VolumeCloud.*`, `ScaledCloudConfiguration`) and from the assets in the
`celestialbody-shared-<body>` / `celestialbody-simulation-<body>` bundles (dumped with UnityPy). The shaders
themselves are compiled DXBC and were not decompiled; what they do is inferred from their property names and
values, and says so below.

## 1. Terrain mesh: PQS quadtree

- A cube sphere: six root quads, each a quadtree. `PQSGlobalSettings.subdivisionInfo` (one shared asset,
  `pqs_global_settings`): `minLevel 3`, `maxLevel 12`, `minDetailMultiplier 10`, `subdivisionThreshold 1`,
  `disableSubdivRadiusMultiplier 3`.
- `PQSGlobalSettings.Setup`: `subdivisionThresholds[i] = radius * minDetailMultiplier / 2^i * subdivisionThreshold`,
  collapse thresholds equal. `PQJob.UpdateSubdivision`: a quad subdivides when `level < minLevel` or
  `gcDist < threshold[level]`, collapses when `gcDist > threshold[level]` or `level > maxLevel`; `gcDist` is the
  great-circle distance from the camera's ground point plus altitude minus the quad's angular size. Kerbin:
  6000 km at level 0, 1.46 km at level 12.
- Subdivision stops (the planet stays at its current quads) above `disableSubdivRadiusMultiplier * radius`
  altitude (1800 km for Kerbin).
- Vertex height: `PQS.GetSurfaceHeight` (see `terrain.md`): `radius - oceanAltitude + global map * heightMapScale +
  tiled detail layers + decals`. The tiled layers (`largeR..A`, `mediumR..A`, `subzone3/4R..A`: a height map, a
  `uvScale` repeat count and a `heightScale` in metres each) are weighted by the biome mask channel they belong to.
- Collision fidelity (ours): the server and the client read the same terrain pack and the same
  `TerrainMap::height_m`, so the only gap is the drawn mesh being linear between grid vertices.
  `examples/terrain_fidelity.rs` measures it on Kerbin: level 12 (7.2 m vertices, drawn within about 730 m of
  the camera) median 0.01 m, p99 0.10 m, max 0.4 m; level 11 p99 0.37 m, max 2.2 m; level 10 p99 1.3 m.
  A landed craft therefore sits within half a metre of the ground it is drawn on.
- The KSC campus: the server stands craft on the campus scene's own planes (`ose_sim::campus`, areas from
  `KSC_AREAS`), the client draws the campus model there and sinks the terrain 20 m under it;
  `examples/ksc_ground.rs` checks the spawns against the server (pad 1 exact). Two known gaps: the client's
  sunk area (all facility boxes) reaches the Boat_Launch box (scene x 2048 to 3374) where the server's campus
  ground stops at x 2048, so there the server uses the terrain; and the terrain pack's launch-pad flats for
  pads 2 to 4 sit at the install's site coordinates, 400 to 900 m from the campus model's pads (pad 2's and
  pad 4's partly outside the campus, as small plateaus), since the campus is fitted to all four pads at once.
- Scaled space: beyond `heightMapInfo.scaledToLocalTransition` (Kerbin 5,000 km, blend 500 km) the body is the
  scaled-space sphere with `<body>_scaled` material (albedo `_MainTex`, normal, packed map, pole textures).

## 2. Surface material (`<body>_mat`, `PQSData.materialSettings.surfaceMaterial`)

Textures (Kerbin): `_AlbedoScaledTex` = `kerbin_scaled_nowater_d` (4096², equirectangular, the sea floor coloured
instead of water), `_NormalScaledTex`, `_PackedScaledTex`, `_BiomeMaskTex` (4096² RGBA: one biome per channel;
on Kerbin R covers most land, B the polar caps, G a few patches; A, whose slots are snow, dirt, rock and grass,
is presumably the mountains), `_SubzoneMaskTex`, global/large/mid/sub-zone
gradience (slope) and curvature maps per channel, `_SmallAlbedoArray` (13 slices of 2048² albedo + height in
alpha), `_SmallNormalArray` (normal, smoothness, AO), decal arrays, `_ShorelineTex` (ocean height SDF).

Per biome channel X in RGBA the material carries `_SmallBiomeX` = four slice indices into the small arrays
(Kerbin R: rock 8, dark grass 5, dirt 3, bright grass 4), and for each of the four layers `HeightParams`,
`SlopeParams`, `GradMapWeights`, `CurvMapWeights`, `Tint`, plus `_SmallBiomeHeightEnableX`/`SlopeEnableX`,
`_SmallUVScaleX`, `_SmallHeightWeightX` (height-blend), `_SmallWeightSoftnessX`, `_SmallNormalStrengthX`.
Inferred: each layer's weight is a window on terrain height and slope (min, max, blend), layers blend by their
albedo-height, and the result is tiled in world space (`_TriplanarUVScaleOffset` 10, `_TriplanarContrast` 4,
anti-tiling `_AntiTile*` and distance resampling `_DistanceResample*` at 100 / 1000 / 4000 / 10000 m).

Distance fades (`(start, end, min, max)` in metres): `_AlbedoScaledFadeParams (0, 160000, 0.3, 1)`: the scaled
albedo covers the detail fully from 160 km and still contributes 30 % at the ground; `_NormalScaledFadeParams`
around 80 km.

## 3. Ocean (`WaterManager`, `PQSRenderer` ocean part, `<body>_ocean_mat`, `OceanWaveSpectrum`)

- A separate sea-level sphere (`OceanQuadMeshGenerator`, quads like the terrain) at `radius`; only bodies with
  `hasOcean` (Kerbin, Eve, Laythe).
- Waves: an FFT spectrum (`OceanWaveSpectrum`: wind 10, fetch 500 km, 14 octave powers) rendered into animation
  textures near the camera, plus a sum-of-waves set (`_Wavelengths`, `_Amplitudes`, `_WaveDirX/Z`, `_Phases`) and
  scrolling normal maps (`_DetailNormal*`, `_LargeNormalMap_1/2` with tiling 500 m and 4 km).
- Colour: `_DeepWaterColor (0, 0.34, 0.62)`, `_ShallowColor (0.08, 0.36, 0.38)` over `_ShallowMaxDepth 343 m`,
  `_DiffuseColor`, sky reflection from the atmosphere (`_EnableAdvancedSkyColor`), Fresnel `_FresnelPower 15`,
  sun specular `_SpecularStrength 203`, smoothness 0.865, foam by the shoreline SDF, caustics and under-water
  fog. From orbit the water blends into the scaled albedo (`_ScaleAlbedoStart/EndHeight 66 km / 160 km`).

## 4. Atmosphere (`AtmosphereModel`, Bruneton precomputed scattering)

- The model is Eric Bruneton's precomputed atmospheric scattering: per body a transmittance LUT (1024 x 128),
  an irradiance LUT (64 x 16) and a combined scattering LUT (3D, `NU 8 * MU_S 32` x `MU 128` x `R 32`, Mie in
  alpha), all RGBA float, baked into the bundle (`<body>_atmosphere_*_lut`). Lengths in kilometres.
- Uniforms (`AtmosphereModel.BindToMaterial`): bottom radius 600, top = bottom + `AtmosphereHeight` (60),
  `mu_s_min = cos(SunZenithAngle)` (111.4 deg), `MieAnisotropy` 0.113, sun angular radius `SunAngleRadius * 0.1`,
  exposure `Exposure * 0.1` (4.12, 5.0), solar irradiance `SolarIrradiance * 2`, Rayleigh `RayleighScattering *
  scale * 0.1` per km (0.0175, 0.041, 0.1), Mie `MieScattering * scale * 0.01`.
- Used for the sky (camera ray), aerial perspective on terrain and parts (`AerialPerspective`: a camera volume of
  transmittance and in-scatter), the sun's colour at the ground (transmittance) and sky irradiance (ambient).
  Gas giants use a separate keyword.

## 5. Clouds (`VolumeCloudConfiguration`, `ScaledCloudConfiguration`)

- Near the planet: ray-marched volumetric cumulus layers (Kerbin "Fluffy Clouds" 7,000 to 9,500 m, coverage from
  a distribution cubemap, 3D Perlin-Worley noise, wind, god rays, cloud shadows).
- Far: `bakedScaledTexture` cubemaps (`kerbin_scaledclouds_01`, 2048² RGBA half; coverage in alpha) drawn on a
  shell, with a bottom texture for the view from below and a normal cubemap.

## 5b. Against the original's screenshots (2026-10-25)

Compared with the official KSP2 screenshots (OpenCritic media page: Kerbin from low orbit, the KSC from orbit,
Laythe's beach, Eve and Duna from orbit). Changes made from it:
- Far clouds: the baked coverage read as a soft blanket from orbit; it is now sharpened into cumulus fields with
  gaps (smoothstep 0.24 to 0.74, broken by 1.5 and 0.75 km noise on the sphere), thin edges greyer.
- The sea blends into the scaled albedo between 66 and 160 km (`_ScaleAlbedoStart/EndHeight`), so Kerbin's
  oceans are the original's deep navy from orbit instead of a pale sky-tinted cyan.
- Aerial perspective over the ground is eased to 60 % as the camera climbs from 10 to 40 km: the original's land
  stays vivid green from orbit (its aerial perspective is a camera volume of limited reach). The sky is unchanged.
- Lagoons: clear seas (opaque depth over 1 m) light their shallows turquoise around coasts and islands, falling
  off over 8 % of `_ShallowMaxDepth` (Kerbin 27 m), visible from orbit as in the original.
- Land from high up gets back 30 % saturation as the camera climbs from 10 to 40 km (the original's grading).
Still different: the original's open sea is a brighter blue from 40 to 70 km. Fixed: on Laythe, standing on raised ground, a thin purple line crossed the ground where the view meets the
sea-level horizon. Low down (below 2 km, blended out by 6 km) the in-scatter to a point is now the full ray's
in-scatter times the share of its extinction the point covers (well conditioned, unlike the difference of two
nearly equal LUT values), blended across the LUT's horizon switch with the sky side sampled 0.03 above it (its
rows next to the switch are blurred with the ground half).

Open, on Laythe's surface against the beach screenshot: the original's sky is a clear saturated blue and its
sand a light tan, ours grey-blue and darker brown. Not the Mie haze (a third of it changes little) nor the
exposure alone (1.6x brighter is still grey). The only post-processing profiles in the install's scenes are the
menus' (PPv2 `ColorGrading` with the Neutral tonemapper, temperature 5, bloom, vignette); the flight camera's
grading is set up elsewhere, perhaps in code. The Neutral tonemapper itself is not the difference: its curve is
within 0.03 of our `1 - exp(-x)` from 0 to 3, so the gap is in the radiance (exposure or the LUT's colour).

## 6. Sunlight

Directional light from the star; colour at a point = solar irradiance x atmosphere transmittance towards the sun;
sky ambient from the irradiance LUT; terrain and parts in a body's shadow are dark (no eclipse code beyond that
in the renderer).

## Minimal functionality list (what a player needs to see), and what we implement

| # | Feature | Original | v1 here |
|---|---|---|---|
| 1 | Planet surface you can descend to without popping | PQS quadtree, levels 3..12, thresholds above | Cube-sphere quadtree with the same thresholds; 33 x 33 vertices per quad with skirts, built on the CPU over frames; vertices geomorph onto the parent quad's surface between 1.4 and 1.9 x the split distance, so level changes do not pop (the nearest ~500 m never morphs and matches the physics ground) |
| 2 | Ground where the physics puts it | `GetSurfaceHeight` | Vertex heights from `ose_data::terrain::TerrainMap::height_m`, the server's own function: the terrain pack's global map, tiled layers and decals (terrain.md), or the embedded global map without a pack |
| 3 | Planet colour from orbit to ground | scaled albedo + biome detail | Scaled albedo (no-water version) everywhere, faded to 30 % at the ground under biome detail |
| 4 | Ground texture up close | biome mask, small arrays, rules, distance resampling | Biome mask picks the four small slices (land with no biome weight takes the first biome); height and slope windows weight them; tiled at 8, 64 and 512 m like the distance resampling; the slices' normal maps within 600 m; detail modulates the albedo so large-scale colour stays the original's |
| 5 | Water | ocean sphere, FFT waves from `OceanWaveSpectrum`, colours, Fresnel, glint, shoreline foam | Sea-level shell where terrain is below it; FFT waves synthesised on the CPU each frame from the body's own Crest spectrum (two patches, 256 m and 32 m, ocean_fft.rs) displacing the sea within 1.5 km and giving its normals and crest foam, mipped by distance, with the analytic swell beyond; depth colour from the material's shallow/deep colours; Fresnel sky reflection; sun glint; the material's foam texture where the sea is shallower than `_ShorelineFoamMaxDepth` |
| 6 | Sky and aerial perspective | Bruneton LUTs | The body's own LUTs, Bruneton's `GetSkyRadiance` / `GetSkyRadianceToPoint` in WGSL, exposure tone map |
| 7 | Clouds | volumetric + scaled shell, cloud shadows | Below 40 km a ray march through the layer (coverage from the baked map eroded by noise, lit by a short march towards the sun, two-lobe phase, depth-tested against craft and terrain) out to 24 km; beyond that the baked shell; the cover along the sun ray shades the ground and sea; no god rays (Kerbin's `enableGodray` is 0) |
| 8 | Sunlight and exposure | light + transmittance, CelestialBodyLighting ambient, post-process auto exposure | Sun colour through the transmittance LUT, sky light from the irradiance LUT; the body's own ambient colours (day on airless bodies, night everywhere); a fixed exposure per body that puts its mean albedo where Kerbin's sits, standing in for the auto exposure |
| 9 | Scaled vs local | swap at 5,000 km | One path: the quadtree at low levels is the scaled body |
| 10 | Rocks, plants, trees | Vegetation Studio Pro packages per biome | The packages' prefabs placed by their spacing, density, scale, height, steepness and noise rules near the camera (section 7) |
| 11 | Gas giant (Jool) | `IsGasGiant`, no PQS surface, banded scaled map | A smooth sphere with the banded albedo under the atmosphere; no height map, scatter, ground detail or cloud layer (Jool's layer starts at 0 m) |

Checking: `?planet=<Body>&lat=..&lon=..&alt=..&heading=..&pitch=..&sunaz=..&sunel=..` opens the client on a planet
preview (planet_preview.rs) that draws only that body from the viewpoint, the way flight would; `debug=1..6` shows
the biome mask, detail ratio, detail inputs, albedo, sunlight or normals.

Deviations: quads split below 2.5 x radius / 2^level instead of 10 x (about 1.6 quad edges, so a browser builds
a few hundred quads, not a thousand), and the shallowest level drawn is 2, not 3. The KSC hub scene's flat 60 km
ground plane (`ksc.glb` mesh `ground`) is hidden in flight once the terrain draws.

## 7. Scatter (Vegetation Studio Pro)

Rocks, boulders, plants and trees come from `VegetationPackagePro` assets in each body's simulation bundle, one per
biome mask channel (`VegetationSystemPro.BiomeTextureMask<C>ChannelType` names the package's `BiomeType`). Each
`VegetationInfo` has a prefab, `SampleDistance` (grid spacing), `Density`, a scale range, `UseHeightRule`
(`MinHeight`/`MaxHeight`), `UseSteepnessRule` (degrees), noise density/cutoff rules and a render distance from the
system's `VegetationSettings` (plants 150 m, trees +200 m, medium and large objects half of 2500 m) times the
item's `RenderDistanceFactor`. `tools/export_scatter.py` writes the prefabs (LOD0, one mesh per submesh so bark and
leaves keep their materials; foliage blended instead of alpha-tested) and `scatter.json`; `planet_scatter.rs`
places them on a planet-fixed grid on the camera's cube face with hashed jitter, density, scale and turn, the biome
weight as probability, and the rules; at most 120 per item and 900 in all, nearest first, props under 0.4 m left
to the ground texture. Grass is not drawn. The original's PQS texture sample rules are unused on the stock bodies.

Since v1: grass (Vegetation Studio's grass items: the patch texture between the dry and healthy tints, drawn as
crossed cards in one instanced batch within 100 m); under water (fog and light falling off with depth, the surface
from below with Snell's window); eclipses (the disc overlap of up to four bodies with the star, on ground, sea,
clouds, grass, sky and props); terrain packs and atmosphere LUTs served gzip-compressed.

Then: caustics on the sea floor (the foam texture's caustics channel, tiled and animated per the ocean
material, fading with depth); far trees and rocks out to 2.5 km as the original's Amplify octahedral impostors
(atlas frame chosen by view direction, brightness matched to the meshes); volumetric clouds (table row 7); gas
giants (row 11).

FFT waves: `OceanWaveSpectrum` is Crest's: 14 octaves from 2^-4 m, each a log10 variance density per rad/s
(`_powerLog`, disabled octaves at the floor), wind 10 m/s, fetch 500 km, direction spread `_waveDirectionVariance`
(Kerbin 90 deg, Eve 122, Laythe 97). Exported as planet.json `ocean.spectrum`; Kerbin's gives a significant wave
height of about 0.7 m. Gravity is 9.81 for every sea (the spectrum is tuned for it; `_gravityScale` unused).

God rays: every stock cloud configuration has `enableGodray` 0 (Kerbin, Jool, ...); the original's god rays are
the ocean's underwater ones (`OceanFeatures.GodRayQuality` 3 on high). Here: a full-screen march through the water
in front of the camera, each sample lit by the caustics where its refracted sun ray crossed the surface. Under
water the sea surface draws without its LOD skirts (they hang below it).

Jool: `jool_scaled` has `_FlowmapSpeed1` 0.22 and `_FlowmapStrength1` 0.25 but no flow texture assigned; the
bands drift here by alternating zonal jets by latitude (two cross-faded phases), exported as `surface.flow`.
Its north and south pole textures are the main map again, so no pole blend is needed.

Poles: the equirect albedo and cloud maps pinch into a starburst at the poles; `tools/export_poles.py` writes
azimuthal caps (30 deg around each pole, rows box-filtered by latitude first) that the client blends in inside
about 20 deg of each pole. Clouds: the baked cubemaps (Kerbin and Laythe 6 x 2048², Eve and Duna 1024²) are
unwrapped bilinearly to 4096 x 2048 (nearest sampling had aliased the noisy coverage into speckle), alpha only,
and the shell samples them with a cubic B-spline so they stay soft when magnified from low orbit.

Ground colour (open): `_AlbedoScaledFadeParams` weights the scaled albedo at the ground (Kerbin 0.3, Eve 0.2,
Mun 0.5) and the biome detail takes the rest, but the detail slices are not in true colour (their linear mean is
15x Minmus's scaled albedo, 9x Moho's, 1x the Mun's), so the original tints them in a way the material data does
not show. A straight lerp gives white Minmus and tan patches on the Mun along biome mask edges; detail stays a
luminance ratio on the scaled colour here. Eve's ground therefore stays as dark as its scaled map.

Relief: the original shades its tiled height layers with matching normal maps (`_LargeNormalR..A`,
`_MidNormalR..A`, e.g. `rock_mun_crater_01_n`). Here the same layers' slopes come from the terrain pack itself
(`ose_data::terrain_detail::Detail::layer_slopes`, an 8-layer RG16F array with mips) and the pack's own layer
mask (not the scaled biome mask; they differ), projected and blended exactly as the physics heights; the shader
adds only the slope finer than twice the mesh spacing, so craters and ridges stay crisp at any LOD and agree with
the ground the server simulates. `debug=7` shows it; preview `exp=` scales the exposure for tuning.

Terrain shadows: the original casts terrain shadows with its shadow maps. Here each terrain pixel marches 20 steps
towards the sun over the global height map (R16F with mips, `global_height` / `terrain_shadow` in planet.wgsl),
from half a texel out to ~90 km, softened over 0.03 rad; start and samples both use the global map so the ground
never shadows itself (the tiled layers and decals shade through the relief normals instead). The polar caps are now
one `poles.png` (albedo RGB, cloud coverage A): the grass and impostor pipelines were at WebGPU's 16 sampled
textures per stage.

Exposure after the sRGB fix (fx_post compose now encodes): bodies with an atmosphere use their AtmosphereModel
`Exposure` x 0.1 as Bruneton's tone map intends (Kerbin 4.12, Duna 4.04, Eve 3, Jool 5), floored at 2 (Laythe's
0.9 relies on the original's auto exposure); airless bodies keep the mean-albedo rule. Eve stays largely overcast:
its baked cloud coverage averages 76 % (Kerbin 11 %). Cloud colour: where the config sets `enableColorMap`, the layer's
`cloudColorMap` cubemap (Eve's purple and pink swirls, Duna's dust) goes into clouds.png RGB and tints the shell and the
volume by `colorMapIntensity` (Eve 3, Duna 2); other bodies' clouds are white.

Stars: the original's skybox (`skybox_spacescape_starfield`, 4096 x 2048 equirect, the menu scene's) draws behind the
sky in the planet sky pass, on that pass's own bind group (the quad pipelines are at the texture limit). Its frame
turns with the sun's direction in the body's equatorial plane, so stars cross the sky with the day; it is divided by
the exposure so the tone map shows it at about its own brightness in space, and the daytime sky hides it.
Ambient colours (CelestialBodyLighting) are decoded from sRGB like every Unity colour in a linear-space project;
raw, Kerbin's night ambient lit the ground about nine times too bright. The volume clouds' fill light is day-only.

The terrain shader is `KSP2/Environment/CelestialBody/CelestialBody_Local` (sharedshaders_assets_all.bundle, path id
7486954214038344636). Its properties have no tint, hue or saturation for the small albedo array
(`kerbin_textureconfig_albedoHeight_tarray`: albedo RGB, height A for height blending), and
`_AlbedoScaledFadeParams` defaults to (0, 20000, 0.5, 1), so the scaled albedo is most likely lerped in with weight
min..max over start..end; the 16 passes' compiled programs are in the packed per-platform blob, which UnityPy does
not unpack, so the exact blend is unconfirmed.

Planet detail (Settings > Graphics, `ose.planetQuality`, preview `quality=`): high draws everything; medium halves
the terrain shadow and volume cloud march steps and keeps grass within 55 m; low draws no volume clouds (the shell
only), terrain shadows, per-pixel relief, FFT sea (the analytic waves), light shafts, grass or far impostors. At 5K
on the dev machine: Kerbin meadow 314 / 214 / 183 fps (low / medium / high), Mun 505 / - / 345.

Minmus's scaled albedo (`minmus_scaled_d`, BC7) is the only one imported as linear (`m_ColorSpace` 0); the export
now sRGB-encodes linear-flagged albedo, which lifts Minmus from near-black teal to its pale grey-mint with teal flats
(mean albedo 0.033 -> 0.19). Props: the renderer lights rock and tree meshes as albedo x (0.3 + 0.7 x sun) with no
exposure, darker than the terrain beside them; their sunlight is now scaled to the terrain's exposure (x1..2.5,
eased above exposure 6 because dark bodies' rocks reuse bright stock textures), and the far impostors are lit the
same way (then taken back through the tone map for the aerial haze) so a prop keeps its brightness at the switch.

Colour spaces (all bodies checked): scaled normals, detail normal arrays and cloud cubemaps are linear and the
detail albedo arrays sRGB, as the client samples them; Minmus's scaled albedo is linear (handled above); the biome
masks of Minmus, Duna and Bop are sRGB, so the GPU decodes them in the original before weighing biomes - the export
now stores them decoded. Anti-tiling: the detail slices at the 8 m and 64 m tilings (albedo and normal) blend in a
turned, shifted copy by low-frequency noise (the original's `_StochasticScale` sampling), on medium and high detail.

Sun: the disc goes into the HDR target at 24 x (tinted by the air's transmittance) so fx_post's bloom glows around
it, with a soft corona (0.55 e^(-a/0.012) + 0.12 e^(-a/0.08) beyond the disc's edge) in the sky pass; terrain and
props hide both. Lens flare: lens_flare.rs, a full-screen additive pass the renderer draws into the HDR scene after the engine
effects and before the post pass (so it blooms and is encoded with the rest); ghosts along the line through the
screen centre, a thin horizontal streak and a faint glare, hidden by the depth buffer sampled over the sun's disc
(terrain, craft, the limb) and faded and reddened by the air near the horizon. The planet renderer requests it
each frame (`Renderer::set_flare`).

Scatter biomes: Vegetation Studio places by its own `BiomeTextureMask` (kerbin_scatter_mask, dres/vall_scatter_mask,
else the body's scaled or biome mask), not the terrain pack's layer mask; it is exported per body as
`scatter_mask.bin` (1024 x 512 RGBA, decoded where imported as sRGB) and `ScatterFile::weights` reads it. Items'
densities are multiplied by `VegetationSettings` <Kind>Density (Duna objects 0.5 / medium 0.4 / large 0.5, Minmus
0.5 / 0.2 / 0.25, Eve 0.5, Laythe/Tylo/Vall large 0.5), which had been missing: Duna was a boulder field.
Detail normals are resampled with distance like the albedo (8 m near, 64 m beyond ~30 m, out to 2 km).

Rings, aurora, city lights: the stock system's only ring is Dres's (scaled bundle: a ring component with
`innerRadius` 414 km, `outerRadius` 440 km, `alphaEdge` 0.1 and a 512 x 4 radial texture `dres_scaled_ring_01_d`, in
the equatorial plane; plus an asteroid particle field, not drawn). `tools/export_rings.py` writes ring.png and planet.json
`ring`; the client draws a 256-segment strip (vs_ring / fs_ring) lit from either side and in the body's shadow. No
body has aurora or night lights (Kerbin's `_EmissionScaledTex` is unset).

Export pitfall (fixed 2026-10-25): Pillow's `Image.resize` resamples RGBA with premultiplied alpha, zeroing RGB wherever
alpha is 0. Every planet texture whose alpha is data was damaged when resized: the biome masks (A = a biome's weight:
the other three biomes vanished wherever it was 0, which is why so much land read as "no biome"), the detail albedo
arrays (A = height) and the scaled normal maps (A = normal x). `export_planets.resize_bands` resizes bands separately.
The scatter masks are 2048 x 1024 (Kerbin's KSC grassland strip is lost at 1024 x 512). Where the vegetation mask has
no biome, nothing grows (Vegetation Studio places nothing there; the first-biome fallback only applies to the
terrain pack's mask). Streaming: a quad costs ~1.7 ms to build (native; 6 ms per frame budget in the browser), and one
that arrives after the camera is already inside its range morphs in from its parent over 0.4 s instead of popping.

Map view: the focused body is drawn by the flight view's planet renderer (`FlightView::planet_renderer`, as an
extra pass of the map frame), so it has the same clouds, atmosphere, terrain and starfield as in flight; other
bodies stay as their scaled meshes. The map does not turn the bodies, so the model frame is the map frame there.

