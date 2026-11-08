# T4 K1 drag capture (KSP2 run 20): installs the K1 drag pilot build, runs the "k1" scenario (Kerbal K1 straight up,
# dumping every part's forces and exposed drag areas at Mach 0.8 / 1.15 / 1.4 / 2.0 / 2.8), copies the results to
# D:\Projects\OpenSpaceExplorer\reference\ksp2-run20-k1-drag, and in a finally block puts back the shared plugin DLL
# (f622a335...), run.txt ("docking") and the previous out folder. KSP2 quits itself when the run ends (about 4 min).
#
#   powershell -ExecutionPolicy Bypass -File D:\Projects\ose-dock\tools\ksp2-parity-pilot\run-k1-drag.ps1

$ErrorActionPreference = 'Stop'
$ksp2 = 'C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2'
$par = Join-Path $ksp2 'BepInEx\ose-parity'
$dll = Join-Path $ksp2 'BepInEx\plugins\OSE.ParityPilot.dll'
$build = 'D:\Projects\ose-dock\tools\ksp2-parity-pilot\builds\k1-drag\OSE.ParityPilot.dll'
$dest = 'D:\Projects\OpenSpaceExplorer\reference\ksp2-run20-k1-drag'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

if (Get-Process KSP2_x64 -ErrorAction SilentlyContinue) { throw 'KSP2 is already running; close it first.' }
if (-not (Test-Path (Join-Path $par 'craft-k1.json'))) { throw "craft-k1.json is missing from $par" }

$backupDll = Join-Path $env:TEMP "OSE.ParityPilot.$stamp.dll"
$runTxt = Join-Path $par 'run.txt'
$oldRun = if (Test-Path $runTxt) { Get-Content $runTxt -Raw } else { 'docking' }
$out = Join-Path $par 'out'
$oldOut = Join-Path $par "out-before-k1drag-$stamp"
Copy-Item $dll $backupDll
Write-Host "backed up the plugin ($((Get-FileHash $dll).Hash.Substring(0,8))) and run.txt '$oldRun'"
try {
    if (Test-Path $out) { Rename-Item $out $oldOut }
    New-Item -ItemType Directory $out | Out-Null
    Copy-Item $build $dll -Force
    Set-Content -Path $runTxt -Value 'k1' -NoNewline -Encoding ascii
    Write-Host 'starting KSP2 (scenario k1)...'
    Start-Process 'steam://rungameid/954850'
    $t = 0
    while (-not (Get-Process KSP2_x64 -ErrorAction SilentlyContinue) -and $t -lt 180) { Start-Sleep 5; $t += 5 }
    if (-not (Get-Process KSP2_x64 -ErrorAction SilentlyContinue)) { throw 'KSP2 did not start within 3 minutes' }
    $t = 0
    while ((Get-Process KSP2_x64 -ErrorAction SilentlyContinue) -and $t -lt 1200) { Start-Sleep 5; $t += 5 }
    if (Get-Process KSP2_x64 -ErrorAction SilentlyContinue) { Write-Warning 'KSP2 still running after 20 minutes; closing it'; Stop-Process -Name KSP2_x64 -Force; Start-Sleep 5 }
    New-Item -ItemType Directory $dest -Force | Out-Null
    Copy-Item (Join-Path $out '*') $dest -Recurse -Force
    Write-Host "results copied to $dest"
    if (-not (Test-Path (Join-Path $out 'done.txt'))) { Write-Warning 'no done.txt: the run did not finish' }
}
finally {
    if (Get-Process KSP2_x64 -ErrorAction SilentlyContinue) { Stop-Process -Name KSP2_x64 -Force; Start-Sleep 5 }
    Copy-Item $backupDll $dll -Force
    Set-Content -Path $runTxt -Value $oldRun.Trim() -NoNewline -Encoding ascii
    if (Test-Path $oldOut) {
        if (Test-Path $out) { Rename-Item $out (Join-Path $par "out-k1drag-$stamp") }
        Rename-Item $oldOut $out
    }
    Write-Host "restored the plugin ($((Get-FileHash $dll).Hash.Substring(0,8))), run.txt '$((Get-Content $runTxt -Raw))' and the out folder"
}
