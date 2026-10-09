# Stream CPU/RAM/GPU and per-volume disk readings. No files or credentials are read.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# These Win32 calls read counters directly instead of repeatedly enumerating WMI
# hardware/OS providers. GetSystemTimes' kernel counter includes idle time.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class BenchmonNative {
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetSystemTimes(out ulong idle, out ulong kernel, out ulong user);
    [StructLayout(LayoutKind.Sequential)]
    struct MemoryStatus {
        public uint Length, Load;
        public ulong TotalPhys, AvailPhys, TotalPage, AvailPage;
        public ulong TotalVirtual, AvailVirtual, AvailExtended;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GlobalMemoryStatusEx(ref MemoryStatus status);
    public static ulong[] Cpu() {
        ulong idle, kernel, user;
        if (!GetSystemTimes(out idle, out kernel, out user))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return new ulong[] {idle, kernel, user};
    }
    public static ulong[] Memory() {
        var status = new MemoryStatus();
        status.Length = (uint)Marshal.SizeOf(typeof(MemoryStatus));
        if (!GlobalMemoryStatusEx(ref status))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return new ulong[] {status.TotalPhys - status.AvailPhys, status.TotalPhys};
    }
}
'@
# A Windows SSH disconnect can kill sshd while leaving cmd and PowerShell alive.
# Hold process handles once, so PID reuse cannot turn a dead owner into a live one.
$owners = @()
try {
    $parentPid = (Get-CimInstance Win32_Process -Filter "ProcessId=$PID" -Property ParentProcessId).ParentProcessId
    $parent = [Diagnostics.Process]::GetProcessById($parentPid)
    $null = $parent.Handle
    $owners += $parent
    if ($parent.ProcessName -eq 'cmd') {
        $ancestorPid = (Get-CimInstance Win32_Process -Filter "ProcessId=$parentPid" -Property ParentProcessId).ParentProcessId
        $ancestor = [Diagnostics.Process]::GetProcessById($ancestorPid)
        if ($ancestor.ProcessName -eq 'sshd') {
            $null = $ancestor.Handle
            $owners += $ancestor
        } else { $ancestor.Dispose() }
    }
} catch {
    # Do not leave an unowned remote collector running when its session is gone.
    if ($env:SSH_CONNECTION) { exit 1 }
}
$smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
$volumes = @()
$capacityAt = [DateTime]::MinValue
$previousCpu = [BenchmonNative]::Cpu()
while ($true) {
    Start-Sleep -Seconds 2
    foreach ($owner in $owners) { if ($owner.HasExited) { exit 0 } }
    # Capacity changes slowly; performance counters are sampled on every iteration.
    if ([DateTime]::UtcNow -ge $capacityAt.AddSeconds(30)) {
        $volumes = @([IO.DriveInfo]::GetDrives() | ForEach-Object {
            try {
                if ($_.DriveType -eq 'Fixed' -and $_.IsReady -and $_.TotalSize -gt 0) {
                    [pscustomobject]@{Name=$_.Name.TrimEnd('\');Total=$_.TotalSize;Used=$_.TotalSize-$_.TotalFreeSpace}
                }
            } catch { } # A locked or disappearing volume must not stop the probe.
        })
        $capacityAt = [DateTime]::UtcNow
    }
    $performance = @(Get-CimInstance Win32_PerfFormattedData_PerfDisk_LogicalDisk `
        -Property Name,PercentDiskReadTime,PercentDiskWriteTime -ErrorAction SilentlyContinue)
    $disks = @($volumes | ForEach-Object {
        $volume = $_
        $io = $performance | Where-Object { $_.Name -eq $volume.Name } | Select-Object -First 1
        [ordered]@{
            name = $volume.Name
            used_bytes = [double]$volume.Used
            total_bytes = [double]$volume.Total
            read_percent = $(if ($null -ne $io) { [double]$io.PercentDiskReadTime } else { $null })
            write_percent = $(if ($null -ne $io) { [double]$io.PercentDiskWriteTime } else { $null })
        }
    })
    $diskJson = ConvertTo-Json -InputObject $disks -Compress -Depth 3
    $currentCpu = [BenchmonNative]::Cpu()
    $totalDelta = ([double]$currentCpu[1] - $previousCpu[1]) + ([double]$currentCpu[2] - $previousCpu[2])
    $idleDelta = [double]$currentCpu[0] - $previousCpu[0]
    $cpu = [Math]::Round([Math]::Min(100, [Math]::Max(0, 100 * (1 - $idleDelta / [Math]::Max(1, $totalDelta)))), 1)
    $previousCpu = $currentCpu
    $memory = [BenchmonNative]::Memory()
    $gpu = ''
    if ($smi) {
        $gpu = (& $smi --query-gpu=utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits |
            Select-Object -First 1)
    }
    try {
        [Console]::Out.WriteLine("S $cpu $([Math]::Floor($memory[0] / 1024)) $([Math]::Floor($memory[1] / 1024)) $gpu | D $diskJson")
        [Console]::Out.Flush()
    } catch { exit }
}
