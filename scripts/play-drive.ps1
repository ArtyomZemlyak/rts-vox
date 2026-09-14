$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path $repoRoot 'target/release/drive_fixture.exe'
$binary = Join-Path $repoRoot 'target/release/play.exe'
if (!(Test-Path -LiteralPath $fixture) -or !(Test-Path -LiteralPath $binary)) {
    throw 'Build: scripts/cargo-local.ps1 build --release -p sim-cli --bin drive_fixture -p render --bin play'
}
$savePath = Join-Path $repoRoot 'out/damaged-drive/fixture.bin'
& $fixture $savePath
if ($LASTEXITCODE -ne 0) { throw 'Cannot create the damaged drive fixture' }
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList @('--load', ('"{0}"' -f $savePath), '--dist', '0.65', '--pitch', '0.90', '--yaw', '-0.7')
