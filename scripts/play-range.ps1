param(
    [string]$LayoutPath = '',
    [switch]$AutoFire,
    [switch]$Build,
    [switch]$Settings,
    [ValidateRange(1, 12)][int]$Zone = 1,
    [ValidateSet('energy', 'shell', 'anni')][string]$Weapon = 'energy'
)
# ASCII only: Windows PowerShell 5.1 reads a .ps1 without a BOM as ANSI.
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$binary = Join-Path $repoRoot 'target/release/blast.exe'
if ($Build -or !(Test-Path -LiteralPath $binary)) {
    Write-Output 'Building the range window (release, local MSVC); a fresh tree takes minutes, an up-to-date one seconds.'
    & powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'cargo-local.ps1') build --release -p render --bin blast
    if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $binary)) {
        Write-Output 'Build failed; the log is above. Press Enter to close.'
        [void][Console]::ReadLine()
        exit 1
    }
}
if (!$LayoutPath) {
    $LayoutPath = 'data/range-km.ron'
}
$rangeArgs = @('--weapon', $Weapon, '--range', ('"{0}"' -f $LayoutPath))
if (!$AutoFire) { $rangeArgs += '--drive' }
if ($Settings) { $rangeArgs += '--settings' }
if ($Zone -gt 1) { $rangeArgs += @('--zone', $Zone.ToString()) }
$env:WGPU_BACKEND = 'vulkan'
Write-Output 'POLIGON: RMB ground = move, RMB unit = attack. The left panel contains weapons, power, pause, editor, settings and camera controls.'
Write-Output 'Optional: wheel zoom, middle mouse pan. Layouts: -LayoutPath data/range-slab.ron (a slab on one pillar), out/range-layout.ron (saved from the editor).'
# User-requested interactive game window.
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList $rangeArgs
