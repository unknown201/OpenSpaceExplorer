param(
    [string]$GameDir = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2',
    [string]$SaveDir = 'C:/Users/Josh/AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Saves/SinglePlayer',
    [string]$Seed = '',
    [string]$RunDir = '.cache/water-parity',
    [string]$IsolatedDir = '.cache/water-native'
)
$ErrorActionPreference = 'Stop'
if (!$Seed) { $Seed = Join-Path $SaveDir 'OSE Joint Validation/seed.json' }
$GameDir = (Resolve-Path -LiteralPath $GameDir).Path
$RunDir = [IO.Path]::GetFullPath($RunDir)
$IsolatedDir = [IO.Path]::GetFullPath($IsolatedDir)
if (Test-Path -LiteralPath "$IsolatedDir/BepInEx/ose-water-validation/out") {
    throw 'Existing capture found. Use a new IsolatedDir and RunDir; never mix attempts.'
}
New-Item -ItemType Directory -Force -Path "$IsolatedDir/BepInEx/plugins", "$IsolatedDir/BepInEx/config" | Out-Null
foreach ($name in @('KSP2_x64.exe', 'UnityPlayer.dll', 'winhttp.dll', 'doorstop_config.ini')) {
    Copy-Item -LiteralPath "$GameDir/$name" -Destination "$IsolatedDir/$name"
}
Set-Content -LiteralPath "$IsolatedDir/steam_appid.txt" -Value '954850' -Encoding ascii
foreach ($name in @('KSP2_x64_Data', 'MonoBleedingEdge')) {
    if (!(Test-Path -LiteralPath "$IsolatedDir/$name")) {
        New-Item -ItemType Junction -Path "$IsolatedDir/$name" -Target "$GameDir/$name" | Out-Null
    } elseif ((Get-Item -LiteralPath "$IsolatedDir/$name").Target -ne "$GameDir\$name") {
        throw "Unexpected existing junction: $IsolatedDir/$name"
    }
}
foreach ($name in @('core', 'patchers')) {
    if (Test-Path -LiteralPath "$IsolatedDir/BepInEx/$name") { throw "Existing loader directory: $name" }
    Copy-Item -LiteralPath "$GameDir/BepInEx/$name" -Destination "$IsolatedDir/BepInEx/$name" -Recurse
}
Copy-Item -LiteralPath "$GameDir/BepInEx/config/BepInEx.cfg" -Destination "$IsolatedDir/BepInEx/config/BepInEx.cfg"
.venv/Scripts/python.exe tools/water-parity/prepare.py --game $GameDir --saves $SaveDir --seed $Seed --out "$IsolatedDir/BepInEx/ose-water-validation"
if ($LASTEXITCODE -ne 0) { throw 'Fixture preparation failed' }
dotnet build tools/water-parity/WaterParity.csproj -c Release "-p:GameManagedDir=$GameDir/KSP2_x64_Data/Managed" "-p:BepInExCoreDir=$GameDir/BepInEx/core" -o "$RunDir/build"
if ($LASTEXITCODE -ne 0) { throw 'Recorder build failed' }
Copy-Item -LiteralPath "$RunDir/build/OSE.WaterParity.dll" -Destination "$IsolatedDir/BepInEx/plugins/OSE.WaterParity.dll"
Copy-Item -LiteralPath "$IsolatedDir/BepInEx/ose-water-validation/craft.json" -Destination "$RunDir/craft.json"
Start-Process -FilePath "$IsolatedDir/KSP2_x64.exe" -WorkingDirectory $IsolatedDir -ArgumentList '-screen-width 960 -screen-height 540 -screen-fullscreen 0'
Write-Output "Launched isolated water benchmark. Collect $IsolatedDir/BepInEx/ose-water-validation/out only after done.txt appears."
