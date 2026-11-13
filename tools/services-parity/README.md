# Controlled native service capture

This BepInEx probe measures the running KSP2 modules, resource broker, CommNet graph
and flag callbacks. It is an active benchmark: it changes EC and antenna ranges,
teleports the test vessel and uses synthetic transmitter payloads which cannot
award science. Run only in its dedicated disposable `OSE Services Validation`
campaign. See the [results and evidence limits](../../docs/reference-notes/live-services-alignment.md).

Requires the local game, existing BepInEx loader, imported stock assembly JSON under
`assets/ksp2/data/stockassemblies`, the local `BepInEx/ose-parity/craft-stack.json`
template and an empty Sandbox save. These game-derived inputs are ignored and are
not downloaded or installed by this tool.

Close the game first. Preserve existing plugins and temporarily rename conflicting
autorun markers, especially `BepInEx/ose-parity/run.txt`. Record their original state.
Prepare only the dedicated validation campaign, then build:

```powershell
$game = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
$saves = 'C:/Users/Josh/AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Saves/SinglePlayer'
python tools/services-parity/prepare.py --game $game --saves $saves --seed "$saves/OSE Joint Validation/seed.json"
dotnet build tools/services-parity/ServicesParity.csproj -c Release `
  "-p:GameManagedDir=$game/KSP2_x64_Data/Managed" "-p:BepInExCoreDir=$game/BepInEx/core" `
  -o .cache/services-parity/build
```

Copy only the resulting `OSE.ServicesParity.dll` to `BepInEx/plugins`, then launch
the game. Require `BepInEx/ose-services-validation/out/done.txt` and no `failed.txt`.
The plugin loads the seed, launches its craft, records and quits. Holds use universe
time; both real time and universe time are recorded. Record executable, assembly,
plugin, source and generated-craft SHA-256 hashes with each run.

Archive `out` separately between three runs: the default run to
`.cache/services-parity/native`; add `flags-only.txt` beside `run.txt` for the
normal launchpad EVA run (`native-flags`); remove that marker and add
`solar-only.txt` for the four-phase orbital run (`native-solar`). The default run's
teleported flag attempt is diagnostic, not the accepted placement observation.
The flag supplement releases the ladder through normal jump input; pickup invokes
the native interaction callback. The solar supplement requires actual Kerbin
blocker/zero-output observations. JSONL can contain consecutive indented JSON
objects because the game overrides JSON formatting; use `JSONDecoder.raw_decode`
and reject trailing/truncated objects rather than assuming one record per line.

After exit, remove only this probe's plugin/marker and restore prior autorun
markers. Keep unsuccessful attempts separate. Do not mix frames from different
plugin builds. The compact committed fixture records measured values; OSE's
production regressions and GPU script are documented in the results note.
