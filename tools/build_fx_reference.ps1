# Build the independent D3D11 renderer used to check the WGSL shader port.
$ErrorActionPreference = 'Stop'
$particleRoot = Split-Path -Parent $PSScriptRoot
$particleVsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$particleVs = & $particleVsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$particleVs) { throw 'Install the Visual Studio C++ build tools first.' }
$particleVersion = (Get-Content (Join-Path $particleVs 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt')).Trim()
$particleCompiler = Join-Path $particleVs "VC/Tools/MSVC/$particleVersion"
$particleSdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10'
$particleSdkVersion = (Get-ChildItem (Join-Path $particleSdk 'Include') -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'um/d3d11.h') } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1).Name
if (!$particleSdkVersion) { throw 'Install the Windows SDK first.' }
$particleOut = Join-Path $particleRoot '.cache/fx-parity'
New-Item -ItemType Directory -Force -Path $particleOut | Out-Null
& "$particleCompiler/bin/Hostx64/x64/cl.exe" /nologo /EHsc /std:c++17 `
    "/I$particleCompiler/include" "/I$particleSdk/Include/$particleSdkVersion/um" `
    "/I$particleSdk/Include/$particleSdkVersion/shared" "/I$particleSdk/Include/$particleSdkVersion/ucrt" `
    "/I$particleSdk/Include/$particleSdkVersion/winrt" `
    (Join-Path $particleRoot 'tools/fx_reference.cpp') "/Fo:$particleOut/fx_reference.obj" "/Fe:$particleOut/fx_reference.exe" `
    /link "/LIBPATH:$particleCompiler/lib/x64" "/LIBPATH:$particleSdk/Lib/$particleSdkVersion/um/x64" `
    "/LIBPATH:$particleSdk/Lib/$particleSdkVersion/ucrt/x64" d3d11.lib
if ($LASTEXITCODE) { throw "Reference renderer build failed: $LASTEXITCODE" }
