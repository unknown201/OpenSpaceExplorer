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

Not yet: the tiled detail layers (they add roughly 0.5 to 1.5 km of relief on Kerbin; peaks are low), the
decals other than our site discs, and full-resolution maps (one texel is about 7.4 km on Kerbin and 2.5 km on the
Mun at 512 x 256).
