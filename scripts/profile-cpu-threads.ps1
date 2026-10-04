param(
    [Parameter(Mandatory = $true)][int]$TargetProcessId,
    [ValidateRange(1, 7200)][int]$Seconds = 40,
    [ValidateRange(100, 5000)][int]$IntervalMilliseconds = 1000,
    [Parameter(Mandatory = $true)][string]$Output,
    [switch]$ProcessOnly,
    [switch]$OsPressure,
    [string[]]$AdditionalProcessNames = @()
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/range-profile-tools.ps1"
if ($OsPressure) {
    . "$PSScriptRoot/range-os-pressure.ps1"
    Initialize-RangeOsPressure
}
# OS counters only: no instrumentation or clocks enter the simulation.
if (-not $ProcessOnly -and -not ('RangeThreadNames' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RangeThreadNames {
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr thread, out IntPtr text);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);
    public static string Read(uint id) {
        IntPtr thread = OpenThread(0x0800, false, id);
        if (thread == IntPtr.Zero) return "";
        IntPtr text = IntPtr.Zero;
        try {
            return GetThreadDescription(thread, out text) == 0 ? Marshal.PtrToStringUni(text) : "";
        } finally {
            if (text != IntPtr.Zero) LocalFree(text);
            CloseHandle(thread);
        }
    }
}
'@
}
$directory = [System.IO.Path]::GetFullPath($Output)
New-Item -ItemType Directory -Force -Path $directory | Out-Null
$target = Get-Process -Id $TargetProcessId
$pressureWarmup = if ($OsPressure) { [RangeOsPressure]::Read($target.Handle) } else { $null }
$previousOsProcesses = if ($OsPressure) { Get-RangeOsProcessSnapshot } else { $null }
$logical = [Environment]::ProcessorCount
$previous = @{}
$names = @{}
$created = @{}
$samples = [System.Collections.Generic.List[object]]::new()
$processSamples = [System.Collections.Generic.List[object]]::new()
$environmentSamples = [System.Collections.Generic.List[object]]::new()
$osProcessSamples = [System.Collections.Generic.List[object]]::new()
$baselineTime = Get-Date
$baselineCpuMs = $target.TotalProcessorTime.TotalMilliseconds
$processCpuMs = 0.0
$timer = [System.Diagnostics.Stopwatch]::StartNew()
$lastMs = 0.0
$samplerFailure = $null
$timedOut = $false
try {
    # Capture a baseline so the first interval excludes process startup CPU.
    foreach ($thread in $(if (-not $ProcessOnly) { $target.Threads } else { @() })) {
        try {
            $previous[$thread.Id] = $thread.TotalProcessorTime.TotalMilliseconds
            $created[$thread.Id] = $thread.StartTime.ToUniversalTime().ToString('o')
        } catch { }
    }
    while ($timer.Elapsed.TotalSeconds -lt $Seconds) {
        Start-Sleep -Milliseconds $IntervalMilliseconds
        $target.Refresh()
        if ($target.HasExited) { break }
        $nowMs = $timer.Elapsed.TotalMilliseconds
        $utc = (Get-Date).ToUniversalTime().ToString('o')
        foreach ($other in (Get-RangeCompetingProcesses -Exclude @($TargetProcessId, $PID) -AdditionalNames $AdditionalProcessNames)) {
            $environmentSamples.Add([pscustomobject]@{
                Seconds = [Math]::Round($nowMs / 1000.0, 3)
                Utc = $utc
                ProcessId = $other.ProcessId
                Name = $other.Name
                StartedUtc = $other.StartedUtc
                CpuMs = $other.CpuMs
            })
        }
        # The process counter also includes threads that exited between samples.
        $processCpuMs = [Math]::Max(0.0, $target.TotalProcessorTime.TotalMilliseconds - $baselineCpuMs)
        $pressure = if ($OsPressure) { [RangeOsPressure]::Read($target.Handle) } else { $null }
        $osProcesses = if ($OsPressure) { Get-RangeOsProcessSnapshot } else { $null }
        if ($OsPressure) {
            $deltas = @(
                foreach ($key in $osProcesses.Readable.Keys) {
                    $current = $osProcesses.Readable[$key]
                    $old = $previousOsProcesses.Readable[$key]
                    if ($null -eq $old -or $current.CpuMs -lt $old.CpuMs) { continue }
                    [pscustomobject]@{
                        Seconds = [Math]::Round($nowMs / 1000.0, 3); Utc = $utc
                        ProcessId = $current.ProcessId; Name = $current.Name
                        StartedUtc = $current.StartedUtc; CpuMs = $current.CpuMs - $old.CpuMs
                        IsTarget = $current.ProcessId -eq $TargetProcessId
                        IsSampler = $current.ProcessId -eq $PID
                    }
                }
            )
            foreach ($delta in ($deltas | Sort-Object @{Expression='CpuMs';Descending=$true},ProcessId | Select-Object -First 8)) {
                $osProcessSamples.Add($delta)
            }
            $previousOsProcesses = $osProcesses
        }
        $processSample = [pscustomobject]@{
            Seconds = [Math]::Round($nowMs / 1000.0, 3)
            Utc = $utc
            CpuMs = $processCpuMs
            PrivateBytes = $target.PrivateMemorySize64
            WorkingSetBytes = $target.WorkingSet64
            ThreadCount = $target.Threads.Count
            HandleCount = $target.HandleCount
            SystemIdle100ns = if ($pressure) { $pressure.SystemIdle100ns } else { $null }
            SystemKernel100ns = if ($pressure) { $pressure.SystemKernel100ns } else { $null }
            SystemUser100ns = if ($pressure) { $pressure.SystemUser100ns } else { $null }
            SystemCpuGroup = if ($pressure) { $pressure.SystemCpuGroup } else { $null }
            SystemCpuProcessors = if ($pressure) { $pressure.SystemCpuProcessors } else { $null }
            MachineProcessors = if ($pressure) { $pressure.MachineProcessors } else { $null }
            PageFaults = if ($pressure) { $pressure.PageFaults } else { $null }
            PhysicalAvailableBytes = if ($pressure) { $pressure.PhysicalAvailableBytes } else { $null }
            SystemCommitBytes = if ($pressure) { $pressure.SystemCommitBytes } else { $null }
            SystemCommitLimitBytes = if ($pressure) { $pressure.SystemCommitLimitBytes } else { $null }
            OsPressureReadUs = if ($pressure) { $pressure.ReadUs } else { $null }
            OsReadableProcesses = if ($osProcesses) { $osProcesses.Readable.Count } else { $null }
            OsUnreadableProcesses = if ($osProcesses) { $osProcesses.Unreadable } else { $null }
            OsSamplerWorkMs = $null
        }
        $processSamples.Add($processSample)
        $elapsedMs = $nowMs - $lastMs
        foreach ($thread in $(if (-not $ProcessOnly) { $target.Threads } else { @() })) {
            try {
                $id = $thread.Id
                $cpu = $thread.TotalProcessorTime.TotalMilliseconds
                $threadStarted = $thread.StartTime
                $currentCreation = $threadStarted.ToUniversalTime().ToString('o')
                if ($created.ContainsKey($id) -and $created[$id] -ne $currentCreation) {
                    $previous.Remove($id); $names.Remove($id)
                }
                $created[$id] = $currentCreation
                if (-not $names.ContainsKey($id)) {
                    # Some hosts expose no OS descriptions, including for
                    # named Rust threads. Empty names do not identify a pool.
                    try { $names[$id] = [RangeThreadNames]::Read($id) } catch { $names[$id] = '' }
                }
                $delta = if ($previous.ContainsKey($id)) {
                    [Math]::Max(0.0, $cpu - $previous[$id])
                } elseif ($threadStarted -ge $baselineTime) {
                    $cpu
                } else {
                    # A pre-existing thread inaccessible at baseline has no
                    # reliable delta on its first successful observation.
                    0.0
                }
                $previous[$id] = $cpu
                $state = ''
                $wait = ''
                try {
                    $state = $thread.ThreadState.ToString()
                    if ($state -eq 'Wait') { $wait = $thread.WaitReason.ToString() }
                } catch { }
                $samples.Add([pscustomobject]@{
                    Seconds = [Math]::Round($nowMs / 1000.0, 3)
                    IntervalMs = [Math]::Round($elapsedMs, 3)
                    ThreadId = $id
                    Name = $names[$id]
                    CreatedAt = $created[$id]
                    CpuMs = $delta
                    CorePercent = [Math]::Round(100.0 * $delta / $elapsedMs, 2)
                    State = $state
                    WaitReason = $wait
                })
            } catch { }
        }
        if ($OsPressure) { $processSample.OsSamplerWorkMs = $timer.Elapsed.TotalMilliseconds - $nowMs }
        $lastMs = $nowMs
    }
    $timedOut = -not $target.HasExited
} catch {
    # A process can exit between Refresh and a counter read. Preserve all
    # samples even for a real sampler failure, rather than losing the trace.
    $exited = $false
    try { $exited = $target.HasExited } catch { }
    if (-not $exited) { $samplerFailure = $_.Exception.Message }
} finally { $target.Dispose() }
$samples | Export-Csv -LiteralPath (Join-Path $directory 'cpu-threads.csv') -NoTypeInformation -Encoding UTF8
$processSamples | Export-Csv -LiteralPath (Join-Path $directory 'cpu-process-samples.csv') -NoTypeInformation -Encoding UTF8
if ($OsPressure) {
    if ($osProcessSamples.Count -gt 0) {
        $osProcessSamples | Export-Csv -LiteralPath (Join-Path $directory 'cpu-os-processes.csv') -NoTypeInformation -Encoding UTF8
    } else {
        '"Seconds","Utc","ProcessId","Name","StartedUtc","CpuMs","IsTarget","IsSampler"' | Set-Content -LiteralPath (Join-Path $directory 'cpu-os-processes.csv') -Encoding utf8
    }
}
if ($environmentSamples.Count -gt 0) {
    $environmentSamples | Export-Csv -LiteralPath (Join-Path $directory 'cpu-environment.csv') -NoTypeInformation -Encoding UTF8
} else {
    '"Seconds","Utc","ProcessId","Name","StartedUtc","CpuMs"' | Set-Content -LiteralPath (Join-Path $directory 'cpu-environment.csv') -Encoding UTF8
}
$totalCpu = ($samples | Measure-Object CpuMs -Sum).Sum
$summary = @($samples | Group-Object ThreadId,CreatedAt | ForEach-Object {
    $cpu = ($_.Group | Measure-Object CpuMs -Sum).Sum
    [pscustomobject]@{
        ThreadId = $_.Group[0].ThreadId
        Name = $_.Group[0].Name
        CreatedAt = $_.Group[0].CreatedAt
        CpuMs = [Math]::Round($cpu, 3)
        MeanCorePercent = if ($lastMs -gt 0) { [Math]::Round(100.0 * $cpu / $lastMs, 2) } else { 0 }
        CpuSharePercent = if ($totalCpu -gt 0) { [Math]::Round(100.0 * $cpu / $totalCpu, 2) } else { 0 }
    }
} | Sort-Object CpuMs -Descending)
$summary | Export-Csv -LiteralPath (Join-Path $directory 'cpu-thread-summary.csv') -NoTypeInformation -Encoding UTF8
[pscustomobject]@{
    ProcessId = $TargetProcessId
    LogicalProcessors = $logical
    ObservedSeconds = [Math]::Round($lastMs / 1000.0, 3)
    CpuSeconds = [Math]::Round($processCpuMs / 1000.0, 3)
    SampledThreadCpuSeconds = [Math]::Round($totalCpu / 1000.0, 3)
    MeanProcessPercent = if ($lastMs -gt 0) { [Math]::Round(100.0 * $processCpuMs / $lastMs / $logical, 2) } else { 0 }
} | Export-Csv -LiteralPath (Join-Path $directory 'cpu-process-summary.csv') -NoTypeInformation -Encoding UTF8
$summary | Select-Object -First 12 | Format-Table -AutoSize
[pscustomobject]@{
    ProcessId = $TargetProcessId
    ProcessOnly = [bool]$ProcessOnly
    OsPressure = [bool]$OsPressure
    OsPressureWarmupUs = if ($pressureWarmup) { $pressureWarmup.ReadUs } else { $null }
    IntervalMilliseconds = $IntervalMilliseconds
    StartedUtc = $baselineTime.ToUniversalTime().ToString('o')
    TimedOut = $timedOut
    Error = $samplerFailure
    ProcessSamples = $processSamples.Count
    ThreadSamples = $samples.Count
    CompetingProcessSamples = $environmentSamples.Count
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'sampler-status.json') -Encoding UTF8
if ($samplerFailure) { throw $samplerFailure }
