param([string]$LayoutPath = '', [switch]$AutoFire)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$binary = Join-Path $repoRoot 'target/release/blast.exe'
if (!(Test-Path -LiteralPath $binary)) { throw 'Build first: scripts/cargo-local.ps1 build --release -p render --bin blast' }
if (!$LayoutPath) {
    $LayoutPath = 'data/range-cover.ron'
}
$rangeArgs = @('--weapon','energy','--range',('"{0}"' -f $LayoutPath))
if (!$AutoFire) { $rangeArgs += '--drive' }
$env:WGPU_BACKEND = 'vulkan'
Write-Output 'POLIGON: RMB ground = move, RMB unit = attack. The left panel contains weapons, power, pause, editor and camera controls.'
Write-Output 'Optional: wheel zoom, middle mouse pan. Saved layouts: -LayoutPath out/range-layout.ron.'
# User-requested interactive game window.
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList $rangeArgs
