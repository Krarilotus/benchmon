# Stream CPU/RAM/GPU and per-volume disk readings. No files or credentials are read.
$ErrorActionPreference = 'SilentlyContinue'
$smi = Get-Command nvidia-smi
$volumes = @()
$capacityAt = [DateTime]::MinValue
while ($true) {
    # Capacity changes slowly; performance counters are sampled on every iteration.
    if ([DateTime]::UtcNow -ge $capacityAt.AddSeconds(30)) {
        $volumes = @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' |
            Where-Object { $_.Size -gt 0 })
        $capacityAt = [DateTime]::UtcNow
    }
    $performance = @(Get-CimInstance Win32_PerfFormattedData_PerfDisk_LogicalDisk)
    $disks = @($volumes | ForEach-Object {
        $volume = $_
        $io = $performance | Where-Object { $_.Name -eq $volume.DeviceID } | Select-Object -First 1
        [ordered]@{
            name = $volume.DeviceID
            used_bytes = [double]($volume.Size - $volume.FreeSpace)
            total_bytes = [double]$volume.Size
            read_percent = $(if ($null -ne $io) { [double]$io.PercentDiskReadTime } else { $null })
            write_percent = $(if ($null -ne $io) { [double]$io.PercentDiskWriteTime } else { $null })
        }
    })
    $diskJson = ConvertTo-Json -InputObject $disks -Compress -Depth 3
    $cpu = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
    $os = Get-CimInstance Win32_OperatingSystem
    $gpu = ''
    if ($smi) {
        $gpu = (& nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits |
            Select-Object -First 1)
    }
    try {
        [Console]::Out.WriteLine("S $cpu $($os.TotalVisibleMemorySize - $os.FreePhysicalMemory) $($os.TotalVisibleMemorySize) $gpu | D $diskJson")
        [Console]::Out.Flush()
    } catch { exit }
    Start-Sleep -Seconds 2
}
