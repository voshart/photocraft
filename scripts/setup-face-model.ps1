# Google's pinned Face Landmarker model and Windows x64 runtime. No Python installation required.
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT' -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'This personal setup script supports Windows x64.'
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$modelDir = Join-Path $repoRoot 'dist/windows/models'
$runtimeDir = Join-Path $env:LOCALAPPDATA 'mediapipe-rs/0.10.35'
New-Item -ItemType Directory -Force $modelDir, $runtimeDir | Out-Null

function Get-VerifiedAsset($url, $destination, $sha256) {
    if ((Test-Path -LiteralPath $destination) -and (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -eq $sha256) { return }
    $download = "$destination.download"
    Invoke-WebRequest -Uri $url -UserAgent 'Photocraft-dev' -OutFile $download
    if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne $sha256) { throw "Download checksum mismatch: $destination" }
    Move-Item -LiteralPath $download -Destination $destination -Force
}

Get-VerifiedAsset 'https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task' `
    (Join-Path $modelDir 'face_landmarker.task') '64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff'
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE-APACHE') -Destination (Join-Path $modelDir 'LICENSE-APACHE.txt') -Force
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE-APACHE') -Destination (Join-Path $runtimeDir 'LICENSE-APACHE.txt') -Force

$wheel = Join-Path $modelDir 'mediapipe-runtime.whl'
$runtime = Join-Path $runtimeDir 'libmediapipe.dll'
if ((Test-Path -LiteralPath $runtime) -and
    (Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash -eq 'aa8e6c1b618c30cd3a6ad584dee1b2f2c99c3f3025d683bada36e1566d9092b7') {
    Write-Output "Face model and Windows runtime are already ready."
    return
}
Get-VerifiedAsset 'https://files.pythonhosted.org/packages/5b/f6/763477e9aeed98accc984ed6ee3f11a21a0c5fd1d1c6586b8d07067748ff/mediapipe-0.10.35-py3-none-win_amd64.whl' `
    $wheel 'b08f001cf3c3cd0d88d9ed68f3368dc8a4913f568281a93117f083115aa672ba'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($wheel)
try {
    $entry = $archive.Entries | Where-Object { $_.Name -eq 'libmediapipe.dll' } | Select-Object -First 1
    if ($null -eq $entry) { throw 'Official MediaPipe package does not contain the Windows runtime.' }
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $runtime, $true)
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE-APACHE') -Destination (Join-Path $runtimeDir 'LICENSE-APACHE.txt') -Force
} finally { $archive.Dispose() }
Remove-Item -LiteralPath $wheel
Write-Output "Face model ready: $modelDir"
Write-Output "MediaPipe runtime ready: $runtimeDir"
