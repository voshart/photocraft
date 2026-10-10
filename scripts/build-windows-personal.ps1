# Build only the native Windows app and copy it to dist/windows.
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
    throw 'Run this script on Windows with the Rust MSVC toolchain.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$savedDebug = $env:CARGO_PROFILE_DEV_DEBUG
$savedIncremental = $env:CARGO_INCREMENTAL
Push-Location -LiteralPath $repoRoot
try {
    # Retain useful backtraces without large full-symbol PDBs or incremental caches.
    $env:CARGO_PROFILE_DEV_DEBUG = 'line-tables-only'
    $env:CARGO_INCREMENTAL = '0'
    & cargo build --locked --jobs 2 -p photocraft
    if ($LASTEXITCODE -ne 0) {
        throw "Windows build failed with exit code $LASTEXITCODE."
    }
    & (Join-Path $PSScriptRoot 'setup-face-model.ps1')
    $outputDir = Join-Path $repoRoot 'dist/windows'
    New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
    Copy-Item -LiteralPath (Join-Path $repoRoot 'target/debug/photocraft.exe') -Destination $outputDir -Force
    Get-ChildItem -LiteralPath (Join-Path $repoRoot 'target/debug') -Filter '*.dll' -File |
        Copy-Item -Destination $outputDir -Force
    Write-Output "Ready: $outputDir\photocraft.exe"
}
finally {
    $env:CARGO_PROFILE_DEV_DEBUG = $savedDebug
    $env:CARGO_INCREMENTAL = $savedIncremental
    Pop-Location
}
