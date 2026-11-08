# One KSP2 run of the wing probe, leaving the install as it was found.
#   powershell -ExecutionPolicy Bypass -File D:\Projects\ose-procwing\tools\ksp2-wing-probe\run-probe.ps1
# Builds OSE.WingProbe.dll, pauses the parity pilot (BepInEx\ose-parity\run.txt), installs the probe, starts
# KSP2 and waits for it to quit (about 10 minutes; it quits by itself). Then it copies the results to
# reference\ksp2-wings\ and restores everything: the pilot's run.txt, no probe DLL, no probe run.txt.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$K = 'C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2'
$W = "$K\BepInEx\ose-wings"
$pilotRun = "$K\BepInEx\ose-parity\run.txt"
$paused = "$K\BepInEx\ose-parity\run.txt.paused-by-wings"
$dll = "$K\BepInEx\plugins\OSE.WingProbe.dll"
if (Get-Process -Name KSP2_x64 -ErrorAction SilentlyContinue) { throw 'KSP2 is running; wait for it to close.' }

dotnet build "$repo\tools\ksp2-wing-probe\WingProbe.csproj" -c Release "-p:GameManagedDir=$K\KSP2_x64_Data\Managed" "-p:BepInExCoreDir=$K\BepInEx\core" -o "$repo\.cache\wingprobe"
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$hadPilotRun = Test-Path $pilotRun
try {
    if ($hadPilotRun) { Move-Item $pilotRun $paused -Force }
    New-Item -ItemType Directory -Force $W | Out-Null
    if (Test-Path "$W\out") { Remove-Item -Recurse -Force "$W\out" }
    Copy-Item "$repo\assets\ksp2\data\stockassemblies\Stock04_Jet.json" "$W\craft.json" -Force
    Copy-Item "$repo\tools\ksp2-wing-probe\shapes.txt" "$W\shapes.txt" -Force
    Copy-Item "$repo\.cache\wingprobe\OSE.WingProbe.dll" $dll -Force
    Set-Content -Path "$W\run.txt" -Value 'wings' -Encoding ascii
    if (-not (Test-Path "$W\run.txt") -or (Test-Path $pilotRun)) { throw 'probe run.txt not written or the pilot is not paused' }
    Write-Host 'Starting KSP2 (the probe quits it when done)...'
    Start-Process steam://rungameid/954850
    $deadline = (Get-Date).AddMinutes(25)
    while (-not (Get-Process -Name KSP2_x64 -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep 5 }
    while ((Get-Process -Name KSP2_x64 -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep 10 }
    $dest = "$repo\reference\ksp2-wings"
    New-Item -ItemType Directory -Force $dest | Out-Null
    if (Test-Path "$W\out") { Copy-Item "$W\out\*" $dest -Recurse -Force }
    Write-Host "Results in $dest (done.txt present: $(Test-Path "$dest\done.txt"))"
}
finally {
    if (Test-Path "$W\run.txt") { Remove-Item "$W\run.txt" -Force }
    if (Test-Path $dll) { Remove-Item $dll -Force }
    if ($hadPilotRun -and (Test-Path $paused)) { Move-Item $paused $pilotRun -Force }
    Write-Host 'Install restored (pilot run.txt back, probe removed).'
}
