# Build with remapped local paths and package only explicitly approved public files.
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
    $flags = @('--remap-path-prefix=' + $projectRoot + '=benchmon')
    if ($env:USERPROFILE) { $flags += '--remap-path-prefix=' + $env:USERPROFILE + '=build-home' }
    if ($previousFlags) { $flags = @($previousFlags.Split([char]31)) + $flags }
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $versionLine = Get-Content -LiteralPath Cargo.toml | Where-Object { $_ -match '^version\s*=' } | Select-Object -First 1
    $version = [regex]::Match($versionLine, '"([^"]+)"').Groups[1].Value
    if (-not $version) { throw 'Cannot read package version' }
    $dist = Join-Path $projectRoot 'dist'
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $archive = Join-Path $dist "benchmon-$version-windows-x64.zip"
    # No directory traversal or configuration auto-discovery: hosts.txt is never packaged.
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $publicFiles = @{
        'benchmon.exe' = 'target/release/benchmon.exe'
        'hosts.example.txt' = 'hosts.example.txt'
        'README.md' = 'README.md'
        'LICENSE' = 'LICENSE'
        'assets/screenshot.jpg' = 'assets/screenshot.jpg'
    }
    $zip = [IO.Compression.ZipArchive]::new([IO.File]::Create($archive), [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in $publicFiles.GetEnumerator()) {
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $entry.Value, $entry.Key) | Out-Null
        }
    } finally { $zip.Dispose() }
    Get-FileHash -LiteralPath $archive -Algorithm SHA256 | Format-List
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    Pop-Location
}
