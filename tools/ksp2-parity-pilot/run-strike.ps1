<#
.SYNOPSIS
  KSP2 reference runs for how each part meets the ground (GroundStrike.cs), then puts everything back.

.DESCRIPTION
  Run by hand from the repository root, with KSP2 closed and a visible desktop (screenshots need a display):

      powershell -ExecutionPolicy Bypass -File tools\ksp2-parity-pilot\run-strike.ps1

  What it does:
    1. Builds the parity pilot (tools\ksp2-parity-pilot) into a temp folder.
    2. Backs up what it is about to change in the KSP2 folder: BepInEx\plugins\OSE.ParityPilot.dll,
       BepInEx\ose-parity\run.txt, craft-jet.json, craft-drop-stack.json, BepInEx\ose-parity\out, and the
       "OSE Parity" campaign folder under %USERPROFILE%\AppData\LocalLow\Intercept Games\Kerbal Space Program 2\
       Saves\SinglePlayer. Nothing else is touched; your own campaigns are never opened.
    3. Installs the new pilot and the two craft, then for each scenario writes run.txt, starts KSP2 through Steam,
       waits for the pilot to finish (it quits KSP2 itself) and copies BepInEx\ose-parity\out to
       reference\ksp2-strike-<mode>:
         strike land   Stock04_Jet takes off from Runway_1 and is flown back down onto it, gear down
         strike belly  the same with the gear raised: a belly landing
         strike tail   Stock04_Jet cut at 38 m/s, stick held full back to 8 m/s: it rotates onto its tail
         strike stack  pod, tanks and Swivel set down upright on Runway_1 at 4 m/s
       Each lists every part's colliders (colliders.csv, collider-points.csv) and logs every contact a part
       makes with the ground (strike-contacts.csv), the craft (strike-trace.csv) and lost parts (strike-parts.csv).
    4. Always (also on failure or Ctrl+C) restores everything from step 2, so the KSP2 folder and the parity
       campaign are as they were.

  Takes about 4 minutes a scenario. Check each run's steps.log names its scenario.
#>
param(
    [string]$Ksp2 = "C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2",
    [string[]]$Scenarios = @("strike tail", "strike belly", "strike land", "strike stack"),
    [int]$TimeoutMinutes = 20,
    # appended to each output folder, so a rerun keeps the earlier captures
    [string]$Suffix = ""
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$pilotDir = Join-Path $Ksp2 "BepInEx\ose-parity"
$plugin = Join-Path $Ksp2 "BepInEx\plugins\OSE.ParityPilot.dll"
$campaign = Join-Path $env:USERPROFILE "AppData\LocalLow\Intercept Games\Kerbal Space Program 2\Saves\SinglePlayer\OSE Parity"
$outNames = @{ "strike tail" = "ksp2-strike-tail"; "strike belly" = "ksp2-strike-belly"; "strike land" = "ksp2-strike-land"; "strike stack" = "ksp2-strike-stack" }

if (-not (Test-Path $pilotDir)) { throw "No ${pilotDir}: the parity pilot (BepInEx) is not installed in KSP2." }
if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { throw "KSP2 is running; close it first." }
foreach ($s in $Scenarios) { if (-not $outNames.ContainsKey($s)) { throw "Unknown scenario '$s' (strike tail|belly|land|stack)." } }

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
Save-Item (Join-Path $pilotDir "craft-jet.json") "craft-jet.json"
Save-Item (Join-Path $pilotDir "craft-drop-stack.json") "craft-drop-stack.json"
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
    Copy-Item -Force (Join-Path $PSScriptRoot "craft-jet.json") (Join-Path $pilotDir "craft-jet.json")
    Copy-Item -Force (Join-Path $PSScriptRoot "craft-drop-stack.json") (Join-Path $pilotDir "craft-drop-stack.json")
    foreach ($s in $Scenarios) {
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
        $dest = Join-Path $repo ("reference\" + $outNames[$s] + $Suffix)
        if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
        if (Test-Path $out) { Copy-Item -Recurse -Force $out $dest }
        $ok = Test-Path (Join-Path $dest "done.txt")
        Write-Host ("'$s': " + $(if ($ok) { "done" } else { "did not finish" }) + " -> $dest")
    }
}
finally {
    # 4. put everything back
    if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { Stop-Process -Name "KSP2_x64" -Force; Start-Sleep -Seconds 5 }
    Restore-Item $plugin "OSE.ParityPilot.dll"
    Restore-Item (Join-Path $pilotDir "run.txt") "run.txt"
    Restore-Item (Join-Path $pilotDir "craft-jet.json") "craft-jet.json"
    Restore-Item (Join-Path $pilotDir "craft-drop-stack.json") "craft-drop-stack.json"
    Restore-Item (Join-Path $pilotDir "out") "out"
    Restore-Item $campaign "OSE Parity"
    Write-Host "Restored the pilot, its run files and the OSE Parity campaign (backup kept in $backup)."
}
