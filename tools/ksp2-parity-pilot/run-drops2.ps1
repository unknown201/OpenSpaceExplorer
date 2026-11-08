<#
.SYNOPSIS
  KSP2 reference drops for crash crush-through (T14 follow-up), then puts everything back.

.DESCRIPTION
  Run by hand, with KSP2 closed and a visible desktop (screenshots need a display):

      powershell -ExecutionPolicy Bypass -File <this file>

  The pilot's existing "drop <craft> <speed>" scenario, once per KSP2 launch: the craft upright about 1 m above
  the pad, falling at the set speed; the parts aboard every tick (drop-parts.csv) beside the trace.
    drop stack 10/15/20/30/45  pod, six 1x4 tanks and a Swivel (craft-drop-stack.json): how far up the stack the
                               crush goes, layer by layer, and what is left standing
    drop podtank 25/35         pod over a 1x2 tank: the tank breaks (crashTolerance 20); does the pod (15) survive
  Backs up the plugin dll, run.txt, the drop craft files, out\ and the "OSE Parity" campaign folder, installs the
  pilot built from this checkout, copies each run's out\ to reference\ksp2-drops2\<craft>-<speed>, and restores
  everything in a finally block (also on failure or Ctrl+C). Your own campaigns are never opened.
  About 2 minutes per drop (7 drops). -Scenarios picks a subset, e.g. -Scenarios "drop stack 20".
#>
param(
    [string]$Ksp2 = "C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2",
    [string[]]$Scenarios = @("drop stack 10", "drop stack 15", "drop stack 20", "drop stack 30", "drop stack 45", "drop podtank 25", "drop podtank 35"),
    [int]$TimeoutMinutes = 20
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$pilotDir = Join-Path $Ksp2 "BepInEx\ose-parity"
$plugin = Join-Path $Ksp2 "BepInEx\plugins\OSE.ParityPilot.dll"
$campaign = Join-Path $env:USERPROFILE "AppData\LocalLow\Intercept Games\Kerbal Space Program 2\Saves\SinglePlayer\OSE Parity"
function Out-Name($scenario) { $w = $scenario.Split(" "); "ksp2-drops2\" + $w[1] + "-" + $w[2] }
$crafts = @("craft-drop-stack.json", "craft-drop-podtank.json", "craft-drop-pod.json")

if (-not (Test-Path $pilotDir)) { throw "No ${pilotDir}: the parity pilot (BepInEx) is not installed in KSP2." }
if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { throw "KSP2 is running; close it first." }
foreach ($s in $Scenarios) { if ($s -notmatch '^drop (stack|podtank|pod) [0-9.]+$') { throw "Unknown scenario '$s' (drop <stack|podtank|pod> <speed>)." } }

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
foreach ($c in $crafts) { Save-Item (Join-Path $pilotDir $c) $c }
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
    foreach ($c in $crafts) { Copy-Item -Force (Join-Path $PSScriptRoot $c) (Join-Path $pilotDir $c) }
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
        $dest = Join-Path $repo ("reference\" + (Out-Name $s))
        New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
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
    foreach ($c in $crafts) { Restore-Item (Join-Path $pilotDir $c) $c }
    Restore-Item (Join-Path $pilotDir "out") "out"
    Restore-Item $campaign "OSE Parity"
    Write-Host "Restored the pilot, its run files and the OSE Parity campaign (backup kept in $backup)."
}
