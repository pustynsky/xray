[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$repoRoot = Split-Path -Parent $PSScriptRoot
$stageRoot = Join-Path $repoRoot 'target\install-xray'
$source = Join-Path $stageRoot 'bin\xray.exe'
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    throw 'LOCALAPPDATA is not set.'
}
$destinationDir = Join-Path $env:LOCALAPPDATA 'xray'
$destination = Join-Path $destinationDir 'xray.exe'
$tempDestination = "$destination.new.exe"

# Cargo discovers repository-local configuration from the current directory.
Push-Location $repoRoot
try {
    & cargo install --path $repoRoot --force --root $stageRoot --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo install failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "Staged binary missing: $source"
}

New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$deploymentError = $null
for ($attempt = 1; $attempt -le 5; $attempt++) {
    $processes = @(Get-Process -Name xray -ErrorAction SilentlyContinue)
    if ($processes.Count -gt 0) {
        $processes | Stop-Process -Force -ErrorAction Stop
        $processes | Wait-Process -ErrorAction SilentlyContinue
    }

    try {
        [System.IO.File]::Copy($source, $tempDestination, $true)
        $tempHash = (Get-FileHash -LiteralPath $tempDestination -Algorithm SHA256).Hash
        if ($sourceHash -ne $tempHash) {
            throw 'Staged binary hash mismatch.'
        }
        $null = & $tempDestination --version
        if ($LASTEXITCODE -ne 0) {
            throw "Staged binary failed version check with exit code $LASTEXITCODE."
        }
        Move-Item -LiteralPath $tempDestination -Destination $destination -Force
        $deploymentError = $null
        break
    }
    catch {
        $deploymentError = $_
    }
    finally {
        if (Test-Path -LiteralPath $tempDestination) {
            Remove-Item -LiteralPath $tempDestination -Force -ErrorAction SilentlyContinue
        }
    }
}
if ($null -ne $deploymentError) {
    throw "Could not deploy xray after 5 attempts: $($deploymentError.Exception.Message)"
}

$destinationHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
if ($sourceHash -ne $destinationHash) {
    throw 'Installed binary hash mismatch.'
}

$version = (& $destination --version) -join [Environment]::NewLine
if ($LASTEXITCODE -ne 0) {
    throw "Installed binary failed version check with exit code $LASTEXITCODE."
}
$installed = Get-Item -LiteralPath $destination
[pscustomobject]@{
    Path = $installed.FullName
    Version = $version
    Sha256 = $destinationHash
    Length = $installed.Length
    LastWriteTime = $installed.LastWriteTime
}
