<#
.SYNOPSIS
  KSP2 reference runs for orbital reentry (T11): three craft come down from an 80 x 30 km Kerbin orbit, then
  everything is put back.

.DESCRIPTION
  Run by hand from the repository root, with KSP2 closed and a visible desktop (screenshots need a display):

      powershell -ExecutionPolicy Bypass -File tools\ksp2-parity-pilot\run-reentry-orbit.ps1

  Same pattern as run-reentry-com.ps1: build the pilot, back up the plugin, run.txt, the craft files it installs,
  BepInEx\ose-parity\out and the "OSE Parity" campaign; run each scenario; always restore.
    reentry: Mk1 pod over a 1.25 m heat shield    -> reference\ksp2-reentryorbit-reentry
    pod:     the bare Mk1 pod                      -> reference\ksp2-reentryorbit-pod
    stack:   pod, six 1x4 tanks, Swivel (engine first) -> reference\ksp2-reentryorbit-stack
  Each one starts 74 km up on the way down to a 30 km periapsis with SAS holding retrograde, and logs speed against
  altitude, every part's temperature, and every part lost (ReentryOrbit.cs). About 8 minutes each.
  "pod free" is the bare pod again with SAS switched off once it is in the air (its own aerodynamic stability).
  Pass -Scenarios pod (etc.) to run only some.
#>
param(
    [string]$Ksp2 = "C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2",
    [string[]]$Scenarios = @("reentry", "pod", "stack"),
    [int]$TimeoutMinutes = 20
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$pilotDir = Join-Path $Ksp2 "BepInEx\ose-parity"
$plugin = Join-Path $Ksp2 "BepInEx\plugins\OSE.ParityPilot.dll"
$campaign = Join-Path $env:USERPROFILE "AppData\LocalLow\Intercept Games\Kerbal Space Program 2\Saves\SinglePlayer\OSE Parity"
$outNames = @{ reentry = "ksp2-reentryorbit-reentry"; pod = "ksp2-reentryorbit-pod"; stack = "ksp2-reentryorbit-stack"; "pod free" = "ksp2-reentryorbit-podfree"; settings = "ksp2-thermal-settings" }
$crafts = @("craft-reentry.json", "craft-drop-pod.json", "craft-stack.json")

if (-not (Test-Path $pilotDir)) { throw "No ${pilotDir}: the parity pilot (BepInEx) is not installed in KSP2." }
if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { throw "KSP2 is running; close it first." }
foreach ($s in $Scenarios) { if (-not $outNames.ContainsKey($s)) { throw "Unknown scenario '$s' (reentry, pod, stack, 'pod free')." } }

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
        Set-Content -Path (Join-Path $pilotDir "run.txt") -Value $(if ($s -eq "settings") { $s } else { "reentryorbit $s" }) -Encoding ascii -NoNewline
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
        $dest = Join-Path $repo ("reference\" + $outNames[$s])
        if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
        if (Test-Path $out) { Copy-Item -Recurse -Force $out $dest }
        $ok = Test-Path (Join-Path $dest "done.txt")
        $log = Join-Path $dest "steps.log"
        if ((Test-Path $log) -and -not (Select-String -Quiet -SimpleMatch -Path $log -Pattern "scenario $(if ($s -eq 'settings') { $s } else { 'reentryorbit ' + $s })")) { Write-Warning "steps.log does not name scenario reentryorbit $s" }
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
