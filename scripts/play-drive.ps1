# ASCII only: Windows PowerShell 5.1 reads a .ps1 without a BOM as ANSI.
# Writes the range fixture save and opens it in the range window (ADR-671).
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path $repoRoot 'target/release/drive_fixture.exe'
$binary = Join-Path $repoRoot 'target/release/blast.exe'
if (!(Test-Path -LiteralPath $fixture) -or !(Test-Path -LiteralPath $binary)) {
    throw 'Build: scripts/cargo-local.ps1 build --release -p sim-cli --bin drive_fixture -p render --bin blast'
}
$savePath = Join-Path $repoRoot 'out/damaged-drive/fixture.bin'
& $fixture $savePath
if ($LASTEXITCODE -ne 0) { throw 'Cannot create the damaged drive fixture' }
$env:WGPU_BACKEND = 'vulkan'
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList @('--load', ('"{0}"' -f $savePath))
