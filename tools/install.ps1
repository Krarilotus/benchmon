# Update one stable deployment directory, preserving hosts.txt and any existing taskbar pin.
param([string]$Destination)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$locationFile = Join-Path $projectRoot 'install-location.local'
if (-not $Destination) {
    $Destination = if (Test-Path -LiteralPath $locationFile) {
        (Get-Content -LiteralPath $locationFile -Raw).Trim()
    } else { $projectRoot }
}
$Destination = [IO.Path]::GetFullPath($Destination)
$source = Join-Path $projectRoot 'target\release\benchmon.exe'
$target = Join-Path $Destination 'benchmon.exe'
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw 'Build or package benchmon first.' }
if ($source -eq $target) { throw 'Choose a deployment directory outside target\release.' }
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
$running = @(Get-CimInstance Win32_Process -Filter "Name='benchmon.exe'" |
    Where-Object { $_.ExecutablePath -eq $target })
foreach ($process in $running) {
    Stop-Process -Id $process.ProcessId -ErrorAction Stop
    Wait-Process -Id $process.ProcessId -ErrorAction SilentlyContinue
}
# Windows can briefly retain the executable mapping after its process exits.
for ($attempt = 0; $attempt -lt 20; $attempt++) {
    try {
        Copy-Item -LiteralPath $source -Destination $target -Force
        break
    } catch {
        if ($attempt -eq 19) { throw }
        Start-Sleep -Milliseconds 250
    }
}
foreach ($name in @('hosts.example.txt', 'README.md', 'LICENSE', 'assets/screenshot.jpg')) {
    $from = Join-Path $projectRoot $name
    $to = Join-Path $Destination $name
    if ($from -ne $to) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $to) -Force | Out-Null
        Copy-Item -LiteralPath $from -Destination $to -Force
    }
}
# Write the machine-specific destination only to an ignored local file.
Set-Content -LiteralPath $locationFile -Value $Destination -Encoding UTF8
$pinFolder = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar'
if (Test-Path -LiteralPath $pinFolder) {
    $shell = New-Object -ComObject WScript.Shell
    Get-ChildItem -LiteralPath $pinFolder -Filter '*.lnk' | ForEach-Object {
        $shortcut = $shell.CreateShortcut($_.FullName)
        if ($shortcut.TargetPath -eq $target) {
            $shortcut.IconLocation = "$target,0"
            $shortcut.WorkingDirectory = $Destination
            $shortcut.Save()
        }
    }
}
Start-Process -FilePath $target -WorkingDirectory $Destination
Write-Output "Running: $target"
