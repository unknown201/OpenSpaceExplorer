<#
.SYNOPSIS
  KSP2 reference run for procedural fairing drag cubes (fairings, T38), then puts everything back.

.DESCRIPTION
  Run by hand from the repository root, with KSP2 closed and a visible desktop (screenshots need a display):

      powershell -ExecutionPolicy Bypass -File tools\ksp2-parity-pilot\run-fairings.ps1

  What it does:
    1. Builds the parity pilot (tools\ksp2-parity-pilot) into a temp folder.
    2. Backs up what it is about to change in the KSP2 folder: BepInEx\plugins\OSE.ParityPilot.dll,
       BepInEx\ose-parity\run.txt, BepInEx\ose-parity\craft-fairing.json, BepInEx\ose-parity\out, and the
       "OSE Parity" campaign folder under %USERPROFILE%\AppData\LocalLow\Intercept Games\Kerbal Space Program 2\
       Saves\SinglePlayer. Nothing else is touched; your own campaigns are never opened.
    3. Installs the new pilot and craft-fairing.json (the Mk1 pod under an AE-FF125 fairing base), then writes run.txt,
       starts KSP2 through Steam, waits for the pilot to finish (it quits KSP2 itself) and copies
       BepInEx\ose-parity\out to reference\ksp2-fairings.
         fairings: on the pad, the fairing wall is rebuilt in flight for each shape in Fairings.cs and every drag
                   cube the part holds is written to fairing-cubes.csv (areas, drag, depth, centre, size).
    4. Always (also on failure or Ctrl+C) restores everything from step 2, so the KSP2 folder and the parity
       campaign are as they were.

  One KSP2 run per wall in -Walls (KSP2 keeps a rebuilt wall's old panels, so each wall is loaded fresh);
  results in reference\ksp2-fairings\<wall>. Takes about 3 minutes a wall.
#>
param(
    [string]$Ksp2 = "C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2",
    # one KSP2 run per wall: "name capped(0/1) height:radius ..." (sections from the base, m)
    [string[]]$Walls = @(
        "cylinder-2 0 0:0.625 2:0.625",
        "cylinder-2-capped 1 0:0.625 2:0.625",
        "cone-2 0 0:0.625 2:0.15",
        "ogive-4 0 0:0.625 0.5:1.25 3:1.25 4:0.15",
        "wide-3 0 0:0.625 1:2 3:2",
        "wide-3-capped 1 0:0.625 1:2 3:2",
        "tall-6 0 0:0.625 6:0.625"
    ),
    [int]$TimeoutMinutes = 20
)
$Scenarios = @("fairings")
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$pilotDir = Join-Path $Ksp2 "BepInEx\ose-parity"
$plugin = Join-Path $Ksp2 "BepInEx\plugins\OSE.ParityPilot.dll"
$campaign = Join-Path $env:USERPROFILE "AppData\LocalLow\Intercept Games\Kerbal Space Program 2\Saves\SinglePlayer\OSE Parity"
$outNames = @{ fairings = "ksp2-fairings" }

if (-not (Test-Path $pilotDir)) { throw "No ${pilotDir}: the parity pilot (BepInEx) is not installed in KSP2." }
if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { throw "KSP2 is running; close it first." }
foreach ($s in $Scenarios) { if (-not $outNames.ContainsKey($s)) { throw "Unknown scenario '$s' (fairings)." } }

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$work = Join-Path $env:TEMP "ose-parity-$stamp"
$backup = Join-Path $work "backup"
New-Item -ItemType Directory -Force $backup | Out-Null
Write-Host "Backup and build folder: $work"

# 1. build the pilot
$build = Join-Path $work "pilot"
& dotnet build -c Release (Join-Path $PSScriptRoot "ParityPilot.csproj") `
    "-p:BepInExCoreDir=$Ksp2\BepInEx\core" "-p:GameManagedDir=$Ksp2\KSP2_x64_Data\Managed" -o $build | Out-Host
if ($LASTEXITCODE -ne 0) { throw "Pilot build failed." }

# 2. back up what this run changes
function Save-Item($path, $name) {
    if (Test-Path $path) { Copy-Item -Recurse -Force $path (Join-Path $backup $name) }
    else { New-Item -ItemType File (Join-Path $backup "$name.absent") | Out-Null }
}
Save-Item $plugin "OSE.ParityPilot.dll"
Save-Item (Join-Path $pilotDir "run.txt") "run.txt"
Save-Item (Join-Path $pilotDir "craft-fairing.json") "craft-fairing.json"
Save-Item (Join-Path $pilotDir "out") "out"
Save-Item $campaign "OSE Parity"

function Restore-Item($path, $name) {
    $saved = Join-Path $backup $name
    if (Test-Path "$saved.absent") { if (Test-Path $path) { Remove-Item -Recurse -Force $path } }
    elseif (Test-Path $saved) {
        if (Test-Path $path) { Remove-Item -Recurse -Force $path }
        Copy-Item -Recurse -Force $saved $path
    }
}

try {
    # 3. install and run
    Copy-Item -Force (Join-Path $build "OSE.ParityPilot.dll") $plugin
    $s = "fairings"
    $partDef = Join-Path $repo "assets\ksp2\data\parts\fairing_1v_procedural.json"
    if (-not (Test-Path $partDef)) { $partDef = Join-Path $repo "..\OpenSpaceExplorer\assets\ksp2\data\parts\fairing_1v_procedural.json" }
    foreach ($wall in $Walls) {
        $name = $wall.Split(" ")[0]
        & python -I (Join-Path $PSScriptRoot "make_craft_fairing.py") $partDef (Join-Path $PSScriptRoot "craft-drop-pod.json") `
            (Join-Path $pilotDir "craft-fairing.json") @($wall.Split(" ")) | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Could not build the craft for '$wall'." }
        $out = Join-Path $pilotDir "out"
        if (Test-Path $out) { Remove-Item -Recurse -Force $out }
        Set-Content -Path (Join-Path $pilotDir "run.txt") -Value $s -Encoding ascii -NoNewline
        Write-Host "Starting KSP2 for '$s'..."
        Start-Process "steam://rungameid/954850"
        $deadline = (Get-Date).AddMinutes($TimeoutMinutes)
        $started = $false
        while ((Get-Date) -lt $deadline) {
            Start-Sleep -Seconds 5
            $running = [bool](Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue)
            if ($running) { $started = $true }
            if ($started -and -not $running) { break }
        }
        if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) {
            Write-Warning "KSP2 still running after $TimeoutMinutes minutes; closing it."
            Stop-Process -Name "KSP2_x64" -Force
            Start-Sleep -Seconds 5
        }
        $dest = Join-Path $repo ("reference\" + $outNames[$s] + "\" + $name)
        if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
        New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
        if (Test-Path $out) { Copy-Item -Recurse -Force $out $dest }
        $ok = Test-Path (Join-Path $dest "done.txt")
        Write-Host ("'$name': " + $(if ($ok) { "done" } else { "did not finish" }) + " -> $dest")
    }
}
finally {
    # 4. put everything back
    if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { Stop-Process -Name "KSP2_x64" -Force; Start-Sleep -Seconds 5 }
    Restore-Item $plugin "OSE.ParityPilot.dll"
    Restore-Item (Join-Path $pilotDir "run.txt") "run.txt"
    Restore-Item (Join-Path $pilotDir "craft-fairing.json") "craft-fairing.json"
    Restore-Item (Join-Path $pilotDir "out") "out"
    Restore-Item $campaign "OSE Parity"
    Write-Host "Restored the pilot, its run files and the OSE Parity campaign (backup kept in $backup)."
}
