# Opt-in OS observations, outside simulation and the ordinary FPS benchmark.
function Get-RangeOsProcessSnapshot {
    $readable = @{}
    $unreadable = 0
    foreach ($candidate in (Get-Process)) {
        try {
            $started = $candidate.StartTime.ToUniversalTime().ToString('o')
            $readable["$($candidate.Id)/$started"] = [pscustomobject]@{
                ProcessId = $candidate.Id; Name = $candidate.ProcessName
                StartedUtc = $started; CpuMs = $candidate.TotalProcessorTime.TotalMilliseconds
            }
        } catch { $unreadable++ }
        finally { $candidate.Dispose() }
    }
    [pscustomobject]@{ Readable = $readable; Unreadable = $unreadable }
}

function Initialize-RangeOsPressure {
    if ('RangeOsPressure' -as [type]) { return }
    Add-Type @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class RangeOsPressure {
    [StructLayout(LayoutKind.Sequential)] struct FileTime {
        public uint Low, High;
        public ulong Value { get { return ((ulong)High << 32) | Low; } }
    }
    [StructLayout(LayoutKind.Sequential)] struct Processor {
        public ushort Group; public byte Number, Reserved;
    }
    [StructLayout(LayoutKind.Sequential)] struct ProcessMemory {
        public uint Size, PageFaults;
        public UIntPtr PeakWorkingSet, WorkingSet, PeakPagedPool, PagedPool;
        public UIntPtr PeakNonPagedPool, NonPagedPool, Pagefile, PeakPagefile, Private;
    }
    [StructLayout(LayoutKind.Sequential)] struct Performance {
        public uint Size;
        public UIntPtr Commit, CommitLimit, CommitPeak, Physical, Available, Cache;
        public UIntPtr Kernel, KernelPaged, KernelNonPaged, PageSize;
        public uint Handles, Processes, Threads;
    }
    [DllImport("kernel32.dll")] static extern bool GetSystemTimes(out FileTime idle, out FileTime kernel, out FileTime user);
    [DllImport("kernel32.dll")] static extern void GetCurrentProcessorNumberEx(out Processor processor);
    [DllImport("kernel32.dll")] static extern uint GetActiveProcessorCount(ushort group);
    [DllImport("psapi.dll")] static extern bool GetProcessMemoryInfo(IntPtr process, ref ProcessMemory memory, uint size);
    [DllImport("psapi.dll")] static extern bool GetPerformanceInfo(ref Performance info, uint size);
    public sealed class Sample {
        public ulong? SystemIdle100ns, SystemKernel100ns, SystemUser100ns;
        public ushort? SystemCpuGroup;
        public uint? SystemCpuProcessors, MachineProcessors, PageFaults;
        public ulong? PhysicalAvailableBytes, SystemCommitBytes, SystemCommitLimitBytes;
        public double ReadUs;
    }
    public static Sample Read(IntPtr process) {
        long start = Stopwatch.GetTimestamp();
        var result = new Sample();
        Processor before, after;
        FileTime idle, kernel, user;
        GetCurrentProcessorNumberEx(out before);
        bool cpu = GetSystemTimes(out idle, out kernel, out user);
        GetCurrentProcessorNumberEx(out after);
        if (cpu && before.Group == after.Group) {
            result.SystemIdle100ns = idle.Value;
            result.SystemKernel100ns = kernel.Value;
            result.SystemUser100ns = user.Value;
            result.SystemCpuGroup = before.Group;
            result.SystemCpuProcessors = GetActiveProcessorCount(before.Group);
            result.MachineProcessors = GetActiveProcessorCount(0xffff);
        }
        var memory = new ProcessMemory();
        memory.Size = (uint)Marshal.SizeOf(typeof(ProcessMemory));
        if (GetProcessMemoryInfo(process, ref memory, memory.Size)) result.PageFaults = memory.PageFaults;
        var perf = new Performance();
        perf.Size = (uint)Marshal.SizeOf(typeof(Performance));
        if (GetPerformanceInfo(ref perf, perf.Size)) {
            ulong page = perf.PageSize.ToUInt64();
            result.PhysicalAvailableBytes = perf.Available.ToUInt64() * page;
            result.SystemCommitBytes = perf.Commit.ToUInt64() * page;
            result.SystemCommitLimitBytes = perf.CommitLimit.ToUInt64() * page;
        }
        result.ReadUs = (Stopwatch.GetTimestamp() - start) * (1000000.0 / Stopwatch.Frequency);
        return result;
    }
}
'@
}
