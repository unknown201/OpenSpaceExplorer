# KSP2 wing probe

A BepInEx plugin that records what KSP2 computes for procedural wings (mass, lift areas, flap pivot, lift at
5 and 10 degrees angle of attack), as the reference for `ose_data::procedural`. It loads `craft.json` (the stock
jet, `assets/ksp2/data/stockassemblies/Stock04_Jet.json`) in the VAB of the "OSE Parity" campaign, reshapes
parts per `shapes.txt` through the editor's own slider API, launches and dumps again in flight.

KSP2 is shared with other threads: ask before launching it. The parity pilot (`tools/ksp2-parity-pilot`)
must stay idle while this runs, so move `BepInEx/ose-parity/run.txt` aside and put it back afterwards.

```powershell
$K = 'C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2'
dotnet build tools/ksp2-wing-probe/WingProbe.csproj -c Release "-p:GameManagedDir=$K\KSP2_x64_Data\Managed" "-p:BepInExCoreDir=$K\BepInEx\core" -o .cache/wingprobe
New-Item -ItemType Directory -Force "$K\BepInEx\ose-wings" | Out-Null
Copy-Item assets/ksp2/data/stockassemblies/Stock04_Jet.json "$K\BepInEx\ose-wings\craft.json"
Copy-Item tools/ksp2-wing-probe/shapes.txt "$K\BepInEx\ose-wings\"
Copy-Item .cache/wingprobe/OSE.WingProbe.dll "$K\BepInEx\plugins\"
Set-Content -Path "$K\BepInEx\ose-wings\run.txt" -Value 'wings' -Encoding ascii
Start-Process steam://rungameid/954850   # quits by itself; results in BepInEx\ose-wings\out\wings.csv
```

Afterwards remove `BepInEx\ose-wings\run.txt` (the plugin idles without it) and compare with
`cargo run -p ose-data --example wing_parity -- <wings.csv>`.
