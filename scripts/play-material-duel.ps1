$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$binary = Join-Path $repoRoot 'target/release/play.exe'
if (!(Test-Path -LiteralPath $binary)) { throw 'Build first: scripts/cargo-local.ps1 build --release -p render --bin play' }
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList @('--scenario','data/material-duel.ron','--dist','0.7','--yaw','0.2','--pitch','0.95')
