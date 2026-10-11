#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression tests for setup-xray.ps1 runtime dependency handling.

.DESCRIPTION
    Verifies that public downloads do not require gh, Git-managed repositories
    fail before mutation when git is unusable, and a non-Git folder can still
    be configured without installing Git filters. Local MCP and migration
    behavior is covered by test-mcp-setup.ps1.
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$Script:Passed = 0
$Script:Failed = 0

function Assert-True {
    param(
        [Parameter(Mandatory)] [bool]$Condition,
        [Parameter(Mandatory)] [string]$Label
    )

    if ($Condition) {
        $Script:Passed++
        Write-Host "PASS  $Label" -ForegroundColor Green
    }
    else {
        $Script:Failed++
        Write-Host "FAIL  $Label" -ForegroundColor Red
    }
}

function Assert-Equal {
    param(
        $Actual,
        $Expected,
        [Parameter(Mandatory)] [string]$Label
    )

    Assert-True -Condition ($Actual -eq $Expected) -Label $Label
    if ($Actual -ne $Expected) {
        Write-Host "      expected: $Expected" -ForegroundColor DarkGray
        Write-Host "      actual:   $Actual" -ForegroundColor DarkGray
    }
}

function Assert-BytesEqual {
    param(
        [Parameter(Mandatory)] [byte[]]$Actual,
        [Parameter(Mandatory)] [byte[]]$Expected,
        [Parameter(Mandatory)] [string]$Label
    )

    $actualBase64 = [Convert]::ToBase64String($Actual)
    $expectedBase64 = [Convert]::ToBase64String($Expected)
    Assert-Equal -Actual $actualBase64 -Expected $expectedBase64 -Label $Label
}

function Write-FakeWindowsExecutable {
    param([Parameter(Mandatory)] [string]$Path)

    $bytes = New-Object byte[] 68
    $bytes[0] = 0x4D
    $bytes[1] = 0x5A
    [BitConverter]::GetBytes(64).CopyTo($bytes, 0x3C)
    $bytes[64] = 0x50
    $bytes[65] = 0x45
    [IO.File]::WriteAllBytes($Path, $bytes)
}

function New-TempDirectory {
    $path = Join-Path ([IO.Path]::GetTempPath()) ("xray-dependency-test-{0}" -f [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    return $path
}

function New-InstallDirectory {
    param([Parameter(Mandatory)] [string]$Root)

    $path = Join-Path $Root ("install-{0}" -f [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    New-Item -ItemType File -Path (Join-Path $path 'xray.exe') -Force | Out-Null
    return $path
}

function New-GitFixture {
    param(
        [Parameter(Mandatory)] [string]$Root,
        [Parameter(Mandatory)] [string]$Name,
        [ValidateSet('None', 'VsCode', 'CopilotCli')] [string]$Config = 'None'
    )

    $repo = Join-Path $Root $Name
    New-Item -ItemType Directory -Path $repo -Force | Out-Null
    & $Script:GitExe -C $repo init --quiet
    & $Script:GitExe -C $repo config user.name 'Dependency Test'
    & $Script:GitExe -C $repo config user.email 'dependency-test@example.invalid'
    & $Script:GitExe -C $repo config core.autocrlf true
    & $Script:GitExe -C $repo config core.safecrlf true

    if ($Config -eq 'VsCode') {
        $configPath = Join-Path $repo '.vscode/mcp.json'
        New-Item -ItemType Directory -Path (Split-Path -Parent $configPath) -Force | Out-Null
        [IO.File]::WriteAllText($configPath, "{`r`n  `"servers`": {`r`n  }`r`n}`r`n", [Text.UTF8Encoding]::new($false))
        & $Script:GitExe -C $repo -c core.safecrlf=false add '.vscode/mcp.json'
    }
    elseif ($Config -eq 'CopilotCli') {
        $configPath = Join-Path $repo '.mcp.json'
        [IO.File]::WriteAllText($configPath, "{`r`n  `"mcpServers`": {`r`n  }`r`n}`r`n", [Text.UTF8Encoding]::new($false))
        & $Script:GitExe -C $repo -c core.safecrlf=false add '.mcp.json'
    }
    else {
        Set-Content -Path (Join-Path $repo 'source.ts') -Value 'export const value = 1;' -Encoding UTF8
        & $Script:GitExe -C $repo -c core.safecrlf=false add 'source.ts'
    }

    & $Script:GitExe -C $repo commit --quiet -m baseline
    if ($LASTEXITCODE -ne 0) { throw "Could not create fixture repository: $repo" }
    return $repo
}

function Invoke-SetupProcess {
    param(
        [Parameter(Mandatory)] [string]$ScriptPath,
        [Parameter(Mandatory)] [string]$RepoPath,
        [Parameter(Mandatory)] [string]$InstallDir,
        [Parameter(Mandatory)] [string[]]$Arguments,
        [string]$PathOverride
    )

    # Windows PowerShell 5.1 surfaces expected native stderr as ErrorRecord.
    $ErrorActionPreference = 'Continue'
    $oldPath = $env:PATH
    try {
        if ($PSBoundParameters.ContainsKey('PathOverride')) {
            $env:PATH = $PathOverride
        }
        $output = @(& $Script:PowerShellExe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $ScriptPath `
                -RepoPath $RepoPath -InstallDir $InstallDir @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        $env:PATH = $oldPath
    }

    return [PSCustomObject]@{
        ExitCode = $exitCode
        Output   = $output
    }
}

$scriptPath = Join-Path $PSScriptRoot 'setup-xray.ps1'
if (-not (Test-Path $scriptPath)) { throw "setup-xray.ps1 not found: $scriptPath" }
$Script:GitExe = (Get-Command git -CommandType Application -ErrorAction Stop).Source
$Script:PowerShellExe = (Get-Process -Id $PID).Path
$minimalPath = $PSHOME
$root = New-TempDirectory
$oldGlobalConfig = $env:GIT_CONFIG_GLOBAL
$oldNoSystemConfig = $env:GIT_CONFIG_NOSYSTEM

try {
    $isolatedGlobalConfig = Join-Path $root 'global.gitconfig'
    New-Item -ItemType File -Path $isolatedGlobalConfig -Force | Out-Null
    $env:GIT_CONFIG_GLOBAL = $isolatedGlobalConfig
    $env:GIT_CONFIG_NOSYSTEM = '1'

    # Load the pure download helpers without executing setup's imperative body.
    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
    if (@($parseErrors).Count -gt 0) { throw "Parse errors in setup-xray.ps1: $($parseErrors -join '; ')" }
    foreach ($functionName in @('Test-WindowsExecutable', 'Save-XrayReleaseAsset')) {
        $functionAst = $ast.Find({
                param($node)
                $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
            }, $true)
        if (-not $functionAst) { throw "$functionName not found in setup-xray.ps1" }
        . ([scriptblock]::Create($functionAst.Extent.Text))
    }

    $script:DownloadMode = 'valid'
    $script:DownloadUri = $null
    function Invoke-WebRequest {
        [CmdletBinding()]
        param(
            [Parameter(Mandatory)]$Uri,
            [Parameter(Mandatory)] [string]$OutFile,
            [switch]$UseBasicParsing
        )

        $script:DownloadUri = "$Uri"
        if ($script:DownloadMode -eq 'throw') { throw 'simulated direct download failure' }
        if ($script:DownloadMode -eq 'invalid') {
            [IO.File]::WriteAllText($OutFile, '<html>not a binary</html>', [Text.UTF8Encoding]::new($false))
            return
        }
        Write-FakeWindowsExecutable -Path $OutFile
    }

    $downloadDir = Join-Path $root 'download-helper'
    New-Item -ItemType Directory -Path $downloadDir -Force | Out-Null
    $downloadPath = Join-Path $downloadDir 'xray.exe'
    $oldPath = $env:PATH
    try {
        $env:PATH = $minimalPath
        Save-XrayReleaseAsset -GithubRepo 'owner/repo' -Destination $downloadPath
    }
    finally { $env:PATH = $oldPath }
    Assert-Equal -Actual $script:DownloadUri -Expected 'https://github.com/owner/repo/releases/latest/download/xray.exe' -Label 'T1 direct download URL'
    Assert-True -Condition (Test-WindowsExecutable -Path $downloadPath) -Label 'T1 direct download accepts PE payload'

    $script:DownloadMode = 'invalid'
    $invalidMessage = $null
    try { Save-XrayReleaseAsset -GithubRepo 'owner/repo' -Destination $downloadPath }
    catch { $invalidMessage = $_.Exception.Message }
    Assert-True -Condition ($invalidMessage -match 'not a valid Windows executable') -Label 'T2 invalid payload is rejected'

    $script:DownloadMode = 'throw'
    $missingGhMessage = $null
    try {
        $env:PATH = $minimalPath
        Save-XrayReleaseAsset -GithubRepo 'owner/repo' -Destination $downloadPath
    }
    catch { $missingGhMessage = $_.Exception.Message }
    finally { $env:PATH = $oldPath }
    Assert-True -Condition ($missingGhMessage -match 'Download failed from https://github.com/owner/repo') -Label 'T3 missing gh reports direct failure'

    $invalidRepoMessage = $null
    try { Save-XrayReleaseAsset -GithubRepo '../invalid' -Destination $downloadPath }
    catch { $invalidRepoMessage = $_.Exception.Message }
    Assert-True -Condition ($invalidRepoMessage -match 'Invalid GitHub repository') -Label 'T3b malformed GitHub repo is rejected early'

    $fakeBin = Join-Path $root 'fake-bin'
    New-Item -ItemType Directory -Path $fakeBin -Force | Out-Null
    $fakeGh = @'
@echo off
set "TARGET="
:next
if "%~1"=="" goto done
if /I "%~1"=="--dir" (
  set "TARGET=%~2"
  shift
)
shift
goto next
:done
if not defined TARGET exit /b 2
copy /b "%TARGET%\..\fake-xray.exe" "%TARGET%\xray.exe" >nul
exit /b 0
'@
    Set-Content -Path (Join-Path $fakeBin 'gh.cmd') -Value $fakeGh -Encoding ASCII
    Write-FakeWindowsExecutable -Path (Join-Path $root 'fake-xray.exe')
    try {
        $env:PATH = "$fakeBin;$minimalPath"
        Save-XrayReleaseAsset -GithubRepo 'owner/private-repo' -Destination $downloadPath -WarningAction SilentlyContinue
    }
    finally { $env:PATH = $oldPath }
    Assert-True -Condition (Test-WindowsExecutable -Path $downloadPath) -Label 'T4 gh fallback accepts PE payload'

    # A true non-Git folder remains supported without git.
    $plainRepo = Join-Path $root 'plain-visible'
    New-Item -ItemType Directory -Path $plainRepo -Force | Out-Null
    $plainInstall = New-InstallDirectory -Root $root
    $plainResult = Invoke-SetupProcess -ScriptPath $scriptPath -RepoPath $plainRepo -InstallDir $plainInstall `
        -Arguments @('-SkipDownload', '-Extensions', 'ts', '-EnableVSCode', '-GitVisibility', 'Visible', '-Force') `
        -PathOverride $minimalPath
    Assert-Equal -Actual $plainResult.ExitCode -Expected 0 -Label 'T5 non-Git setup succeeds without git'
    Assert-True -Condition (Test-Path (Join-Path $plainRepo '.vscode/mcp.json')) -Label 'T5 non-Git setup writes config'

    $hiddenRepo = Join-Path $root 'plain-hidden'
    New-Item -ItemType Directory -Path $hiddenRepo -Force | Out-Null
    $hiddenInstall = New-InstallDirectory -Root $root
    $hiddenResult = Invoke-SetupProcess -ScriptPath $scriptPath -RepoPath $hiddenRepo -InstallDir $hiddenInstall `
        -Arguments @('-SkipDownload', '-Extensions', 'ts', '-EnableVSCode', '-GitVisibility', 'Hidden', '-Force') `
        -PathOverride $minimalPath
    Assert-Equal -Actual $hiddenResult.ExitCode -Expected 0 -Label 'T6 deprecated Hidden uses the same local setup'
    Assert-True -Condition (Test-Path (Join-Path $hiddenRepo '.vscode/mcp.json')) -Label 'T6 deprecated Hidden does not require filter runtime'

    $managedRepo = New-GitFixture -Root $root -Name 'managed-no-git'
    $managedInstall = New-InstallDirectory -Root $root
    $managedResult = Invoke-SetupProcess -ScriptPath $scriptPath -RepoPath $managedRepo -InstallDir $managedInstall `
        -Arguments @('-SkipDownload', '-Extensions', 'ts', '-EnableVSCode', '-GitVisibility', 'Visible', '-Force') `
        -PathOverride $minimalPath
    Assert-True -Condition ($managedResult.ExitCode -ne 0) -Label 'T7 Git metadata without git fails'
    Assert-True -Condition (-not (Test-Path (Join-Path $managedRepo '.vscode/mcp.json'))) -Label 'T7 Git metadata failure writes nothing'

    $shimDir = Join-Path $root 'broken-git-shim'
    New-Item -ItemType Directory -Path $shimDir -Force | Out-Null
    Set-Content -Path (Join-Path $shimDir 'git.cmd') -Value '@exit /b 128' -Encoding ASCII
    $brokenResult = Invoke-SetupProcess -ScriptPath $scriptPath -RepoPath $managedRepo -InstallDir $managedInstall `
        -Arguments @('-SkipDownload', '-Extensions', 'ts', '-EnableVSCode', '-GitVisibility', 'Visible', '-Force') `
        -PathOverride "$shimDir;$minimalPath"
    Assert-True -Condition ($brokenResult.ExitCode -ne 0) -Label 'T8 broken git fails closed'

    $restoreRepo = New-GitFixture -Root $root -Name 'restore-no-git' -Config 'VsCode'
    $restoreInstall = New-InstallDirectory -Root $root
    $restoreConfig = Join-Path $restoreRepo '.vscode/mcp.json'
    Copy-Item -LiteralPath $restoreConfig -Destination "$restoreConfig.bak" -Force
    [IO.File]::WriteAllText($restoreConfig, "{`n  `"servers`": {`n    `"broken`": {}`n  }`n}`n", [Text.UTF8Encoding]::new($false))
    $restoreExpected = [IO.File]::ReadAllBytes($restoreConfig)
    $restoreResult = Invoke-SetupProcess -ScriptPath $scriptPath -RepoPath $restoreRepo -InstallDir $restoreInstall `
        -Arguments @('-Restore') -PathOverride $minimalPath
    Assert-True -Condition ($restoreResult.ExitCode -ne 0) -Label 'T8b restore refuses uninspectable Git metadata'
    Assert-BytesEqual -Actual ([IO.File]::ReadAllBytes($restoreConfig)) -Expected $restoreExpected -Label 'T8b unsafe restore preserves current bytes'
    Assert-True -Condition (Test-Path "$restoreConfig.bak") -Label 'T8b unsafe restore preserves backup'



}
finally {
    $env:GIT_CONFIG_GLOBAL = $oldGlobalConfig
    $env:GIT_CONFIG_NOSYSTEM = $oldNoSystemConfig
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

$expectedAssertions = 16
if (($Script:Passed + $Script:Failed) -ne $expectedAssertions) {
    $Script:Failed++
    Write-Host "FAIL  expected $expectedAssertions assertions, ran $($Script:Passed + $Script:Failed - 1)" -ForegroundColor Red
}

Write-Host "`n=== Summary ===" -ForegroundColor Cyan
Write-Host "Passed: $Script:Passed" -ForegroundColor Green
Write-Host "Failed: $Script:Failed" -ForegroundColor $(if ($Script:Failed -eq 0) { 'Green' } else { 'Red' })
if ($Script:Failed -gt 0) { exit 1 }
exit 0
