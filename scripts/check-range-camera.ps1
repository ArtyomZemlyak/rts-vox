param(
    [ValidateRange(180, 600)][int]$Frames = 240,
    [string]$Output = '',
    [string]$ExistingFrames = '',
    [switch]$SkipBuild
)
# Detect large colour jumps during a deterministic zoom-out / zoom-in / pan.
# This complements the stationary patch test. It catches whole-view flashing
# and renderer substitution; it does not certify subpixel temporal antialiasing.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
public static class RangeCameraMean {
    public static double[] Read(string path) {
        var sum = new double[3]; int n = 0;
        using (var image = new Bitmap(path)) {
            if (image.Width != 1600 || image.Height != 900)
                throw new InvalidOperationException("Expected 1600x900 range frames.");
            // Exclude the sidebar, where labels and the FPS counter change.
            for (int y = 20; y < 880; y += 12)
                for (int x = 320; x < 1580; x += 12) {
                    var c = image.GetPixel(x, y);
                    sum[0] += c.R; sum[1] += c.G; sum[2] += c.B; n++;
                }
        }
        for (int i = 0; i < 3; i++) sum[i] /= n;
        return sum;
    }
}
'@
Push-Location $repo
try {
    if ($ExistingFrames) {
        $dir = (Resolve-Path -LiteralPath $ExistingFrames).Path
    } else {
        if (-not $SkipBuild) {
            & "$PSScriptRoot/cargo-local.ps1" build --release -p render --bin blast
            if ($LASTEXITCODE -ne 0) { throw 'Range build failed.' }
        }
        if (-not $Output) { $Output = "out/camera/$(Get-Date -Format yyyyMMdd-HHmmss-fff)" }
        $dir = if ([System.IO.Path]::IsPathRooted($Output)) {
            [System.IO.Path]::GetFullPath($Output)
        } else {
            [System.IO.Path]::GetFullPath((Join-Path $repo $Output))
        }
        if (Test-Path -LiteralPath $dir) { throw "Choose a fresh output directory: $dir" }
        New-Item -ItemType Directory -Path $dir | Out-Null
        $arguments = @('--range', 'data/range-km.ron', '--view', 'overview',
            '--camera-sweep', '--weapon', 'shell', '--capture', ('"' + $dir + '"'), '--frames', $Frames)
        $process = Start-Process -FilePath 'target/release/blast.exe' -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput "$dir.log" -RedirectStandardError "$dir.err" -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Range capture failed: $dir.err" }
    }
    $files = @(Get-ChildItem -LiteralPath $dir -Filter '*.png' |
        Where-Object { $_.BaseName -match '^\d+$' } | Sort-Object BaseName)
    if ($files.Count -lt 160) { throw 'Need the complete outward and return camera sweep.' }
    $previous = $null
    $rows = @(foreach ($file in $files) {
        $rgb = [RangeCameraMean]::Read($file.FullName)
        if ($null -ne $previous) {
            $delta = 0.0
            for ($i = 0; $i -lt 3; $i++) {
                $delta = [Math]::Max($delta, [Math]::Abs($rgb[$i] - $previous[$i]))
            }
            [pscustomobject]@{ frame = [int]$file.BaseName; maximumMeanChannelChange = $delta }
        }
        $previous = $rgb
    })
    $rows | Export-Csv -LiteralPath (Join-Path $dir 'camera-colour-deltas.csv') -NoTypeInformation
    $worst = $rows | Sort-Object maximumMeanChannelChange -Descending | Select-Object -First 1
    $report = [pscustomobject]@{
        frames = $files.Count; worstFrame = $worst.frame
        maximumMeanChannelChange = $worst.maximumMeanChannelChange
        threshold = 8.0; passed = $worst.maximumMeanChannelChange -le 8.0
    }
    $report | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dir 'camera-stability.json') -Encoding UTF8
    $report | ConvertTo-Json
    if (-not $report.passed) { throw 'Large camera colour jump; see camera-stability.json.' }
} finally { Pop-Location }
