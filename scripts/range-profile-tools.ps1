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

function Get-RangeValidity {
    param([object[]]$Rows, [string[]]$Reasons = @(), [string]$Scenario, [string]$Log,
        [switch]$CorePhases)
    $issues = [System.Collections.Generic.List[string]]::new()
    foreach ($reason in $Reasons) { $issues.Add($reason) }
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
    }
    if ($ticks -eq 0) { $issues.Add('simulation_did_not_advance') }
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
    }
}
