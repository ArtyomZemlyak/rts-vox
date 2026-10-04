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
    [switch]$OsPressure,
    [switch]$CameraTour,
    # Optional boundaries and calling-thread cycle counters, saved after the run.
    [switch]$CorePhases,
    [switch]$RenderTimeline,
    [switch]$RenderTasks,
    # Zero plans a bounded budget for the duration; explicit overflow is invalid.
    [ValidateRange(0, 1000000)][int]$RenderSpanLimit = 0,
    [ValidateRange(0, 10000)][int]$TriggerMilliseconds = 0,
    [switch]$CheckEveryFrame,
    # Existing frozen build: no overwriting target/release for A/B runs.
    [string]$BinaryPath = '',
    [ValidateRange(0, 128)][int]$PhysicsWorkers = 0,
    [ValidateRange(0, 16)][int]$ComputeWorkers = 0,
    [ValidateRange(960, 8192)][int]$WindowWidth = 1600,
    [ValidateRange(540, 8192)][int]$WindowHeight = 900,
    [switch]$AllowContaminated
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/range-profile-tools.ps1"
$repo = Split-Path $PSScriptRoot -Parent
$originalRayonEnvironment = $env:RAYON_NUM_THREADS
$originalRustLog = $env:RUST_LOG
Push-Location $repo
try {
    if ($PhysicsWorkers -gt 0) { $env:RAYON_NUM_THREADS = "$PhysicsWorkers" }
    if ($CorePhases -and $TriggerMilliseconds -gt 0) { throw '-CorePhases and -TriggerMilliseconds are separate diagnostic modes.' }
    if ($BinaryPath -and -not $SkipBuild) { throw '-BinaryPath requires -SkipBuild.' }
    if ($RenderSpanLimit -gt 0 -and -not $RenderTimeline) { throw '-RenderSpanLimit requires -RenderTimeline.' }
    if ($RenderTasks -and -not $RenderTimeline) { throw '-RenderTasks requires -RenderTimeline.' }
    # Cached SystemMeta spans are INFO; retain other inherited log settings.
    if ($RenderTasks) { $env:RUST_LOG = Get-RangeTaskLogFilter -RustLog $originalRustLog }
    $renderBudget = if ($RenderTimeline) { Get-RangeRenderSpanLimit -Seconds $Seconds -Requested $RenderSpanLimit } else { 0 }
    if (-not $SkipBuild) {
        $buildArgs = @('build', '--release', '-p', 'render', '--bin', 'blast')
        if ($RenderTasks) { $buildArgs += @('--features', 'render-tasks') }
        & "$PSScriptRoot/cargo-local.ps1" @buildArgs
        if ($LASTEXITCODE -ne 0) { throw 'Range build failed.' }
    }
    if (-not $Output) { $Output = "out/performance/$(Get-Date -Format yyyyMMdd-HHmmss-fff)" }
    $directory = if ([System.IO.Path]::IsPathRooted($Output)) {
        [System.IO.Path]::GetFullPath($Output)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $repo $Output))
    }
    New-Item -ItemType Directory -Force $directory | Out-Null
    $binary = if ($BinaryPath) { (Resolve-Path -LiteralPath $BinaryPath).Path } else { Join-Path $repo 'target/release/blast.exe' }
    if (-not (Test-Path -LiteralPath $binary)) { throw 'Build the release range first.' }
    $budgetMs = 1000.0 / $TargetFps
    # Run sequentially: competing windows or video readback invalidate timings.
    $report = foreach ($scenario in $Scenarios) {
        $manifest = $null
        try {
            $manifestPath = Join-Path $directory "$scenario.manifest.json"
            foreach ($artifact in "$scenario.manifest.json", "$scenario.csv", "$scenario.log", "$scenario.err", "$scenario.camera.csv", "$scenario.core.csv", "$scenario.physics.csv", "$scenario.stall.csv", "$scenario.render.csv", "$scenario.tasks.csv", "$scenario.task-names.csv", "$scenario-cpu") {
                if (Test-Path -LiteralPath (Join-Path $directory $artifact)) { throw "Existing run retained: $artifact. Use a fresh output directory." }
            }
            $binaryName = [System.IO.Path]::GetFileNameWithoutExtension($binary)
            $preflight = @(Get-RangeCompetingProcesses -Exclude @($PID) -AdditionalNames @($binaryName))
            $binaryHash = (Get-FileHash -LiteralPath $binary).Hash
            $buildEvidence = Get-RangeBuildEvidence -BinaryPath $binary -BinarySha256 $binaryHash
            $manifest = [ordered]@{
                Schema = 2; Scenario = $scenario; RequestedSeconds = $Seconds; TargetFps = $TargetFps
                StartedUtc = (Get-Date).ToUniversalTime().ToString('o')
                GitHead = (& git rev-parse HEAD); SourceAtLaunch = @(& git status --short)
                SourceProvenance = 'SourceAtLaunch is not proof of the source used to build this binary; BinarySha256 identifies the measured executable.'
                BinaryPath = $binary; BinarySha256 = $binaryHash; BuildEvidence = $buildEvidence
                InputHashes = @(Get-RangeInputHashes -Repo $repo)
                RayonEnvironment = $env:RAYON_NUM_THREADS; NavigationWorkers = $NavigationWorkers
                ComputeWorkers = $ComputeWorkers
                CameraTour = [bool]$CameraTour
                GpuTimestamps = [bool]$Gpu; CpuThreads = [bool]$CpuThreads; OsPressure = [bool]$OsPressure; CorePhases = [bool]$CorePhases; CoreTriggerMilliseconds = $TriggerMilliseconds; RenderTimeline = [bool]$RenderTimeline
                RenderSpanLimit = $renderBudget
                RenderTasks = [bool]$RenderTasks
                RustLog = $env:RUST_LOG
                EnvironmentSampleIntervalMs = 1000; PreflightProcesses = $preflight
                RequestedWindow = @($WindowWidth, $WindowHeight)
                Status = 'preflight'; Validity = $null
            }
            Save-RangeManifest $manifest $manifestPath
            if ($preflight.Count -gt 0 -and -not $AllowContaminated) {
                $manifest.Status = 'refused'
                $manifest.Validity = @{ ValidForComparison = $false; AcceptanceEligible = $false; Reasons = @('competing_process_before_start') }
                Save-RangeManifest $manifest $manifestPath
                throw 'Build/test/capture/profile process is already running. Evidence is in the manifest. -AllowContaminated collects diagnostics but cannot qualify a speed comparison.'
            }
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
            $arguments = @('--range', 'data/range-km.ron', '--stock-tuning', '--window', "$WindowWidth,$WindowHeight", '--profile', ('"' + $csv + '"'), '--profile-seconds', $Seconds) + $extra
            if ($Gpu) { $arguments += '--profile-gpu' }
            if ($CameraTour) { $arguments += '--profile-camera-tour' }
            if ($RenderTimeline) { $arguments += '--profile-render' }
            if ($RenderTasks) { $arguments += '--profile-render-tasks' }
            # Preserve automatic runs of frozen schema-1/2/3 binaries. New
            # executables plan the same duration budget internally; only an
            # explicit override requires the new CLI flag.
            if ($RenderSpanLimit -gt 0) { $arguments += @('--profile-render-spans', $RenderSpanLimit) }
            if ($CorePhases) { $arguments += '--profile-core' }
            if ($TriggerMilliseconds -gt 0) { $arguments += @('--profile-trigger-ms', $TriggerMilliseconds) }
            if ($NavigationWorkers -gt 0) { $arguments += @('--nav-workers', $NavigationWorkers) }
            if ($ComputeWorkers -gt 0) { $arguments += @('--compute-workers', $ComputeWorkers) }
            $manifest.Arguments = $arguments
            $manifest.Status = 'running'
            Save-RangeManifest $manifest $manifestPath
            $process = Start-Process -FilePath $binary -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError $err -PassThru
            $null = $process.Handle
            $manifest.ProcessId = $process.Id
            Save-RangeManifest $manifest $manifestPath
            $reasons = [System.Collections.Generic.List[string]]::new()
            if ($preflight.Count -gt 0) { $reasons.Add('competing_process_before_start') }
            try {
                # Environment/process sampling is always active. Optional thread
                # enumeration remains a separate comparison flag, outside core.
                & "$PSScriptRoot/profile-cpu-threads.ps1" -TargetProcessId $process.Id -Seconds ($Seconds + 120) -ProcessOnly:(-not $CpuThreads) -OsPressure:$OsPressure -AdditionalProcessNames @($binaryName) -Output (Join-Path $directory "$scenario-cpu") | Out-Host
            } catch { $reasons.Add('os_sampler_failed'); $manifest.SamplerError = $_.Exception.Message }
            if (-not $process.WaitForExit(1000)) {
                $reasons.Add('profile_timeout')
                # This is the process we launched, never a competing process.
                $process.Kill(); $process.WaitForExit()
            }
            $manifest.ExitCode = $process.ExitCode
            if ($process.ExitCode -ne 0) { $reasons.Add('process_failed') }
            $environmentPath = Join-Path $directory "$scenario-cpu/cpu-environment.csv"
            if (Test-Path -LiteralPath $environmentPath) {
                if (@(Import-Csv -LiteralPath $environmentPath).Count -gt 0) { $reasons.Add('competing_process_during_run') }
            } else { $reasons.Add('environment_observations_missing') }
            if ((Get-FileHash -LiteralPath $binary).Hash -ne $manifest.BinarySha256) { $reasons.Add('binary_changed_during_run') }
            if ((@(Get-RangeInputHashes -Repo $repo) | ConvertTo-Json -Compress) -ne ($manifest.InputHashes | ConvertTo-Json -Compress)) { $reasons.Add('inputs_changed_during_run') }
            $rows = if (Test-Path -LiteralPath $csv) { @(Import-Csv -LiteralPath $csv) } else { @() }
            $content = if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Raw } else { '' }
            if ($CameraTour -and -not (Test-Path -LiteralPath (Join-Path $directory "$scenario.camera.csv"))) {
                $reasons.Add('camera_tour_artifact_missing')
            }
            if ($scenario -eq 'flame') {
                if ($content -notmatch 'PROFILE stream_packets=(\d+) unit_damage=(\d+)' -or
                    [long]$Matches[1] -eq 0 -or [long]$Matches[2] -eq 0) { $reasons.Add('flame_workload_missing') }
            }
            if ($scenario -eq 'stream') {
                if ($content -notmatch 'terrain_origin=\[(\d+), (\d+), (\d+)\]' -or
                    ([long]$Matches[1] -eq 0 -and [long]$Matches[3] -eq 0)) { $reasons.Add('streaming_workload_missing') }
            }
            if ($RenderTimeline -and -not (Test-Path -LiteralPath (Join-Path $directory "$scenario.render.csv"))) {
                $reasons.Add('render_timeline_artifact_missing')
            }
            if ($RenderTasks -and (-not (Test-Path -LiteralPath (Join-Path $directory "$scenario.tasks.csv")) -or -not (Test-Path -LiteralPath (Join-Path $directory "$scenario.task-names.csv")))) { $reasons.Add('render_tasks_artifact_missing') }
            $validity = Get-RangeValidity -Rows $rows -Reasons $reasons.ToArray() -Scenario $scenario -Log $content -CorePhases:$CorePhases -TriggerPhases:($TriggerMilliseconds -gt 0) -RenderTimeline:$RenderTimeline -RenderSpanLimit $renderBudget -RenderTasks:$RenderTasks -ComputeWorkers $ComputeWorkers -CameraTour:$CameraTour -OsPressure:$OsPressure
            $manifest.Validity = $validity
            $manifest.RuntimeEvidence = @($content -split '\r?\n' | Where-Object { $_ -match '^PROFILE (schema=|core_trace_schema=|stall_capture_schema=|stall_windows=|stall_frame_signal|stall_peak_signal|render_timeline_schema|render_tasks_|stall_retained_frame|physics_task_sampling=|tick_rate=|camera_tour_|adapter|state_hash=|battle=|compute_workers=|compute_override=|physics_workers=|navigation_override=|simulation_ticks=|render_context)' })
            $manifest.Status = 'completed'
            $manifest.FinishedUtc = (Get-Date).ToUniversalTime().ToString('o')
            Save-RangeManifest $manifest $manifestPath
            $process.Dispose()
            if ($rows.Count -eq 0) { throw "$scenario produced no frame samples; failure evidence retained in $manifestPath" }
            $frameTiming = Get-RangeFrameTimes -Rows $rows
            $times = @($frameTiming.Values | Sort-Object)
            if ($times.Count -eq 0) { throw 'No monotonic frame intervals; FPS cannot be evaluated.' }
            $p99 = $times[[int][Math]::Floor($times.Count * 0.99)]
            $mean = ($times | Measure-Object -Average).Average
            $result = [pscustomobject]@{
                Scenario = $scenario
                ValidForComparison = $validity.ValidForComparison
                AcceptanceEligible = $validity.AcceptanceEligible
                InvalidReasons = $validity.Reasons -join '|'
                SimulationTicks = $validity.SimulationTicks
                SimulationRealtimeRatio = [Math]::Round($validity.SimulationRealtimeRatio, 4)
                MeetsSimulationRate = $validity.MeetsSimulationRate
                TargetFps = $TargetFps
                FrameTimingMetric = $frameTiming.Metric
                Frames = $times.Count
                MeanFps = [Math]::Round(1000.0 / $mean, 1)
                P95ms = [Math]::Round($times[[int][Math]::Floor($times.Count * 0.95)], 3)
                P99ms = [Math]::Round($p99, 3)
                MaxMs = [Math]::Round($times[-1], 3)
                FramesOverBudget = @($times | Where-Object { $_ -gt $budgetMs }).Count
                MeetsP99 = $validity.AcceptanceEligible -and $validity.MeetsSimulationRate -and $p99 -le $budgetMs
                MeetsEveryFrame = $validity.AcceptanceEligible -and $validity.MeetsSimulationRate -and $times[-1] -le $budgetMs
            }
            Write-Host "$scenario`: p99=$($result.P99ms) ms, max=$($result.MaxMs) ms, frames over ${TargetFps} FPS budget=$($result.FramesOverBudget)"
            if (-not $validity.ValidForComparison) { Write-Warning "$scenario comparison invalid: $($result.InvalidReasons)" }
            $result
            # Retain completed scenarios even if a later launch fails.
            $result | Export-Csv -LiteralPath (Join-Path $directory "$scenario.summary.csv") -NoTypeInformation -Encoding UTF8
        } catch {
            if ($null -ne $manifest) {
                $manifest.Error = $_.Exception.Message
                if ($manifest.Status -ne 'refused') {
                    $manifest.Status = 'failed'
                    $manifest.Validity = @{ValidForComparison=$false; AcceptanceEligible=$false; Reasons=@('runner_failed')}
                }
                Save-RangeManifest $manifest $manifestPath
            }
            throw
        }
    }
    $report | Export-Csv -LiteralPath (Join-Path $directory 'summary.csv') -NoTypeInformation -Encoding UTF8
    $report | Format-Table -AutoSize
    Write-Host "Results: $directory"
    if ($CheckBudget -and @($report | Where-Object { -not $_.MeetsP99 -or ($CheckEveryFrame -and -not $_.MeetsEveryFrame) }).Count -gt 0) {
        throw "The $TargetFps FPS budget was exceeded; maxima and slow frames are retained in summary.csv."
    }
} finally {
    $env:RUST_LOG = $originalRustLog
    $env:RAYON_NUM_THREADS = $originalRayonEnvironment
    Pop-Location
}
