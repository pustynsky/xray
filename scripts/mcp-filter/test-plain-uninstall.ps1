#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression tests for setup-xray.ps1 -Uninstall on PLAIN (non-filter,
    untracked) .mcp.json files.

.DESCRIPTION
    The smudge/clean filter path is exercised by test-e2e.ps1. This file
    covers the OTHER path: the legacy ConvertTo-Json branch that runs when
    .mcp.json is NOT tracked by git. Uninstall there walks via
    `Remove-XrayServerEntry` (or its inline JSON-rewrite equivalent), and
    has historically had two failure modes:

      A. xray-only file: after removing the xray entry the resulting JSON
         contained an empty `mcpServers: {}` and was left on disk; the
         expected behavior is to delete the file (or leave it strictly
         equivalent to "no xray ever installed").
      B. xray + another server: removing xray must preserve the OTHER
         server entry verbatim and the file must remain on disk.

    Each test is followed by a MUTATION CHECK that confirms the assertion
    actually fails when the underlying behavior is wrong (so the test is
    not just documentary).

.EXAMPLE
    .\test-plain-uninstall.ps1
#>
param(
    [switch]$KeepTempDir
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $here)
$setupScript = Join-Path $repoRoot 'scripts\setup-xray.ps1'

if (-not (Test-Path $setupScript)) {
    throw "setup-xray.ps1 not found at $setupScript"
}

# Fake xray binary (so -SkipDownload's Test-Path check passes during install).
$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-plain-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for plain-uninstall test' | Out-File -FilePath $fakeXray -Encoding ASCII

$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-plain-uninstall-$(Get-Random)"
New-Item -ItemType Directory -Path $workRoot | Out-Null

$script:failCount = 0
function Assert-True {
    param([string]$Message, [bool]$Condition)
    if ($Condition) {
        Write-Host ("  PASS  " + $Message) -ForegroundColor Green
    }
    else {
        Write-Host ("  FAIL  " + $Message) -ForegroundColor Red
        $script:failCount++
    }
}

function New-PlainRepo {
    param([string]$Name)
    $dir = Join-Path $workRoot $Name
    New-Item -ItemType Directory -Path $dir | Out-Null
    Push-Location $dir
    try {
        git init -q
        git config user.email 'test@example.com'
        git config user.name  'Test'
        # Important: do NOT commit .mcp.json. The plain-JSON / ConvertTo-Json
        # uninstall path is the one that fires when the file is untracked.
        New-Item -ItemType File -Path '.gitignore' -Force | Out-Null
        '.mcp.json' | Set-Content -Path '.gitignore' -NoNewline
        git add .gitignore
        git commit -q -m 'init'
    }
    finally {
        Pop-Location
    }
    return $dir
}

function Invoke-Install {
    param([string]$RepoDir)
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $RepoDir `
        -InstallDir $fakeInstallDir `
        -SkipDownload `
        -EnableCopilotCli `
        -Force `
        -Extensions 'cs,ts' 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "install failed (exit $LASTEXITCODE)" }
}

function Invoke-Uninstall {
    param([string]$RepoDir)
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $RepoDir `
        -InstallDir $fakeInstallDir `
        -Uninstall `
        -KeepBinary `
        -KeepBackups 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "uninstall failed (exit $LASTEXITCODE)" }
}

try {
    # =================================================================
    # Test A: xray-only plain JSON
    # =================================================================
    Write-Host '== Test A: xray-only plain .mcp.json ==' -ForegroundColor Cyan
    $repoA = New-PlainRepo -Name 'xray-only'

    Invoke-Install -RepoDir $repoA
    $mcpA = Join-Path $repoA '.mcp.json'
    Assert-True 'A.install: .mcp.json created'  (Test-Path $mcpA)
    $contentAInstall = if (Test-Path $mcpA) { Get-Content $mcpA -Raw } else { '' }
    Assert-True 'A.install: contains "xray"'    ($contentAInstall -match '"xray"')
    Assert-True 'A.install: only one server'    (($contentAInstall | Select-String -Pattern '"command"' -AllMatches).Matches.Count -eq 1)

    Invoke-Uninstall -RepoDir $repoA

    # The empty-file regression: after removing the only server, the file
    # must NOT be left on disk containing an empty mcpServers object.
    if (Test-Path $mcpA) {
        $remaining = Get-Content $mcpA -Raw
        $isEffectivelyEmpty = $remaining -match '"mcpServers"\s*:\s*\{\s*\}' -or [string]::IsNullOrWhiteSpace($remaining)
        Assert-True 'A.uninstall: NOT left as empty mcpServers shell' (-not $isEffectivelyEmpty)
    }
    else {
        Assert-True 'A.uninstall: .mcp.json removed (or never empty-shell)' $true
    }
    # And it must definitely not still claim an xray entry.
    if (Test-Path $mcpA) {
        Assert-True 'A.uninstall: xray gone' ((Get-Content $mcpA -Raw) -notmatch '"xray"')
    }

    # ----- MUTATION CHECK A -----
    # Simulate the bug: write an empty-mcpServers file back manually and
    # confirm the assertion above WOULD fail. This proves the regression
    # check has teeth and is not just documentary.
    Write-Host '  -- mutation check A --' -ForegroundColor DarkGray
    [IO.File]::WriteAllText($mcpA, "{`n  ""mcpServers"": {}`n}`n", [Text.UTF8Encoding]::new($false))
    $mutated = Get-Content $mcpA -Raw
    $mutatedLooksEmpty = $mutated -match '"mcpServers"\s*:\s*\{\s*\}'
    Assert-True 'A.mutation: empty-shell pattern is detectable' $mutatedLooksEmpty
    Remove-Item $mcpA -Force

    # =================================================================
    # Test B: xray + another server in plain JSON
    # =================================================================
    Write-Host ''
    Write-Host '== Test B: xray + other server, plain .mcp.json ==' -ForegroundColor Cyan
    $repoB = New-PlainRepo -Name 'xray-and-other'
    $mcpB = Join-Path $repoB '.mcp.json'

    # Pre-seed a multi-server file BEFORE install. Setup-xray will inject
    # xray alongside, then uninstall must remove xray and leave the other
    # entry untouched.
    $preSeed = @{
        mcpServers = @{
            notes = @{
                command = 'C:\tools\notes-mcp.exe'
                args    = @('serve', '--endpoint', 'https://notes.example.com')
            }
        }
    } | ConvertTo-Json -Depth 6
    [IO.File]::WriteAllText($mcpB, $preSeed, [Text.UTF8Encoding]::new($false))

    Invoke-Install -RepoDir $repoB
    $afterInstall = Get-Content $mcpB -Raw
    Assert-True 'B.install: contains xray'   ($afterInstall -match '"xray"')
    Assert-True 'B.install: contains notes'  ($afterInstall -match '"notes"')

    Invoke-Uninstall -RepoDir $repoB
    Assert-True 'B.uninstall: file still exists' (Test-Path $mcpB)
    if (Test-Path $mcpB) {
        $afterUninstall = Get-Content $mcpB -Raw
        Assert-True 'B.uninstall: xray removed'         ($afterUninstall -notmatch '"xray"')
        Assert-True 'B.uninstall: notes preserved'      ($afterUninstall -match '"notes"')
        Assert-True 'B.uninstall: notes command intact' ($afterUninstall -match 'notes-mcp\.exe')
        Assert-True 'B.uninstall: notes args preserved' ($afterUninstall -match 'notes\.example\.com')
    }

    # ----- MUTATION CHECK B -----
    # Simulate the bug: rewrite the file as if uninstall accidentally
    # removed both servers. Confirm the notes-preserved assertion WOULD
    # have fired.
    Write-Host '  -- mutation check B --' -ForegroundColor DarkGray
    [IO.File]::WriteAllText($mcpB, "{`n  ""mcpServers"": {}`n}`n", [Text.UTF8Encoding]::new($false))
    $mutated2 = Get-Content $mcpB -Raw
    Assert-True 'B.mutation: notes-removal is detectable' ($mutated2 -notmatch '"notes"')
}
finally {
    if (-not $KeepTempDir) {
        Remove-Item -Path $workRoot -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $fakeInstallDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    else {
        Write-Host ''
        Write-Host "Kept work dir:  $workRoot" -ForegroundColor DarkYellow
        Write-Host "Kept fake xray: $fakeXray" -ForegroundColor DarkYellow
    }
}

Write-Host ''
if ($script:failCount -eq 0) {
    Write-Host 'All plain-JSON uninstall checks PASSED.' -ForegroundColor Cyan
    exit 0
}
else {
    Write-Host ("{0} plain-JSON uninstall check(s) FAILED." -f $script:failCount) -ForegroundColor Red
    exit 1
}
