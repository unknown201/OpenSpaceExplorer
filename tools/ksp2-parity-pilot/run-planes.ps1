<#
.SYNOPSIS
  KSP2 reference runs for planes built in OSE's hangar: each is loaded into KSP2's hangar, launched on Runway_1 and
  flown the way the stock jet's "jetfly" run is, then everything is put back.

.DESCRIPTION
  Run by hand from the repository root, with KSP2 closed and a visible desktop (screenshots need a display):

      powershell -ExecutionPolicy Bypass -File tools\ksp2-parity-pilot\run-planes.ps1

  The crafts are craft-plane-<name>.json, written by make_craft_planes.py from the planes ose-client's editor
  builds (crates/ose-sim/tests/data/editor-planes.json):
    small:    cockpit, two tanks, the default light wing pair, a Wheesley  -> reference\ksp2-plane-small
    swept:    the same with its wings swept well back                      -> reference\ksp2-plane-swept
    straight: the same with long unswept wings                             -> reference\ksp2-plane-straight
  Each sits 12 s on the runway, then (JetScenario.Fly): brakes off, full throttle to 60 m/s, full pitch up until the
  nose is 10 degrees up, SAS holds 20 s, pitch down, bank. Logged every tick: jetfly-trace.csv (speed, altitude,
  pitch, roll, inputs, thrust), jetfly-forces.csv (every part's airflow forces) and jetfly-surfaces.csv (each
  control surface's deflection). About 4 minutes each. Pass -Scenarios small (etc.) to run only some.
#>
param(
    [string]$Ksp2 = "C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2",
    [string[]]$Scenarios = @("small", "swept", "straight"),
    [int]$TimeoutMinutes = 20
)
$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$pilotDir = Join-Path $Ksp2 "BepInEx\ose-parity"
$plugin = Join-Path $Ksp2 "BepInEx\plugins\OSE.ParityPilot.dll"
$campaign = Join-Path $env:USERPROFILE "AppData\LocalLow\Intercept Games\Kerbal Space Program 2\Saves\SinglePlayer\OSE Parity"
$outNames = @{ small = "ksp2-plane-small"; swept = "ksp2-plane-swept"; straight = "ksp2-plane-straight" }
$crafts = @("craft-plane-small.json", "craft-plane-swept.json", "craft-plane-straight.json")

if (-not (Test-Path $pilotDir)) { throw "No ${pilotDir}: the parity pilot (BepInEx) is not installed in KSP2." }
if (Get-Process -Name "KSP2_x64" -ErrorAction SilentlyContinue) { throw "KSP2 is running; close it first." }
foreach ($s in $Scenarios) { if (-not $outNames.ContainsKey($s)) { throw "Unknown plane '$s' (small, swept, straight)." } }
# another session may be driving KSP2 with its own run-*.ps1: two launches clobber run.txt
$parent = (Get-CimInstance Win32_Process -Filter "ProcessId = $PID").ParentProcessId
$others = Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe' OR Name = 'pwsh.exe'" | Where-Object { $_.ProcessId -ne $PID -and $_.ProcessId -ne $parent -and $_.CommandLine -match 'ksp2-parity-pilot\\run-' }
if ($others) { throw "Another parity run is going (PID $($others.ProcessId -join ', ')); wait for it." }

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
        Set-Content -Path (Join-Path $pilotDir "run.txt") -Value "jetfly $s" -Encoding ascii -NoNewline
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
        if ((Test-Path $log) -and -not (Select-String -Quiet -SimpleMatch -Path $log -Pattern "scenario jetfly $s")) { Write-Warning "steps.log does not name scenario jetfly $s" }
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
