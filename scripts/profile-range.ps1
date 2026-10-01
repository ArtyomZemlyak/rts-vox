param(
    [ValidateRange(5, 3600)][int]$Seconds = 30,
    [ValidateSet('battle', 'battle-eight', 'world-battle', 'energy', 'shell', 'clouds', 'drive', 'stream', 'flame', 'actions')]
    [string[]]$Scenarios = @('battle', 'battle-eight', 'energy', 'shell', 'clouds', 'drive', 'flame'),
    [string]$Output = '',
    [switch]$SkipBuild,
    [switch]$Gpu,
    [switch]$CheckBudget,
    [ValidateRange(30, 1000)][int]$TargetFps = 120,
    # Zero preserves the scene default; 1 selects the serial route path.
    [ValidateRange(0, 8)][int]$NavigationWorkers = 0,
    [switch]$CpuThreads,
    [switch]$CheckEveryFrame
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    if (-not $SkipBuild) {
        & "$PSScriptRoot/cargo-local.ps1" build --release -p render --bin blast
        if ($LASTEXITCODE -ne 0) { throw 'Range build failed.' }
    }
    if (-not $Output) { $Output = "out/performance/$(Get-Date -Format yyyyMMdd-HHmmss)" }
    $directory = if ([System.IO.Path]::IsPathRooted($Output)) {
        [System.IO.Path]::GetFullPath($Output)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $repo $Output))
    }
    New-Item -ItemType Directory -Force $directory | Out-Null
    $binary = Join-Path $repo 'target/release/blast.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Build the release range first.' }
    $budgetMs = 1000.0 / $TargetFps
    # Run sequentially: competing windows or video readback invalidate timings.
    $report = foreach ($scenario in $Scenarios) {
        $extra = switch ($scenario) {
            'actions'     { @('--drive', '--view', 'overview', '--realtime', '--profile-actions') }
            'battle'       { @('--battle') }
            'battle-eight' { @('--battle-eight') }
            'world-battle' { @('--world-battle', '--ai-vs-ai') }
            'energy'       { @('--weapon', 'energy', '--power', '4000', '--open', '--view', 'exposed') }
            'shell'        { @('--weapon', 'shell', '--power', '4000', '--view', 'overview') }
            'clouds'       { @('--drive', '--profile-clouds', '8') }
            'drive'        { @('--move', '2200,336') }
            'stream'       { @('--move', '2200,336', '--view', 'gun') }
            'flame'        { @('--weapon', 'flame', '--open', '--view', 'exposed', '--attack-unit', '1') }
        }
        $csv = Join-Path $directory "$scenario.csv"
        if ($scenario -eq 'actions' -and $Seconds -lt 15) { throw 'The actions scenario needs at least 15 seconds.' }
        $log = Join-Path $directory "$scenario.log"
        $err = Join-Path $directory "$scenario.err"
        $arguments = @('--range', 'data/range-km.ron', '--stock-tuning', '--profile', ('"' + $csv + '"'), '--profile-seconds', $Seconds) + $extra
        if ($Gpu) { $arguments += '--profile-gpu' }
        if ($NavigationWorkers -gt 0) { $arguments += @('--nav-workers', $NavigationWorkers) }
        # -Wait keeps the exit status available in Windows PowerShell 5.1 too.
        if ($CpuThreads) {
            # Read this process's public counters; no kernel trace or system
            # profiling policy is needed. Sample once per second, outside the
            # renderer. Compare only runs using the same sampling option.
            $process = Start-Process -FilePath $binary -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError $err -PassThru
            # Retain the handle before the counter script observes exit;
            # Windows PowerShell 5.1 otherwise loses the child's ExitCode.
            $null = $process.Handle
            # Formatting records from the sampler must not become scenario
            # results in the enclosing foreach or corrupt summary.csv.
            & "$PSScriptRoot/profile-cpu-threads.ps1" -TargetProcessId $process.Id -Seconds ([Math]::Min(3600, $Seconds + 120)) -Output (Join-Path $directory "$scenario-cpu") | Out-Host
            $process.WaitForExit()
        } else {
            $process = Start-Process -FilePath $binary -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError $err -PassThru -Wait
        }
        if ($process.ExitCode -ne 0) { throw "$scenario failed; see $err" }
        if ($scenario -eq 'flame') {
            $contacts = Select-String -LiteralPath $log -Pattern 'PROFILE stream_packets=(\d+) unit_damage=(\d+)'
            if (-not $contacts -or [long]$contacts.Matches[0].Groups[1].Value -eq 0 -or [long]$contacts.Matches[0].Groups[2].Value -eq 0) {
                throw 'Flamethrower benchmark did not produce packets and real unit damage.'
            }
        }
        if ($scenario -eq 'stream') {
            # This preset starts in the origin window. Following the gun must
            # actually move that window; a stationary overview is no proof.
            $origin = Select-String -LiteralPath $log -Pattern 'terrain_origin=\[(\d+), (\d+), (\d+)\]'
            if (-not $origin -or ([long]$origin.Matches[0].Groups[1].Value -eq 0 -and [long]$origin.Matches[0].Groups[3].Value -eq 0)) {
                throw 'Streaming benchmark never moved the terrain window; increase -Seconds (try 90).'
            }
        }
        $rows = @(Import-Csv -LiteralPath $csv)
        if ($rows.Count -eq 0) { throw "$scenario produced no frame samples." }
        $culture = [System.Globalization.CultureInfo]::InvariantCulture
        $times = @($rows | ForEach-Object { [double]::Parse($_.frame_ms, $culture) } | Sort-Object)
        $p99 = $times[[int][Math]::Floor($times.Count * 0.99)]
        $mean = ($times | Measure-Object -Average).Average
        $result = [pscustomobject]@{
            Scenario = $scenario
            TargetFps = $TargetFps
            Frames = $times.Count
            MeanFps = [Math]::Round(1000.0 / $mean, 1)
            P95ms = [Math]::Round($times[[int][Math]::Floor($times.Count * 0.95)], 3)
            P99ms = [Math]::Round($p99, 3)
            MaxMs = [Math]::Round($times[-1], 3)
            FramesOverBudget = @($times | Where-Object { $_ -gt $budgetMs }).Count
            MeetsP99 = $p99 -le $budgetMs
            MeetsEveryFrame = $times[-1] -le $budgetMs
        }
        Write-Host "$scenario`: p99=$($result.P99ms) ms, max=$($result.MaxMs) ms, frames over ${TargetFps} FPS budget=$($result.FramesOverBudget)"
        $result
    }
    $report | Export-Csv -LiteralPath (Join-Path $directory 'summary.csv') -NoTypeInformation -Encoding UTF8
    $report | Format-Table -AutoSize
    Write-Host "Results: $directory"
    if ($CheckBudget -and @($report | Where-Object { -not $_.MeetsP99 -or ($CheckEveryFrame -and -not $_.MeetsEveryFrame) }).Count -gt 0) {
        throw "The $TargetFps FPS budget was exceeded; maxima and slow frames are retained in summary.csv."
    }
} finally {
    Pop-Location
}
