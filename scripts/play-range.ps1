param(
    [string]$LayoutPath = '',
    [switch]$AutoFire,
    [switch]$Build,
    [switch]$Settings,
    [switch]$Battle,
    [switch]$Laboratory,
    [ValidateRange(1, 16)][int]$Zone = 1,
    [ValidateSet('energy', 'shell', 'kinetic', 'anni', 'laser', 'flame', 'machine-gun')][string]$Weapon = 'energy',
    [ValidateSet('tank', 'walker', 'spider')][string]$Chassis = 'tank'
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
# An in-use Windows executable cannot always be replaced. A separately built
# release can be used by the same launcher until the normal build is newer.
if (!$Build) {
    foreach ($candidate in @('target/release/blast-staged.exe', 'target/release/blast-section.exe', 'target/release/blast-hot.exe', 'target/release/blast-crawler.exe', 'target/release/blast-dune.exe', 'target/release/blast-thermal.exe', 'target/release/blast-melt.exe', 'target/release/blast-vapor.exe', 'target/release/blast-water.exe', 'target/release/blast-waterflow.exe', 'target/release/blast-metrics.exe', 'target/release/blast-metrics5.exe', 'target/release/blast-metrics8.exe', 'target/release/blast-metrics9.exe', 'target/release/blast-replay.exe', 'target/release/blast-replay2.exe', 'target/release/blast-replay3.exe', 'target/release/blast-replay4.exe', 'target/release/blast-support.exe', 'target/release/blast-support2.exe', 'target/release/blast-support3.exe', 'target/release/blast-melt-settle.exe', 'target/release/blast-overview.exe', 'target/release/blast-global-metrics3.exe', 'target/release/blast-cubic.exe')) {
        $stagedBinary = Join-Path $repoRoot $candidate
        if ((Test-Path -LiteralPath $stagedBinary) -and
            (Get-Item -LiteralPath $stagedBinary).LastWriteTimeUtc -gt (Get-Item -LiteralPath $binary).LastWriteTimeUtc) {
            $binary = $stagedBinary
        }
    }
}
if (!$LayoutPath) {
    $LayoutPath = 'data/range-km.ron'
}
$rangeArgs = @('--weapon', $Weapon, '--chassis', $Chassis, '--range', ('"{0}"' -f $LayoutPath))
if (!$AutoFire) { $rangeArgs += '--drive' }
if ($Settings) { $rangeArgs += '--settings' }
if ($Laboratory) { $rangeArgs += '--laboratory' }
if ($Battle) { $rangeArgs += @('--battle', '--edit') }
if ($Zone -gt 1) { $rangeArgs += @('--zone', $Zone.ToString()) }
$env:WGPU_BACKEND = 'vulkan'
Write-Output 'RTS VOX: choose the range, a battle against AI, or the laboratory in the main menu. F10 returns to the menu and pauses the session.'
Write-Output 'Optional: wheel zoom, middle mouse pan. Layouts: -LayoutPath data/range-slab.ron (a slab on one pillar), out/range-layout.ron (saved from the editor).'
# User-requested interactive game window.
Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Normal -ArgumentList $rangeArgs
