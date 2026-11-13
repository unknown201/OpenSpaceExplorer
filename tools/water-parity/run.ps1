param(
    [string]$GameDir = 'C:/Program Files (x86)/Steam/steamapps/common/Kerbal Space Program 2',
    [string]$SaveDir = 'C:/Users/Josh/AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Saves/SinglePlayer',
    [string]$Seed = '',
    [string]$RunDir = '.cache/water-parity'
)
$ErrorActionPreference = 'Stop'
if (!$Seed) { $Seed = Join-Path $SaveDir 'OSE Joint Validation/seed.json' }
$GameDir = (Resolve-Path -LiteralPath $GameDir).Path
$RunDir = [IO.Path]::GetFullPath($RunDir)
$PluginDir = Join-Path $GameDir 'BepInEx/plugins'
$InputDir = Join-Path $GameDir 'BepInEx/ose-water-validation'
$CaptureDir = Join-Path $InputDir 'out'
if (Get-Process KSP2_x64 -ErrorAction SilentlyContinue) { throw 'Close the active reference game first.' }
if ((Test-Path -LiteralPath $CaptureDir) -or (Test-Path -LiteralPath "$RunDir/native")) {
    throw 'Existing capture found. Archive it separately before starting a fresh attempt.'
}
New-Item -ItemType Directory -Force -Path "$RunDir/disabled-plugins" | Out-Null
dotnet build tools/water-parity/WaterParity.csproj -c Release "-p:GameManagedDir=$GameDir/KSP2_x64_Data/Managed" "-p:BepInExCoreDir=$GameDir/BepInEx/core" -o "$RunDir/build"
if ($LASTEXITCODE -ne 0) { throw 'Recorder build failed' }
python tools/water-parity/prepare.py --game $GameDir --saves $SaveDir --seed $Seed --out $InputDir
if ($LASTEXITCODE -ne 0) { throw 'Fixture preparation failed' }
Copy-Item -LiteralPath "$InputDir/craft.json" -Destination "$RunDir/craft.json"
$OwnedPlugin = Join-Path $PluginDir 'OSE.WaterParity.dll'
$BuiltHash = (Get-FileHash -LiteralPath "$RunDir/build/OSE.WaterParity.dll").Hash
$Disabled = @()
try {
    # Keep the installed executable's normal Windows identity; isolate only the test plugins.
    foreach ($plugin in Get-ChildItem -LiteralPath $PluginDir -Filter '*.dll' -File) {
        $backup = Join-Path "$RunDir/disabled-plugins" $plugin.Name
        if (Test-Path -LiteralPath $backup) { throw "Plugin backup already exists: $backup" }
        Move-Item -LiteralPath $plugin.FullName -Destination $backup
        $Disabled += @{ Original = $plugin.FullName; Backup = $backup }
    }
    $Disabled | ConvertTo-Json | Set-Content -LiteralPath "$RunDir/plugin-restoration.json" -Encoding utf8
    Copy-Item -LiteralPath "$RunDir/build/OSE.WaterParity.dll" -Destination $OwnedPlugin
    $gameProcess = Start-Process -FilePath "$GameDir/KSP2_x64.exe" -WorkingDirectory $GameDir -ArgumentList '-screen-width 960 -screen-height 540 -screen-fullscreen 0' -PassThru
    Write-Output "Running water benchmark as process $($gameProcess.Id). Activate the game window if startup pauses."
    Wait-Process -Id $gameProcess.Id
    if (Test-Path -LiteralPath $CaptureDir) {
        Copy-Item -LiteralPath $CaptureDir -Destination "$RunDir/native" -Recurse
    }
    if (!(Test-Path -LiteralPath "$RunDir/native/done.txt") -or (Test-Path -LiteralPath "$RunDir/native/failed.txt")) {
        throw 'Native attempt failed or is incomplete; preserved separately, never accepted as parity.'
    }
} finally {
    if ((Test-Path -LiteralPath $OwnedPlugin) -and (Get-FileHash -LiteralPath $OwnedPlugin).Hash -eq $BuiltHash) {
        Remove-Item -LiteralPath $OwnedPlugin
    }
    foreach ($entry in $Disabled) {
        if (Test-Path -LiteralPath $entry.Original) { throw "Cannot overwrite concurrently changed plugin: $($entry.Original)" }
        Move-Item -LiteralPath $entry.Backup -Destination $entry.Original
    }
    if (Test-Path -LiteralPath "$InputDir/run.txt") { Remove-Item -LiteralPath "$InputDir/run.txt" }
}
