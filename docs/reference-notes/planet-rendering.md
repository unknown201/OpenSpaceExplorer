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

## 6. Sunlight

Directional light from the star; colour at a point = solar irradiance x atmosphere transmittance towards the sun;
sky ambient from the irradiance LUT; terrain and parts in a body's shadow are dark (no eclipse code beyond that
in the renderer).

## Minimal functionality list (what a player needs to see), and what we implement

| # | Feature | Original | v1 here |
|---|---|---|---|
| 1 | Planet surface you can descend to without popping | PQS quadtree, levels 3..12, thresholds above | Cube-sphere quadtree with the same thresholds; 33 x 33 vertices per quad with skirts, built on the CPU over frames |
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
