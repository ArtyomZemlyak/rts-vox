param(
    [ValidateRange(90, 1800)][int]$Frames = 240,
    [ValidateRange(1, 10)][int]$Runs = 3,
    [string]$Output = '',
    [string]$ExistingFrames = '',
    [switch]$SkipBuild
)
# Native temporal regression for range-cover, --drive --view overview, 1600x900.
# The four fixed patches exclude UI, moving patrols and their shadows. This is
# a stability check, not an FPS benchmark: PNG readback stalls the renderer.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Add-Type -AssemblyName System.Drawing
if (-not ('RangeFlickerProbe' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
public sealed class RangeFlickerProbe {
    readonly Rectangle[] patches = {
        new Rectangle(1350, 860, 200, 35), // empty area outside the map
        new Rectangle(740, 140, 75, 60),   // stationary exposed tank
        new Rectangle(920, 120, 50, 60),   // concrete wall
        new Rectangle(650, 400, 40, 40)    // terrain
    };
    readonly byte[][] previous = new byte[4][];
    public double[] Sample(string path) {
        var result = new double[8];
        using (var image = new Bitmap(path)) {
            if (image.Width != 1600 || image.Height != 900)
                throw new InvalidOperationException("Expected 1600x900 range frames.");
            for (int i = 0; i < patches.Length; ++i) {
                var r = patches[i];
                var values = new byte[r.Width * r.Height * 3];
                int n = 0;
                for (int y = r.Top; y < r.Bottom; ++y)
                    for (int x = r.Left; x < r.Right; ++x) {
                        var c = image.GetPixel(x, y);
                        values[n++] = c.R; values[n++] = c.G; values[n++] = c.B;
                    }
                for (int j = 0; j < values.Length; ++j) {
                    result[i] += values[j];
                    if (previous[i] != null)
                        result[i + 4] += Math.Abs(values[j] - previous[i][j]);
                }
                result[i] /= values.Length;
                result[i + 4] /= values.Length;
                previous[i] = values;
            }
        }
        return result;
    }
}
'@
}
Push-Location $repo
try {
    if ($ExistingFrames) {
        $directories = @((Resolve-Path -LiteralPath $ExistingFrames).Path)
    } else {
        if (-not $SkipBuild) {
            & "$PSScriptRoot/cargo-local.ps1" build --release -p render --bin blast
            if ($LASTEXITCODE -ne 0) { throw 'Range build failed.' }
        }
        if (-not $Output) { $Output = "out/flicker/$(Get-Date -Format yyyyMMdd-HHmmss-fff)" }
        $root = if ([System.IO.Path]::IsPathRooted($Output)) {
            [System.IO.Path]::GetFullPath($Output)
        } else {
            [System.IO.Path]::GetFullPath((Join-Path $repo $Output))
        }
        if (Test-Path -LiteralPath $root) { throw "Choose a fresh output directory: $root" }
        New-Item -ItemType Directory -Path $root | Out-Null
        $directories = @(foreach ($run in 1..$Runs) {
            $dir = Join-Path $root "run-$run"
            $arguments = @('--range', 'data/range-cover.ron', '--stock-tuning', '--drive', '--view', 'overview',
                '--capture', ('"' + $dir + '"'), '--frames', $Frames)
            $process = Start-Process -FilePath 'target/release/blast.exe' -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput "$dir.log" -RedirectStandardError "$dir.err" -Wait -PassThru
            if ($process.ExitCode -ne 0) { throw "Range capture failed: $dir.err" }
            $dir
        })
    }
    $names = @('sky', 'tank', 'wall', 'ground')
    $thresholds = @(1.0, 2.0, 2.0, 2.0)
    $failed = $false
    foreach ($dir in $directories) {
        $files = @(Get-ChildItem -LiteralPath $dir -Filter '*.png' |
            Where-Object { $_.BaseName -match '^\d+$' -and [int]$_.BaseName -ge 20 } |
            Sort-Object { [int]$_.BaseName })
        if ($files.Count -lt 60) { throw "Need at least 60 settled frames: $dir" }
        $probe = New-Object RangeFlickerProbe
        $maxDelta = @(0.0, 0.0, 0.0, 0.0)
        $minBrightness = @(255.0, 255.0, 255.0, 255.0)
        $bad = @()
        foreach ($file in $files) {
            $sample = $probe.Sample($file.FullName)
            for ($i = 0; $i -lt 4; $i++) {
                $maxDelta[$i] = [Math]::Max($maxDelta[$i], $sample[$i + 4])
                $minBrightness[$i] = [Math]::Min($minBrightness[$i], $sample[$i])
                if ($sample[$i + 4] -gt $thresholds[$i] -or $sample[$i] -lt 10.0) {
                    $bad += "$($file.BaseName):$($names[$i])"
                }
            }
        }
        $report = [pscustomobject]@{
            directory = $dir; frames = $files.Count; patches = $names
            maximumMeanRgbChange = $maxDelta; minimumMeanRgb = $minBrightness
            failures = $bad; passed = ($bad.Count -eq 0)
        }
        $report | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $dir 'stability.json')
        $report | ConvertTo-Json -Depth 4
        $failed = $failed -or $bad.Count -ne 0
    }
    if ($failed) { throw 'Temporal instability detected; see stability.json.' }
} finally { Pop-Location }
