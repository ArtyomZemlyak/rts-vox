param(
    [string]$SavePath = '',
    [string]$ScenarioPath = '',
    [string]$OutputDir = '',
    [ValidateRange(0.1,3600)][double]$Seconds = 10,
    [double]$Distance = 0.42,
    [double]$Pitch = 0.78,
    [double]$Yaw = -0.5
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
if ($SavePath -and $ScenarioPath) { throw 'Choose SavePath or ScenarioPath.' }
if (!$SavePath -and !$ScenarioPath) { $ScenarioPath = 'data/battlefield.ron' }
if (!$OutputDir) { $OutputDir = 'out/play-capture-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff') }
$destination = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
if (Test-Path -LiteralPath $destination) { throw 'OutputDir must be new; old captures are never overwritten.' }
$binary = Join-Path $repoRoot 'target/release/play.exe'
if (!(Test-Path -LiteralPath $binary)) { throw 'Build first: scripts/cargo-local.ps1 build --release -p render --bin play' }
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
    '--speed', '1', '--dist', $Distance.ToString($culture), '--pitch', $Pitch.ToString($culture), '--yaw', $Yaw.ToString($culture))
if ($SavePath) { $captureArgs += @('--load', ('"{0}"' -f $SavePath)) }
else { $captureArgs += @('--scenario', ('"{0}"' -f $ScenarioPath)) }
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
