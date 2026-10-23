# KSP2 install survey (reference implementation)

Surveyed 2026-10-23, read-only. Nothing in the install was modified.

## Location

| Item | Value |
|---|---|
| Path | `C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2` |
| Steam app id | 954850, depot 954851 |
| Build id | 14669105 (last official build; the game was cancelled after v0.2.2.0) |
| Size on disk | 32 GB |
| Engine | Unity 2022.3.5f1, Mono backend (not IL2CPP), Windows x64 |
| Publisher string | Intercept Games |

The Mono backend matters: `Assembly-CSharp.dll` is plain .NET IL with full metadata, so it decompiles cleanly with `ilspycmd` (installed locally, v11.1). No native code reversing is needed for game logic.

## Top level

```
KSP2_x64.exe                 0.6 MB Unity player stub
UnityPlayer.dll             30 MB  engine
UnityCrashHandler64.exe
MonoBleedingEdge/            Mono runtime
KSP2_x64_Data/
  Managed/                   140 .NET assemblies
  Plugins/x86_64/            native plugins (Wwise, Burst, Steam, input)
  Resources/                 Unity built-in resources
  StreamingAssets/aa/        Addressables content (30 GB, 2948 bundles)
  StreamingAssets/Audio/     Wwise sound banks (1.4 GB)
  globalgamemanagers*, resources.assets, sharedassets0.assets, level0
```

## Managed assemblies (the code)

| Assembly | Size | Role |
|---|---|---|
| `Assembly-CSharp.dll` | 11.5 MB | All game code. Root namespace `KSP.*` |
| `Assembly-CSharp-firstpass.dll` | 0.4 MB | Early-compiled helpers |
| `KSPLogging.dll`, `IGTools.dll`, `Intercept.Metrics.dll`, `DataLink.Shared.Net.dll`, `PriorityQueue.dll` | small | In-house support libs |
| `Unity.Addressables`, `Unity.ResourceManager` | | Content loading |
| `Unity.Burst`, `Unity.Mathematics`, `Unity.Collections` | | Jobified/compiled math (terrain, orbits) |
| `AK.Wwise.*`, `AkSoundEngine.dll` | | Audio middleware (Wwise) |
| `Newtonsoft.Json` + UnityConverters | | All data files are JSON |
| `com.rlabrecque.steamworks.net` | | Steam API (achievements, overlay; no Steam networking in use) |
| `HoudiniEngineUnity`, `ShapesRuntime`, `DOTween`, `Febucci.TextAnimator`, `Unity.Postprocessing`, `Unity.TextMeshPro` | | Tooling / UI / VFX |
| `Backtrace.Unity`, `BacktraceCrashpad` | | Crash telemetry |
| `nunit.framework`, `Moq`, `Castle.Core`, `UnityEngine.TestRunner` | | Shipped test deps (dead weight) |

`KSP.*` namespace census from string scan of `Assembly-CSharp.dll` (613 distinct type/namespace strings):

| Namespace | Count | What it is |
|---|---|---|
| `KSP.Sim` | 179 | The simulation model. `Definitions.*` (PartDefinition, CelestialBodyDefinition, VesselDefinition, ResourceContainerDefinition...), `State.*` (serialised runtime state), `ResourceSystem.*` (flow requests, priority groups), `DeltaV.*`, `Maneuver`, `ConnectionGraph`, `PhysicsMode` |
| `KSP.UI` | 56 | HUD, app bar, panels |
| `KSP.Game` | 39 | Game manager, session, save/load, missions |
| `KSP.Navigation` | 29 | Trajectory / patched conics / markers |
| `KSP.OAB` | 28 | Orbital Assembly Building (vehicle editor) |
| `KSP.Rendering` | 26 | Scaled space, terrain (PQS-like), camera rigs |
| `KSP.Networking` | 24 | `MP.*` (GameClient, Message, Serialize, IGNet) plus `OnlineServices.*` (auth, telemetry, cross-save). Multiplayer scaffolding was shipped but never enabled |
| `KSP.Audio`, `KSP.VFX`, `KSP.FX`, `KSP.Animation` | ~50 | Presentation |
| `KSP.Modules` | 17 | Part modules (engines, tanks, decouplers, wheels...) |
| `KSP.Api`, `KSP.Modding`, `KSP.ScriptInterop` | 36 | Mod/Lua API surface (`LuaPipePlugin.dll`) |
| `KSP.Map`, `KSP.Iteration`, `KSP.Research`, `KSP.Messages` | | Map view, time warp, tech tree, message bus |

Of interest for a rebuild: `KSP.Sim` and `KSP.Navigation` are the parts worth re-deriving. `KSP.Networking.MP` shows the message/serialise shapes the original team intended for multiplayer.

## Content: Addressables

`StreamingAssets/aa/catalog.json` (5 MB) is a standard Unity Addressables catalog: 3970 internal ids, 8213 keys (7521 string keys), providers `AssetBundleProvider`, `BundledAssetProvider`, `LegacyResourcesProvider`. The bundles are Unity AssetBundles (LZ4/LZMA) in `StreamingAssets/aa/StandaloneWindows64/`: 2948 files, 2666 hash-named and 282 human-named.

Named bundle families:

- `celestialbody-simulation-<body>` (local-space terrain, up to 1.5 GB each), `celestialbody-scaled-<body>` (map-view spheres), `celestialbody-shared-<body>` (textures/materials), one set for each of Kerbol, Moho, Eve, Gilly, Kerbin, Mun, Minmus, Duna, Ike, Dres, Jool, Laythe, Vall, Tylo, Bop, Pol, Eeloo plus asteroid variants.
- `celestialbody_json_assets_all`: celestial body definitions as JSON.
- `celestialbodies_lua_assets_all`: Lua scripts for body behaviours.
- `parts`, `parts_data`, `proceduralparts-json`, `proceduralparts-shared`: part prefabs plus one JSON per part (`engine_1v_methalox_terrier.json`, `fueltank_3v_m3_inline_methalox_1x1.json`, `pod_3v_lander_crew.json`...). Around 200 part JSON files.
- `Assets/Data/ResourceDefinitions/*.json` (Methalox, Monopropellant, SolidFuel...), `Assets/Data/DifficultyOptions/*.json`, `Assets/Scripts/Missions/Data/**/*.json` (missions, tutorials), `Assets/Data/Stock/*.json` (stock craft), `*_Kerman.json` (roster).
- `kerbin-ksc`, `oab`, `missioncontrol`, `research`, `ui`, `hudinstruments`, `tutorials`, `explorationmissions`, `gamemodes`, `kerbal_variety_*` (per-prefab kerbal cosmetics), `agencyflags`, `vfx_contextual_released`, `audio_assets_all`, `unitybuiltinshaders`.

So the game data layer is JSON (via Newtonsoft) for parts, bodies, resources, missions and saves, stored as TextAssets inside bundles. That is the easiest and most legally comfortable thing to extract: it is data, not code.

## Audio

Wwise: 38 `.bnk` sound banks and 1104 `.wem` streams under `StreamingAssets/Audio/GeneratedSoundBanks/Windows/`. `.wem` is Vorbis-in-RIFF; `ww2ogg` + `revorb` or `vgmstream` convert to Ogg. Bank event names live in the `.bnk`.

## Native plugins

Wwise (`AkSoundEngine`, `AkMotion`, `AkSynthOne`, `MasteringSuite`), `lib_burst_generated.dll` (Burst-compiled jobs; the C# source of those jobs is still in `Assembly-CSharp`), `steam_api64.dll`, `LuaPipePlugin`, DirectInput/XInput wrappers, Logitech LED, `uwa.dll`. None are needed for the rebuild.

## Extraction toolchain (all local, all open)

| Need | Tool | Status |
|---|---|---|
| Decompile C# for reference | `ilspycmd` 11.1 | installed |
| Unpack bundles (JSON, meshes, textures) | `UnityPy` (pip) or AssetRipper | not installed yet |
| Mesh/texture to web formats | glTF via UnityPy export or AssetRipper, KTX2/Basis via `toktx` | not installed yet |
| Audio | `vgmstream` or `ww2ogg` | not installed yet |
| Rust | cargo/rustc 1.99, target `wasm32-unknown-unknown` already installed | installed; `wasm-bindgen`/`trunk` missing |
| .NET | dotnet 9 | installed |

## Licensing note

This is a personal, non-distributed project. Keep everything derived from the install out of the repository: decompiled sources, extracted meshes, textures, audio and JSON go in gitignored folders (`reference/`, `assets/ksp2/`) and are regenerated by scripts from the local Steam install. The repo holds only our own code, our own assets and converters.
