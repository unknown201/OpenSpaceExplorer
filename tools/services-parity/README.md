# Live electrical, radio, transmission and flag audit

This is an **active scripted benchmark**, using the locally installed KSP2 game,
its real part modules, resource broker, CommNet graph and EVA/flag callbacks.
It uses a dedicated `OSE Services Validation` campaign. It changes charge and
antenna ranges and teleports the test craft/Kerbal; it is not a passive player
recording. Synthetic transmitter payloads cannot award science. Flag pickup calls
the game's interaction callback and does not establish mouse selection or reach.

The OSE probe calls production simulation APIs; the WebSocket integration capture
starts its own current-build server on an ephemeral loopback port. No existing
server or player campaign is used. OSE science uses actual experiment reports,
including data and samples, in separate full/empty-charge campaigns.

## Reference preparation

Requires the **existing** local BepInEx loader and an empty KSC sandbox seed.
Close KSP2 first. Preserve existing plugins and back up/temporarily rename other
autorun markers, especially `BepInEx/ose-parity/run.txt`. Record SHA-256 hashes of
the executable, managed assembly, added plugin, source and generated craft.
`prepare.py` refuses an existing seed unless it is the unchanged empty validation
seed. It does not overwrite other campaigns. Local game-derived files stay ignored.

```powershell
$game = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2'
$saves = 'C:/Users/Josh/AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Saves/SinglePlayer'
.venv/Scripts/python.exe tools/services-parity/prepare.py --game $game --saves $saves `
  --seed "$saves/OSE Joint Validation/seed.json"
dotnet build tools/services-parity/ServicesParity.csproj -c Release `
  "-p:GameManagedDir=$game/KSP2_x64_Data/Managed" "-p:BepInExCoreDir=$game/BepInEx/core" `
  -o .cache/services-parity/build
```

Copy only `OSE.ServicesParity.dll` into `BepInEx/plugins/`, then launch the existing
game. The plugin loads the dedicated seed, launches the five-part craft, records
the scenarios, then quits. Require `out/done.txt` and absence of `out/failed.txt`.
Allow several minutes for game startup. Scenario holds use **universe time**;
real time and UT are both recorded. The run uses a 960×540 game window.

Archive `BepInEx/ose-services-validation/out/` as `.cache/services-parity/native/`.
Run the supplements separately, archiving `out/` between runs: create
`BepInEx/ose-services-validation/flags-only.txt` to use a terrestrial launch and
normal EVA jump release, then remove that marker and create `solar-only.txt` for
four phases of a 100 km circular orbit. Archive their outputs as `native-flags/`
and `native-solar/`. Keep each run's actual plugin/source hashes separately.
The default run's teleported flag attempt and longitude-only shadow setup are
diagnostics; they cannot replace the accepted supplements. The analyzer uses the
separate launchpad flag events and requires actual orbital blocker/zero-output
samples when the solar supplement is present.
The game inherits JSON serialization defaults, so `*.jsonl` can contain consecutive
indented JSON objects. `analyze.py` decodes both forms and rejects truncated data.
Preserve unsuccessful attempts separately; never combine their records with a
successful run. After the game exits, archive the dedicated campaign/logs, remove
only this audit's plugin/marker, and restore the prior autorun marker. Verify cleanup
targets resolve inside the intended game or named validation campaign directory.

## OSE capture and comparison

```powershell
cargo run --offline -p ose-sim --example services_probe -- .cache/services-parity
cargo test --offline -p ose-server --test services_runtime -- --nocapture
.venv/Scripts/python.exe -B -m unittest discover -s tools/services-parity -p test_analyze.py
.venv/Scripts/python.exe tools/services-parity/analyze.py .cache/services-parity
.venv/Scripts/python.exe tools/services-parity/plot.py .cache/services-parity
```

The integration test passes when the capture sequence succeeds. It deliberately
does not assert current OSE behavior equals KSP2. The comparison exits **1 for
measured gaps**, **2 for incomplete/unmatched evidence**, and **0 only for full
alignment**. Current solar attitudes, physical placement and native pointer/restore
paths are not matched, so this investigation cannot yield a full parity pass.

Measurements include EC changes at 50 simulation ticks/s, OSE panel orientation
variants, body shading, three range cut/return cycles, native transmitter progress
and resource/link interruption callbacks, actual OSE report submission at full/zero
EC, flag placement/repeated placement, native pickup, OSE vessel recovery,
quicksave/load and transport reconnection. Transport reconnect and in-game CommNet
reconnection are separate cases. Source/configuration facts must remain labeled
separately from these runtime results.
