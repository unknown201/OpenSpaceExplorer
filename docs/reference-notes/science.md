# Reference notes: science and the tech tree

From reading `KSP.Game.Science` (`ScienceManager`, `ScienceRegionsDataProvider`, `ScienceExperimentsDataStore`,
`ExperimentDefinition`, `ResearchLocation`, `CompletedResearchReport`, `TechNodeDataStore`,
`RDCenterUIController`), `KSP.Sim.impl.PartComponentModule_ScienceExperiment` and `KSP.Rendering.Planets.PQSJobUtil`.

## Data (Addressables labels)

- `scienceExperiment`: 8 experiment JSONs (`ExperimentID`, `ExperimentType` Data/Sample/Both, `ValidLocations`
  of body / situation / region / `RequiresRegion`, `DataValue`, `SampleValue`, `TransmissionSize`, `RequiresEVA`).
- `science_region`: per body `SituationData` (`HighOrbitMaxAltitude`, `LowOrbitMaxAltutude`, `AtmosphereMaxAltutude`
  (sic), body and per-situation scalars) and `Regions` (`Id`, `MapId`, atmosphere / splashed / landed scalars).
- `science_region_discoverables`: landmark regions as a body-frame position (Unity Y-up: x to longitude 0,
  z to 90 E, y to the north pole) and a radius; they apply only when landed.
- `science_region_map`: `CelestialBodyBakedScienceRegionMap`, a 2048 x 2048 byte map of `MapId`s sampled with
  `GetSphericalUVsForLatLongSingle` (the same equirectangular UVs as the terrain: `u = lon/360 + 0.5`,
  `v = lat/180 + 0.5`). Our 512 x 256 downsample agrees with the terrain's land and sea on 98% of Kerbin.
- `techNodeData`: 87 nodes (`RequiredSciencePoints`, `UnlockedPartsIDs`, `RequiredTechNodeIDs`, `HiddenByNodeID`,
  `TierToUnlock`, `TechTreePosition`). Parts carry their experiments in `Data_ScienceExperiment.Experiments`
  (`ExperimentDefinitionID`, `CrewRequired`, `TimeToComplete`, `ResourcesCost`).

`tools/extract_science.py` pulls the four science labels; the importer writes `assets/own/science/index.json`,
`assets/own/science/regions.bin` and `assets/own/tech-tree.json`.

## Rules

- Situation (`GetScienceSituation`): landed / prelaunch is Landed, splashed is Splashed; otherwise the first of
  Atmosphere (`alt <= AtmosphereMaxAltitude`), LowOrbit, HighOrbit whose scalar is positive; beyond high orbit
  nothing.
- Region (`GetScienceRegionAt`): a discoverable when landed within its radius, else the map. Orbits use region
  scalar 1; elsewhere a missing region means -1 (no science).
- A report's value (`CreateScienceReports`): `DataValue` (or `SampleValue`) x body scalar x situation scalar
  x region scalar when the matching valid location requires the region.
- Report key: `{ExperimentID}_{DataType|SampleType}_{ResearchLocationId}`, location id `{Body}_{Situation}`
  plus `_{Region}` when the region is required. A key already submitted is worth 0 (`GetPotentialReportValueBase`).
  The original always builds the location with the region the vessel is over (`RefreshLocationsValidity` uses
  `requiresRegion: true`), so orbital and atmospheric reports count once per region; we do the same.
- Science points: capacity is the sum of submitted reports rounded, times the `ScienceRewards` difficulty
  option, plus turned-in missions' science rewards (`MissionRewards`); allocated is the sum of unlocked nodes'
  `RequiredSciencePoints`; a node can be unlocked when every `RequiredTechNodeIDs` entry is unlocked and the
  free points cover it. Nodes with no cost and no requirements are always unlocked.

Run times: an experiment with `TimeToComplete` runs that long where it started, drawing its `ResourcesCost`
per second (waiting without it) and is interrupted if the vessel's situation or region changes.

Not modelled yet: deploy animations, transmission time and antennas (CommNet), EVA
experiments, `TierToUnlock` gating, the original's flavour texts.
