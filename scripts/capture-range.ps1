param(
    [string]$RangePath = '',
    [ValidateSet('energy','shell')][string]$Weapon = 'energy',
    [ValidateRange(100,4000)][int]$Power = 1000,
    [string]$OutputDir = '',
    [ValidateRange(0.1,3600)][double]$Seconds = 10,
    [ValidateSet('auto','overview','exposed','sheltered','gun','target')][string]$View = '',
    [switch]$Edit,
    [switch]$DriveDemo,
    [switch]$Close,
    [string]$Move = ''
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$env:WGPU_BACKEND = 'vulkan'
if (!$RangePath) {
    $RangePath = if (Test-Path -LiteralPath (Join-Path $repoRoot 'out/range-layout.ron')) { 'out/range-layout.ron' } else { 'data/range-cover.ron' }
}
if (!$OutputDir) { $OutputDir = 'out/range-capture-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff') }
$destination = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
if (Test-Path -LiteralPath $destination) { throw 'OutputDir must be new; old captures are never overwritten.' }
$binary = Join-Path $repoRoot 'target/release/blast.exe'
if (!(Test-Path -LiteralPath $binary)) { throw 'Build first: scripts/cargo-local.ps1 build --release -p render --bin blast' }
$ffmpegCmd = Get-Command ffmpeg -ErrorAction SilentlyContinue
$ffmpegPath = if ($ffmpegCmd) { $ffmpegCmd.Source } else {
    Join-Path $repoRoot 'cache/video-tools/imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe'
}
if (!(Test-Path -LiteralPath $ffmpegPath)) { throw 'Install ffmpeg and put it on PATH.' }
$framesPath = Join-Path $destination 'frames'
New-Item -ItemType Directory -Path $framesPath -Force | Out-Null
$culture = [System.Globalization.CultureInfo]::InvariantCulture
$secondsText = $Seconds.ToString($culture)
$captureArgs = @('--record', ('"{0}"' -f $framesPath), '--record-seconds', $secondsText,
    '--weapon', $Weapon, '--power', $Power.ToString($culture), '--range', ('"{0}"' -f $RangePath))
if ($Edit) { $captureArgs += '--edit' }
if ($DriveDemo) { $captureArgs += '--drive-demo' }
if ($Move) { $captureArgs += @('--move', $Move) }
if ($Close) { $captureArgs += '--close' }
if ($View) { $captureArgs += @('--view', $View) }
$process = Start-Process -FilePath $binary -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -Wait `
    -ArgumentList $captureArgs `
    -RedirectStandardOutput (Join-Path $destination 'capture.log') `
    -RedirectStandardError (Join-Path $destination 'capture-error.log')
if ($process.ExitCode -ne 0) { throw "Capture failed: $($process.ExitCode)" }
$timeline = Join-Path $framesPath 'timeline.ffconcat'
if (!(Test-Path -LiteralPath $timeline)) { throw 'Capture ended before the timeline was completed.' }
$video = Join-Path $destination 'realtime-1x.mp4'
# Each PNG keeps its actual wall-clock timestamp. Missed samples hold the
# previous frame; encoding must never compress them into a faster movie.
& $ffmpegPath -hide_banner -loglevel warning -f concat -safe 0 -i $timeline `
    -t $secondsText -vf 'fps=30:round=up' -fps_mode cfr -c:v libx264 -preset medium -crf 18 `
    -pix_fmt yuv420p -movflags +faststart $video
if ($LASTEXITCODE -ne 0) { throw 'Video encoding failed' }
Write-Output $video
