# Shared by the runner and OS sampler; no simulation code or dependencies.
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

function Get-RangeValidity {
    param([object[]]$Rows, [string[]]$Reasons = @(), [string]$Scenario, [string]$Log,
        [switch]$CorePhases)
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
    if ($Log -notmatch '(?m)^PROFILE render_context_schema=1\r?$' -or
        $Log -notmatch '(?m)^PROFILE render_context=RenderContext \{' -or
        $Log -notmatch '(?m)^PROFILE render_context_step=\d+ value=RenderContext \{' -or
        $Log -notmatch '(?m)^PROFILE render_context_changed=(true|false)\r?$') { $issues.Add('render_context_missing') }
    if ($Log -match '(?m)^PROFILE render_context_changed=true\r?$') { $issues.Add('render_context_changed_during_run') }
    if ($Log -notmatch 'PROFILE state_hash=[0-9a-f]{16}\b') { $issues.Add('state_hash_missing') }
    if ($Log -notmatch 'PROFILE physics_workers=\d+ navigation_workers=\d+') { $issues.Add('actual_worker_counts_missing') }
    if ($Log -notmatch 'PROFILE adapter_name=.+adapter_backend=.+adapter_device_type=') {
        $issues.Add('actual_adapter_missing')
    } elseif ($Log -match 'adapter_device_type=Cpu\b') {
        $issues.Add('software_adapter')
    }
    # A separate comparison key must still match scenario, inputs and flags.
    $valid = $issues.Count -eq 0
    [pscustomobject]@{
        ValidForComparison = $valid
        AcceptanceEligible = $valid -and -not $CorePhases
        Reasons = @($issues | Select-Object -Unique)
        SimulationTicks = $ticks
        ObservedSeconds = $seconds
        SimulationRealtimeRatio = if ($seconds -gt 0) { $ticks / ($seconds * $rate) } else { 0.0 }
        MeetsSimulationRate = $Scenario -ne 'world-battle' -or $ticks -ge ([long][Math]::Floor($seconds * $rate) - 1)
        Mode = if ($CorePhases) { 'diagnostic' } else { 'timing' }
        FrameTimingMetric = $timing.Metric
    }
}
