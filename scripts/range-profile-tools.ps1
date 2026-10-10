# Shared by the runner and OS sampler; no simulation code or dependencies.
function ConvertTo-RangeInvariantCsvRow {
    param([Parameter(ValueFromPipeline = $true)]$InputObject)
    process {
        $row = [ordered]@{}
        foreach ($property in $InputObject.PSObject.Properties) {
            $value = $property.Value
            if ($value -is [double] -or $value -is [single] -or $value -is [decimal]) {
                $value = $value.ToString($null, [System.Globalization.CultureInfo]::InvariantCulture)
            }
            $row[$property.Name] = $value
        }
        [pscustomobject]$row
    }
}

function Get-RangeBuildEvidence {
    param([string]$BinaryPath, [string]$BinarySha256)
    $path = "$BinaryPath.build.json"
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $evidence = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($evidence.Schema -ne 1 -or $evidence.BinarySha256 -ne $BinarySha256) {
        throw "Build evidence does not identify this binary: $path"
    }
    [pscustomobject]@{ ManifestPath = $path; Evidence = $evidence }
}
function Get-RangeRenderSpanLimit {
    param([ValidateRange(1, 3600)][int]$Seconds, [ValidateRange(0, 1000000)][int]$Requested = 0)
    if ($Requested -gt 0) { return $Requested }
    # Same bounded planning rule as render::timeline::span_limit_for_seconds.
    [int][Math]::Min(1000000, [Math]::Max(100000, ($Seconds + 1) * 400 * 13))
}
function Get-RangeCompetingProcesses {
    param([int[]]$Exclude = @(), [string[]]$AdditionalNames = @())
    $names = @('cargo','rustc','sim_core*','nav-*','phys-*','bench_*','blast*','ffmpeg') + $AdditionalNames
    foreach ($candidate in (Get-Process -Name $names -ErrorAction SilentlyContinue | Sort-Object Id -Unique)) {
        if ($candidate.Id -in $Exclude) { continue }
        try {
            [pscustomobject]@{
                ProcessId = $candidate.Id
                Name = $candidate.ProcessName
                StartedUtc = $candidate.StartTime.ToUniversalTime().ToString('o')
                CpuMs = $candidate.TotalProcessorTime.TotalMilliseconds
            }
        } catch { }
    }
}

function Get-RangeInputHashes {
    param([string]$Repo)
    @(Get-ChildItem -LiteralPath (Join-Path $Repo 'data') -Recurse -File -Filter '*.ron' |
        Sort-Object FullName | ForEach-Object {
            [pscustomobject]@{
                Path = $_.FullName.Substring($Repo.Length + 1).Replace('\', '/')
                Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
            }
        })
}

function Save-RangeManifest {
    param([object]$Manifest, [string]$Path)
    $Manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Get-RangeFrameTimes {
    param([object[]]$Rows)
    $culture = [System.Globalization.CultureInfo]::InvariantCulture
    $values = [System.Collections.Generic.List[double]]::new()
    $issues = [System.Collections.Generic.List[string]]::new()
    $previousStart = $null; $previousEnd = $null
    foreach ($row in $Rows) {
        try {
            $bevy = [double]::Parse($row.frame_ms, $culture)
            if ($bevy -le 0 -or [double]::IsNaN($bevy) -or [double]::IsInfinity($bevy)) { $issues.Add('invalid_frame_time') }
            if (-not $row.PSObject.Properties['frame_start_ns'] -or -not $row.PSObject.Properties['frame_end_ns']) {
                $issues.Add('monotonic_frame_clock_missing'); continue
            }
            $start = [long]::Parse($row.frame_start_ns, $culture)
            $end = [long]::Parse($row.frame_end_ns, $culture)
            $gap = [long]::Parse($row.frame_gap_ns, $culture)
            $expectedGap = if ($null -ne $previousEnd) { $start - $previousEnd } else { 0 }
            if ($start -lt 0 -or $end -lt $start -or $expectedGap -lt 0 -or $gap -ne $expectedGap) { $issues.Add('invalid_frame_clock') }
            $supplied = if ($row.observed_frame_ms -ne $null -and $row.observed_frame_ms -ne '') { [double]::Parse($row.observed_frame_ms, $culture) } else { $null }
            if ($null -ne $supplied -and ($supplied -le 0 -or [double]::IsNaN($supplied) -or [double]::IsInfinity($supplied))) { $issues.Add('invalid_observed_frame_time') }
            if ($null -ne $previousStart) {
                $duration = ($start - $previousStart) / 1000000.0
                if ($duration -le 0) { $issues.Add('invalid_frame_clock') }
                if ($row.PSObject.Properties['observed_frame_ms'] -and $null -eq $supplied) { $issues.Add('observed_frame_time_missing') }
                if ($null -ne $supplied -and [Math]::Abs($supplied - $duration) -gt 0.000002) { $issues.Add('observed_frame_clock_mismatch') }
                $values.Add($duration)
            } elseif ($null -ne $supplied) { $values.Add($supplied) }
            $previousStart = $start; $previousEnd = $end
        } catch { $issues.Add('invalid_frame_clock') }
    }
    if ($values.Count -eq 0) { $issues.Add('no_observed_frame_intervals') }
    [pscustomobject]@{ Metric='profile_begin_interval'; Values=$values.ToArray(); Reasons=@($issues | Select-Object -Unique) }
}

function Get-RangeTaskLogFilter {
    param([string]$RustLog)
    $target = 'bevy_ecs::system::function_system=info'
    if ($RustLog) { "$RustLog,$target" } else { $target }
}

function Get-RangeValidity {
    param([object[]]$Rows, [string[]]$Reasons = @(), [string]$Scenario, [string]$Log,
        [switch]$CorePhases, [switch]$TriggerPhases, [switch]$RenderTimeline, [int]$RenderSpanLimit = 0, [switch]$RenderTasks, [int]$ComputeWorkers = 0, [int]$PhysicsWorkers = 0, [int]$NavigationWorkers = 0, [switch]$CameraTour, [switch]$OsPressure)
    $issues = [System.Collections.Generic.List[string]]::new()
    foreach ($reason in $Reasons) { $issues.Add($reason) }
    $timing = Get-RangeFrameTimes -Rows $Rows
    foreach ($reason in $timing.Reasons) { $issues.Add($reason) }
    $ticks = 0L
    $seconds = 0.0
    $rate = if ($Log -match 'PROFILE tick_rate=(\d+)') { [int]$Matches[1] } else { 30 }
    $culture = [System.Globalization.CultureInfo]::InvariantCulture
    if ($Rows.Count -eq 0) { $issues.Add('no_frame_samples') }
    foreach ($row in $Rows) {
        $frameMs = [double]::Parse($row.frame_ms, $culture)
        if ($frameMs -le 0 -or [double]::IsNaN($frameMs) -or [double]::IsInfinity($frameMs)) {
            $issues.Add('invalid_frame_time'); break
        }
        $ticks += [long]::Parse($row.simulation_ticks, $culture)
        $seconds = [double]::Parse($row.seconds, $culture)
        if ($row.frame_end_ns -ne $null -and $row.frame_end_ns -ne '') { $seconds = [long]::Parse($row.frame_end_ns, $culture) / 1000000000.0 }
    }
    if ($ticks -eq 0) { $issues.Add('simulation_did_not_advance') }
    $tourDeclared = $Log -match '(?m)^PROFILE camera_tour_schema='
    if ($tourDeclared -ne [bool]$CameraTour -or ($tourDeclared -and
        $Log -notmatch '(?m)^PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1\r?$')) {
        $issues.Add('camera_tour_mode_mismatch')
    }
    if ($Log -notmatch '(?m)^PROFILE render_context_schema=2\r?$' -or
        $Log -notmatch '(?m)^PROFILE render_context=RenderContext \{' -or
        $Log -notmatch '(?m)^PROFILE render_context_step=\d+ value=RenderContext \{' -or
        $Log -notmatch '(?m)^PROFILE render_context_changed=(true|false)\r?$') { $issues.Add('render_context_missing') }
    if ($Log -match '(?m)^PROFILE render_context_changed=true\r?$') { $issues.Add('render_context_changed_during_run') }
    $contexts = @([regex]::Matches($Log, '(?m)^PROFILE render_context(?:_step=\d+ value)?=(RenderContext \{[^\r\n]+\})\r?$'))
    $focusValues = @($contexts | ForEach-Object {
        if ($_.Groups[1].Value -match '\bfocused: (true|false)(?:,| \})') { $Matches[1] }
        else { $issues.Add('render_focus_missing') }
    })
    if ($contexts.Count -eq 0) { $issues.Add('render_focus_missing') }
    if (@($focusValues | Select-Object -Unique).Count -gt 1) { $issues.Add('render_focus_changed_during_run') }
    if ($Log -notmatch 'PROFILE state_hash=[0-9a-f]{16}\b') { $issues.Add('state_hash_missing') }
    if ($Log -notmatch 'PROFILE physics_workers=\d+ navigation_workers=\d+') { $issues.Add('actual_worker_counts_missing') }
    $workers = [regex]::Matches($Log, '(?m)^PROFILE physics_workers=(\d+) navigation_workers=(\d+)\r?$')
    foreach ($pool in @(
        @{ Name = 'physics'; Requested = $PhysicsWorkers; Group = 1 },
        @{ Name = 'navigation'; Requested = $NavigationWorkers; Group = 2 }
    )) {
        if ($pool.Requested -le 0) { continue }
        $override = [regex]::Matches($Log, ('(?m)^PROFILE ' + $pool.Name + '_override=Some\((\d+)\)\r?$'))
        if ($workers.Count -ne 1 -or $override.Count -ne 1 -or
            [int]$workers[0].Groups[$pool.Group].Value -ne $pool.Requested -or
            [int]$override[0].Groups[1].Value -ne $pool.Requested) {
            $issues.Add($pool.Name + '_workers_override_mismatch')
        }
    }
    if ($Log -notmatch 'PROFILE adapter_name=.+adapter_backend=.+adapter_device_type=') {
        $issues.Add('actual_adapter_missing')
    } elseif ($Log -match 'adapter_device_type=Cpu\b') {
        $issues.Add('software_adapter')
    }
    if ($RenderTimeline) {
        $renderMeta = [regex]::Matches($Log, '(?m)^PROFILE render_timeline_schema=([1234]) render_span_limit=(\d+) render_spans_started=(\d+) render_spans_retained=(\d+) render_spans_dropped=(\d+) render_spans_incomplete=(\d+) render_cutoff_ns=\d+ render_pipelined=(true|false)(?: render_gas_details=(true|false))?(?: render_span_bytes=(\d+))?\r?$')
        if ($renderMeta.Count -ne 1) { $issues.Add('render_timeline_metadata_missing') }
        else {
            $g = $renderMeta[0].Groups
            $schema = [int]$g[1].Value; $limit = [long]$g[2].Value
            $started = [long]$g[3].Value; $retained = [long]$g[4].Value
            $dropped = [long]$g[5].Value; $incomplete = [long]$g[6].Value
            if (($schema -lt 4 -and ($limit -ne 100000 -or $g[9].Success)) -or
                ($schema -eq 4 -and ($limit -lt 1 -or $limit -gt 1000000 -or -not $g[9].Success -or [long]$g[9].Value -lt 1 -or [long]$g[9].Value -gt 1024)) -or
                $retained -ne [Math]::Min($started, $limit) -or $started -ne ($retained + $dropped) -or $incomplete -gt $retained -or $retained -eq 0) {
                $issues.Add('render_timeline_budget_count_mismatch')
            }
            if ($schema -eq 4 -and $RenderSpanLimit -gt 0 -and $limit -ne $RenderSpanLimit) { $issues.Add('render_timeline_budget_manifest_mismatch') }
            if ($schema -eq 4 -and $dropped -gt 0) { $issues.Add('render_timeline_truncated') }
        }
    }
    if ($RenderTimeline -and $Log -match '(?m)^PROFILE render_timeline_schema=[234] ' -and
        $Log -notmatch '(?m)^PROFILE render_timeline_schema=[234] [^\r\n]+ render_gas_details=(true|false)( render_span_bytes=\d+)?\r?$') {
        $issues.Add('render_timeline_detail_mode_missing')
    }
    # A separate comparison key must still match scenario, inputs and flags.
    $taskBuild = $Log -match '(?m)^PROFILE render_tasks_build=true\r?$'
    if ($ComputeWorkers -gt 0) {
        $compute = [regex]::Matches($Log, '(?m)^PROFILE compute_workers=(\d+)\r?$')
        $override = [regex]::Matches($Log, '(?m)^PROFILE compute_override=Some\((\d+)\)\r?$')
        if ($compute.Count -ne 1 -or $override.Count -ne 1 -or
            [int]$compute[0].Groups[1].Value -ne $ComputeWorkers -or
            [int]$override[0].Groups[1].Value -ne $ComputeWorkers) { $issues.Add('compute_workers_override_mismatch') }
    }
    if ($RenderTasks) {
        if (-not $taskBuild -or -not $RenderTimeline) { $issues.Add('render_tasks_build_or_mode_missing') }
        $taskMeta = [regex]::Matches($Log, '(?m)^PROFILE render_tasks_schema=1 render_task_limit=2000000 render_tasks_started=(\d+) render_tasks_retained=(\d+) render_tasks_dropped=(\d+) render_tasks_incomplete=(\d+) render_task_cutoff_ns=\d+ render_task_names=(\d+) render_task_excluded_names=\d+ render_task_bytes=(\d+)(?: render_task_stride=(1|8))?\r?$')
        if ($taskMeta.Count -ne 1) { $issues.Add('render_tasks_metadata_missing') }
        else {
            $g = $taskMeta[0].Groups
            if ([long]$g[2].Value -ne [Math]::Min(2000000, [long]$g[1].Value) -or
                [long]$g[1].Value -ne ([long]$g[2].Value + [long]$g[3].Value) -or
                [long]$g[4].Value -gt [long]$g[2].Value -or
                [long]$g[5].Value -eq 0 -or [long]$g[6].Value -lt 1 -or [long]$g[6].Value -gt 1024) { $issues.Add('render_tasks_budget_count_mismatch') }
            if ([long]$g[3].Value -gt 0) { $issues.Add('render_tasks_truncated') }
            if ([long]$g[2].Value -eq 0) { $issues.Add('render_tasks_empty_capture') }
        }
    }
    $valid = $issues.Count -eq 0
    # Only the authored actions diagnostic deliberately pauses/resets the clock.
    # A fast renderer over a slowed canister/drive/shell simulation is no FPS acceptance.
    $meetsRate = $Scenario -eq 'actions' -or $ticks -ge ([long][Math]::Floor($seconds * $rate) - 1)
    [pscustomobject]@{
        ValidForComparison = $valid
        AcceptanceEligible = $valid -and -not $CorePhases -and -not $TriggerPhases -and -not $RenderTimeline -and -not $taskBuild -and -not $RenderTasks -and -not $OsPressure -and $meetsRate
        Reasons = @($issues | Select-Object -Unique)
        SimulationTicks = $ticks
        ObservedSeconds = $seconds
        SimulationRealtimeRatio = if ($seconds -gt 0) { $ticks / ($seconds * $rate) } else { 0.0 }
        MeetsSimulationRate = $meetsRate
        Mode = if ($RenderTasks) { 'render_task_diagnostic' } elseif ($RenderTimeline) { 'render_diagnostic' } elseif ($CorePhases) { 'diagnostic' } elseif ($TriggerPhases) { 'trigger_diagnostic' } elseif ($OsPressure) { 'os_pressure_diagnostic' } else { 'timing' }
        FrameTimingMetric = $timing.Metric
    }
}
