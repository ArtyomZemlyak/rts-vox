$CargoArgs = $args
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
# Session-local fallback for hosts with MSVC but no installed Windows SDK libs.
$vcRoot = 'C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717'
$sdkRoot = Join-Path $repoRoot 'cache/toolchain/winsdk-x64/c'
if (Test-Path $vcRoot) {
    $env:PATH = "$vcRoot/bin/Hostx64/x64;$env:PATH"
    $env:LIB = "$vcRoot/lib/x64;$sdkRoot/um/x64;$sdkRoot/ucrt/x64;$env:LIB"
}
Push-Location $repoRoot
try { & cargo @CargoArgs; exit $LASTEXITCODE } finally { Pop-Location }
