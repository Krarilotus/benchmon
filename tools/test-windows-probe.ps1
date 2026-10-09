# Exercise real process lifetime, including an SSH owner disappearing while cmd lives on.
# Run with Windows PowerShell 5.1; no SSH server, keys, or remote machine is required.
#requires -Version 5.1
param([string]$Collector = (Join-Path (Split-Path -Parent $PSScriptRoot) 'src\collectors\windows.ps1'))
$ErrorActionPreference = 'Stop'
$taskProbePath = [IO.Path]::GetFullPath($Collector)
$taskScratchRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$taskScratch = Join-Path $taskScratchRoot ('benchmon-probe-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskScratch | Out-Null
try {
    $taskFakeSshd = Join-Path $taskScratch 'sshd.exe'
    Add-Type -OutputAssembly $taskFakeSshd -OutputType ConsoleApplication -TypeDefinition @'
using System.Diagnostics;
class BenchmonTestSshOwner {
    static int Main(string[] args) {
        // Keep stdout writable after this owner dies. A broken pipe must not be
        // the reason the collector exits in the SSH-ancestor regression case.
        var command = " /d /c powershell.exe -NoProfile -NonInteractive -EncodedCommand "
            + args[0] + " > \"" + args[1] + "\"";
        using (var child = new Process()) {
            child.StartInfo = new ProcessStartInfo("cmd.exe", command) {
                UseShellExecute = false, CreateNoWindow = true
            };
            child.Start();
            child.WaitForExit();
            return child.ExitCode;
        }
    }
}
'@
    $taskBootstrap = "& ([ScriptBlock]::Create([IO.File]::ReadAllText('" + $taskProbePath.Replace("'", "''") + "')))"
    $taskEncoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($taskBootstrap))
    foreach ($taskMode in @('cmd parent', 'SSH ancestor')) {
        $taskLauncher = [Diagnostics.Process]::new()
        $taskCollector = $null
        $taskCmd = $null
        $taskReadingPath = Join-Path $taskScratch 'reading.txt'
        try {
            $taskLauncher.StartInfo.FileName = if ($taskMode -eq 'cmd parent') { 'cmd.exe' } else { $taskFakeSshd }
            $taskLauncher.StartInfo.Arguments = if ($taskMode -eq 'cmd parent') {
                '/d /c powershell.exe -NoProfile -NonInteractive -EncodedCommand ' + $taskEncoded
            } else { $taskEncoded + ' "' + $taskReadingPath + '"' }
            $taskLauncher.StartInfo.UseShellExecute = $false
            $taskLauncher.StartInfo.CreateNoWindow = $true
            $taskLauncher.StartInfo.RedirectStandardOutput = $true
            $taskLauncher.StartInfo.RedirectStandardError = $true
            [void]$taskLauncher.Start()
            if ($taskMode -eq 'cmd parent') {
                $taskReading = $taskLauncher.StandardOutput.ReadLineAsync()
                if (!$taskReading.Wait(15000)) { throw "No Windows reading for $taskMode" }
                $taskLine = $taskReading.Result
            } else {
                $taskLine = $null
                $taskDeadline = [DateTime]::UtcNow.AddSeconds(15)
                while (!$taskLine -and [DateTime]::UtcNow -lt $taskDeadline) {
                    if (Test-Path -LiteralPath $taskReadingPath) { $taskLine = Get-Content -LiteralPath $taskReadingPath -TotalCount 1 }
                    if (!$taskLine) { Start-Sleep -Milliseconds 200 }
                }
            }
            if (!$taskLine -or $taskLine -notmatch '^S ([0-9.,]+) ([0-9]+) ([0-9]+) .* \| D (.+)$') {
                throw "Invalid Windows reading for $taskMode"
            }
            $taskCpu = [double]::Parse($Matches[1].Replace(',', '.'), [Globalization.CultureInfo]::InvariantCulture)
            $taskUsed = [double]$Matches[2]
            $taskTotal = [double]$Matches[3]
            $taskDisks = $Matches[4] | ConvertFrom-Json
            if ($taskCpu -lt 0 -or $taskCpu -gt 100 -or $taskTotal -le 0 -or $taskUsed -lt 0 -or $taskUsed -gt $taskTotal -or @($taskDisks).Count -eq 0) {
                throw "Out-of-range Windows counters for $taskMode"
            }
            $taskParentPid = $taskLauncher.Id
            if ($taskMode -eq 'SSH ancestor') {
                $taskCmdInfo = Get-CimInstance Win32_Process -Filter "ParentProcessId=$taskParentPid AND Name='cmd.exe'"
                $taskParentPid = $taskCmdInfo.ProcessId
                $taskCmd = [Diagnostics.Process]::GetProcessById($taskParentPid)
            }
            $taskCollectorInfo = Get-CimInstance Win32_Process -Filter "ParentProcessId=$taskParentPid AND Name='powershell.exe'"
            if (!$taskCollectorInfo) { throw 'Collector process was not found' }
            $taskCollector = [Diagnostics.Process]::GetProcessById($taskCollectorInfo.ProcessId)
            $null = $taskCollector.Handle
            $taskLauncher.Kill()
            $taskLauncher.WaitForExit()
            if (!$taskCollector.WaitForExit(7000)) { throw "Collector survived its $taskMode" }
            if ($taskCmd -and !$taskCmd.WaitForExit(3000)) { throw 'SSH command shell survived its collector' }
            Write-Output "PASS: valid counters and collector cleanup after $taskMode exits"
        } finally {
            foreach ($taskProcess in @($taskCollector, $taskCmd, $taskLauncher)) {
                if ($taskProcess) {
                    if (!$taskProcess.HasExited) { $taskProcess.Kill(); $taskProcess.WaitForExit() }
                    $taskProcess.Dispose()
                }
            }
        }
    }
} finally {
    $taskResolvedScratch = [IO.Path]::GetFullPath($taskScratch)
    if (!$taskResolvedScratch.StartsWith($taskScratchRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($taskResolvedScratch) -notmatch '^benchmon-probe-test-[a-f0-9]{32}$') {
        throw 'Refusing to remove an unexpected test directory'
    }
    Remove-Item -LiteralPath $taskResolvedScratch -Recurse -Force
}
