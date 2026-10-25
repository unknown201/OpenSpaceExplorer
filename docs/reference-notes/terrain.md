# Reference notes: terrain height

From reading `KSP.Rendering.Planets` (`PQS.GetSurfaceHeight`, `PQSJobUtil.HeightSample`, `BiquadraticSample`,
`GetSphericalUVsForLatLong`), `KSP.Rendering.Planets.PQSData.HeightMapInfo`, `KSP.Sim.impl.CelestialBodyBehavior`
and the body JSON `LocalSimObjectsData`, plus the `PQSData` assets in the install.

## How the original computes ground height

`CelestialBodyBehavior.GetTerrainAltitudeFromCenter(lat, lon)` calls `PQS.GetSurfaceHeight(dir)`:

```
surface radius = radius - oceanAltitude + HeightSample(uv, dir)
uv             = (lon / 360 + 0.5, lat / 180 + 0.5)     equirectangular, v = 0 at the south pole
HeightSample   = global(uv) * heightMapScale            (onlyGlobalHeight path)
               + large tiled layers R/G/B/A + medium tiled layers R/G/B/A, weighted by a 4-channel mask
               + pole fix-up + height decals (flattened sites such as KSC)
global(uv)     = biquadratic sample of a 16-bit map (3x3 texels, Quadratic(a,b,c,f) =
                 lerp(lerp(a,b,(f+1)/2), lerp(b,c,f/2), f)), wrapping in longitude
```

Within `HeightMapInfo`: `globalHeightMap` (R16, 4096 x 4096 for every stock body), `heightMapScale`
(Kerbin 5000 m), the tiled layers each with `uvScale` and `heightScale` (Kerbin large R/G/B/A 300/350/700/1700 m,
medium 50/100/30/700 m), `mask`, `heightMapContrast`, and sub-zone layers. The `PQSData` asset is in the body's
`celestialbody-shared-<body>` bundle for Kerbin, Duna, Eve, Laythe and Vall, and in
`celestialbody-simulation-<body>` for the others.

Altitude readouts are measured from `radius` (sea level where there is an ocean). For airless bodies
`oceanAltitude` is 0 and the terrain sits well above `radius`: the body JSON's own `MinTerrainHeight` /
`MaxTerrainHeight` agree (Mun 2126 to 3318 m, Tylo 21376 to 24174 m, Minmus 2995 to 7096 m).

## Sites

`LocalSimObjectsData` lists named objects with a Unity Y-up `LocalPosition`; children carry `RelativeTo` and are
offsets in the parent's frame rotated by the parent's `LocalRotation`. Composed and converted:

| Site | lat | lon | altitude |
|---|---|---|---|
| kerbin_KSC_Object | -0.6274 | -74.8305 | 221.7 m |
| kerbin_KSCLaunchpad_spawn_01 | -0.6836 | -74.8198 | 115.2 m |
| kerbin_KSCLaunchpad_spawn_02 | -0.4360 | -75.0270 | 111.0 m |
| kerbin_KSCLaunchpad_spawn_03 | -0.6211 | -75.1853 | 115.8 m |
| kerbin_KSCLaunchpad_spawn_04 | -0.4362 | -75.3480 | 132.0 m |

The global map alone puts KSC at -44 m (the coast); the plateau comes from height decals.

## What we implement (v1)

- `tools/extract_terrain.py` exports every body's global map, box-averaged to 512 x 256, into
  `assets/own/terrain/terrain.bin` with `index.json` (scale, offset = -oceanAltitude, sites). 16 bodies, 4 MB.
- `ose_data::terrain::TerrainMap`: the same biquadratic sampling, plus flattened discs at each site (launch pads
  250 m held at the pad altitude with an 800 m smooth falloff; other sites 1 km with 1.5 km falloff), and the sea
  surface as ground where the terrain is below it on ocean worlds.
- `ose_sim::flight` lands on `radius + ground_m(lat, lon)`; latitude/longitude are body-fixed using the body
  rotation (`initial_rotation_deg + omega * ut`), the same convention as the pad placement. The pad is
  `kerbin_KSCLaunchpad_spawn_01`. Trajectory collision tests against `radius + max terrain` like the original's
  `WillCollideWithParent`.

## Terrain packs (v2): the rest of `HeightSample`

`PQSJobUtil.HeightSample(uv, dir)` in full, as `ose_data::terrain_detail` implements it:

```
height = global(uv) * heightMapScale
       + layers(large) + layers(medium)                      tiled, weighted by the biome mask
       (pole fix-up: radius 0 on every stock body, skipped)
height = DoDecals(dir * radius + dir * height, height)       PQS height decals, in sort order
surface radius = radius - oceanAltitude + height
```

- `dir` is the body-fixed Unity direction `(cos lat cos lon, sin lat, cos lat sin lon)`.
- Layers: for each channel c of the biome mask (bilinear, normalised to sum 1), `lerp(top, side, blend) *
  heightScale_c * mask_c` where `side` samples the layer at `(2u, v) * uvScale`, `top` at `(dir.x, dir.z) / 2 *
  uvScale` and `blend = 1 - |dir.y|^heightMapContrast` (side projection at the equator, top projection at the
  poles). All samples biquadratic and wrapping.
- Decals (`PQSDecalController.UpdateDecalInstances`, `CullDecal`, `DrawDecal`): each placed `PQSDecalInstance`
  sits at `normal(lat, lon) * radius`, turned so local +Y is the normal and then by `Rotation` about it, scaled
  `Scale` metres. A point is affected when it is within `sqrt(1.5) * Scale` of the centre and its local
  position `l = R^T (p - centre) / Scale` has `l.x + 0.5`, `l.z + 0.5` and `l.y + 0.5 - 1 / Scale` all in
  [0, 1]. Then `h = (decal(uv) - 0.5 + HeightOffset) * mode * fade * HeightScale + VerticalOffset * fade` with
  `fade = saturate(shape^FadeStrength * 2) * alpha(uv)` (shape: 1 - 2 * Chebyshev or Euclidean distance from
  the centre by `FadeShape`); `HeightBlendMode` 3 adds, 1 subtracts, 2 replaces (the running height is first
  scaled by `1 - fade`). Settings come from the `PQSDecal` asset unless the instance overrides them.
- Kerbin: global map 0 to 5000 m against an ocean altitude of 4270 m (so 730 m at most from the map; the body
  data's `MaxTerrainHeight` 1499 m is the global map's), eight tiled layers, 67 decals (mountain groups around
  the KSC, the desert mountains, the lake mask under the KSC). The highest point comes out at 2.7 km. Under the
  KSC campus the ground is near sea level: the campus model carries its own ground, as in the original.
- The campus ground (`ksc_campus_VertexPaint_02`, 21 x 19 km, with `ksc_campus_final`) is not flat: a plateau
  at about 107 m (scene y -126) with lake hollows near 40 m, whose skirts slope down to about -700 m at its
  edges. The original draws it with the PQS terrain so that the higher shows, and it is the ground there. West
  of the plateau it stands 20 to 40 m above the 39 m lake country, so with the terrain alone a craft fell
  through it. `tools/export_campus_ground.py` rasterises it, placed as the server places the campus, into a
  256 x 256 lat/lon grid (`assets/own/terrain/ksc_ground.*`, 128 KB, embedded in ose-data); `height_m` takes
  the higher of terrain and that grid, `terrain_m` is the terrain alone (the client draws that under the model).
  Re-export it whenever the campus pose (`KSC_ANCHOR`, `KSC_YAW_DEG`) changes.

`tools/extract_terrain.py --packs assets/web/terrain` writes one pack per body (`<Body>.json` + `<Body>.bin`):
the global map at 4096 x 2048 (Kerbin, Mun) or 2048 x 1024, the mask at 1024 x 512, layers at most 1024 square,
decal heights at 512 square and alphas at 256 square. 15 to 41 MB per body, about 370 MB in all, so the packs
are never committed: they go to the container with the other game assets (`tools/push_assets.py terrain`).
The server loads `<assets-dir>/terrain/<Body>.*` when present and falls back to the embedded global-only maps
(which CI and the session tests use); the client fetches the same pack from `/assets/terrain/` for the body it
flies over, so the drawn ground is the ground the server lands on.

Sites: only launch pads get a flat (250 m held at the pad altitude, 800 m falloff). Base anchors such as
`kerbin_KSC_Object` (221.7 m) are reference points, not ground: a 1 km disc there stood 100 m above the campus.

Not yet: the sub-zone layers (`subzone3/4`, used by the shader only), the scatter and colliders of the campus
model itself.
