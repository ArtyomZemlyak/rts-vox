param(
    [string]$OutputDir = '',
    [int]$FrameCount = 0,
    [ValidateSet('shell', 'energy')][string]$Weapon = 'shell',
    [string]$RangePath = '',
    [ValidateRange(100,4000)][int]$Power = 1000
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = if ($Weapon -eq 'energy') { 'out/energy-cannon' } else { 'out/blast-cinematic' }
}
if ($FrameCount -eq 0) { $FrameCount = if ($Weapon -eq 'energy') { 420 } else { 330 } }
Push-Location $repoRoot
try {
    $binary = Join-Path $repoRoot 'target/release/blast.exe'
    if (!(Test-Path -LiteralPath $binary)) { throw 'Build first: cargo build --release -p render --bin blast' }
    $destination = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
    $framesPath = Join-Path $destination 'frames'
    New-Item -ItemType Directory -Path $framesPath -Force | Out-Null
    $ffmpegCmd = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($ffmpegCmd) { $ffmpegPath = $ffmpegCmd.Source }
    else {
        $ffmpegPath = Join-Path $repoRoot 'cache/video-tools/imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe'
        if (!(Test-Path -LiteralPath $ffmpegPath)) { throw 'Install ffmpeg and put it on PATH to encode the capture.' }
    }
    $env:WGPU_BACKEND = 'vulkan'
    $captureArgs = @('--weapon', $Weapon, '--power', "$Power", '--capture', ('"{0}"' -f $framesPath), '--frames', "$FrameCount")
    if ($RangePath) { $captureArgs += @('--range', ('"{0}"' -f $RangePath)) }
    $process = Start-Process -FilePath $binary -WindowStyle Hidden -PassThru -Wait `
        -ArgumentList $captureArgs `
        -RedirectStandardOutput (Join-Path $destination 'capture.log') `
        -RedirectStandardError (Join-Path $destination 'capture-error.log')
    if ($process.ExitCode -ne 0) { throw "Capture failed: $($process.ExitCode)" }
    $videoName = if ($Weapon -eq 'energy') { 'energy-cannon.mp4' } else { 'blast.mp4' }
    $video = Join-Path $destination $videoName
    & $ffmpegPath -hide_banner -loglevel warning -y -framerate 30 -start_number 1 `
        -i (Join-Path $framesPath '%04d.png') -frames:v ($FrameCount - 1) `
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -movflags +faststart $video
    if ($LASTEXITCODE -ne 0) { throw 'Video encoding failed' }
    Write-Output $video
} finally { Pop-Location }
