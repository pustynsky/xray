#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression tests for the -GitVisibility Visible|Hidden modes of
    setup-xray.ps1 (the xray entry written into mcp.json).

.DESCRIPTION
    The smudge/clean filter (Hidden mode) lifecycle is covered by
    test-e2e.ps1 (.mcp.json) and test-vscode-tracked.ps1 (.vscode/mcp.json).
    This file covers the OTHER axis: the Visible mode added later, plus the
    mode-resolution rules and the hidden<->visible transitions.

    Scenarios:
      A. -GitVisibility Visible on a tracked .mcp.json (cloned): writes a
         normal, user-visible xray entry with a "//" warning field, installs
         NO filter / NO skip-worktree / NO exclude, and leaves the file as a
         normal MODIFIED change in git status. Other servers preserved.
      B. -Force WITHOUT -GitVisibility defaults to Hidden (regression guard
         for the backward-compat default): filter installed, marker present,
         git status clean.
      C. Hidden -> Visible transition on a tracked file: the smudge/clean
         filter is torn down, the marker line is gone, and the entry becomes
         a normal visible change.
      D. Untracked .mcp.json Hidden (.git/info/exclude) -> Visible: the
         exclude entry is lifted so the file surfaces in git status.
      E. Uninstall after a Visible install removes the xray entry and
         preserves other servers.
      F. -GitVisibility Visible on a tracked .vscode/mcp.json (servers
         container shape): type=stdio entry with the "//" warning, no filter.

    Uses a fake xray.exe path with -SkipDownload; never runs the binary.

.EXAMPLE
    .\test-visible-mode.ps1
#>
param(
    [switch]$KeepTempDir
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $here)
$setupScript = Join-Path $repoRoot 'scripts\setup-xray.ps1'
if (-not (Test-Path $setupScript)) { throw "setup-xray.ps1 not found at $setupScript" }

$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-visible-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for visible-mode test' | Out-File -FilePath $fakeXray -Encoding ASCII

$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-visible-$(Get-Random)"
New-Item -ItemType Directory -Path $workRoot | Out-Null

$script:failCount = 0
function Assert-True {
    param([string]$Message, [bool]$Condition)
    if ($Condition) { Write-Host ("  PASS  " + $Message) -ForegroundColor Green }
    else { Write-Host ("  FAIL  " + $Message) -ForegroundColor Red; $script:failCount++ }
}

function Read-Mcp {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $null }
    return [IO.File]::ReadAllText($Path, [Text.UTF8Encoding]::new($false))
}

function Test-StrictJson {
    # ConvertFrom-Json TOLERATES a trailing comma; Copilot CLI's strict parser
    # does NOT. Validate with System.Text.Json (rejects trailing commas) when
    # available (PS 7), with a ConvertFrom-Json + manual trailing-comma scan
    # fallback for hosts without that assembly.
    param([string]$Text)
    if ('System.Text.Json.JsonDocument' -as [type]) {
        try { [void][System.Text.Json.JsonDocument]::Parse($Text); return $true }
        catch { return $false }
    }
    try { $null = $Text | ConvertFrom-Json -ErrorAction Stop } catch { return $false }
    return (-not ($Text -match ',\s*[\}\]]'))
}

function Invoke-Setup {
    param([string[]]$ExtraArgs)
    $common = @('-NoProfile', '-File', $setupScript, '-InstallDir', $fakeInstallDir, '-SkipDownload', '-Extensions', 'cs,md')
    & pwsh @common @ExtraArgs *> (Join-Path $workRoot 'last-setup.log')
    return $LASTEXITCODE
}

# Build a git repo with a tracked, LF-normalized mcp config, then clone it so
# the working copy is checked out through any filter (mirrors test-e2e.ps1).
function New-TrackedClone {
    param([string]$Name, [string]$RelPath, [string]$Content)
    $upstream = Join-Path $workRoot "$Name-upstream"
    $clone = Join-Path $workRoot "$Name-clone"
    git init -q -b main $upstream
    Push-Location $upstream
    try {
        git config user.email 'vis@example.com'
        git config user.name 'Vis'
        $full = Join-Path $upstream $RelPath
        $dir = Split-Path -Parent $full
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        [IO.File]::WriteAllText($full, ($Content -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add -- $RelPath
        git commit -q -m "baseline $RelPath"
    }
    finally { Pop-Location }
    git clone -q $upstream $clone | Out-Null
    Push-Location $clone
    try { git config user.email 'vis@example.com'; git config user.name 'Vis' }
    finally { Pop-Location }
    return $clone
}

function Get-GitStatusToken {
    param([string]$Cwd, [string]$RelToken)
    Push-Location $Cwd
    try {
        $st = & git status --porcelain 2>&1 | Out-String
        return ($st -split "`n" | Where-Object { $_ -match ('\s' + [Regex]::Escape($RelToken) + '\s*$') })
    }
    finally { Pop-Location }
}

$copilotBaseline = @'
{
  "mcpServers": {
    "notes": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@some/notes-mcp-server"]
    },
    "playwright": {
      "type": "stdio",
      "command": "npx",
      "args": ["@playwright/mcp"]
    }
  }
}
'@

$vscodeBaseline = @'
{
  "servers": {
    "playwright": {
      "type": "stdio",
      "command": "npx",
      "args": ["@playwright/mcp"]
    }
  }
}
'@

try {
    Write-Host "Work dir:  $workRoot" -ForegroundColor Cyan
    Write-Host "Fake xray: $fakeXray" -ForegroundColor Cyan

    # ---- A: Visible on tracked .mcp.json ----
    Write-Host "`n== A: -GitVisibility Visible on tracked .mcp.json ==" -ForegroundColor Cyan
    $cloneA = New-TrackedClone -Name 'A' -RelPath '.mcp.json' -Content $copilotBaseline
    $exit = Invoke-Setup @('-RepoPath', $cloneA, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'A: setup exit 0' ($exit -eq 0)
    $mcpA = Read-Mcp (Join-Path $cloneA '.mcp.json')
    Assert-True 'A: xray entry present'        ($mcpA -match '"xray"\s*:')
    Assert-True 'A: "//" warning field present' ($mcpA -match '"//"\s*:')
    Assert-True 'A: warning mentions commit/push' ($mcpA -match 'MAKE SURE')
    Assert-True 'A: NO hidden marker'          (-not ($mcpA -match '_xrayMcpMarker'))
    Assert-True 'A: notes preserved'           ($mcpA -match '"notes"')
    Assert-True 'A: playwright preserved'      ($mcpA -match '"playwright"')
    Assert-True 'A: NO .git/xray-mcp dir'      (-not (Test-Path (Join-Path $cloneA '.git\xray-mcp')))
    $attrsA = Get-Content (Join-Path $cloneA '.git\info\attributes') -ErrorAction SilentlyContinue
    Assert-True 'A: NO filter attribute'       (-not ($attrsA -match '\.mcp\.json\s+filter='))
    $cfgA = (& git -C $cloneA config --local --get-regexp '^filter\.xray-mcp\.') 2>&1 | Out-String
    Assert-True 'A: NO filter in .git/config'  ([string]::IsNullOrWhiteSpace($cfgA))
    Assert-True 'A: file MODIFIED in git status' (@(Get-GitStatusToken -Cwd $cloneA -RelToken '.mcp.json').Count -gt 0)
    $lsvA = (& git -C $cloneA ls-files -v '.mcp.json') 2>&1 | Out-String
    Assert-True 'A: NOT skip-worktree'         (-not ($lsvA -cmatch '^[Sh] '))
    Assert-True 'A: parses as strict JSON'     (Test-StrictJson $mcpA)

    # ---- B: -Force defaults to Hidden ----
    Write-Host "`n== B: -Force WITHOUT -GitVisibility defaults to Hidden ==" -ForegroundColor Cyan
    $cloneB = New-TrackedClone -Name 'B' -RelPath '.mcp.json' -Content $copilotBaseline
    $exit = Invoke-Setup @('-RepoPath', $cloneB, '-EnableCopilotCli', '-Force')
    Assert-True 'B: setup exit 0' ($exit -eq 0)
    $mcpB = Read-Mcp (Join-Path $cloneB '.mcp.json')
    Assert-True 'B: marker present (hidden)'   ($mcpB -match '_xrayMcpMarker')
    Assert-True 'B: filter dir created'        (Test-Path (Join-Path $cloneB '.git\xray-mcp'))
    $cfgB = (& git -C $cloneB config --local --get-regexp '^filter\.xray-mcp\.') 2>&1 | Out-String
    Assert-True 'B: filter in .git/config'     ($cfgB -match 'filter.xray-mcp.smudge')
    Assert-True 'B: git status clean (hidden)' (@(Get-GitStatusToken -Cwd $cloneB -RelToken '.mcp.json').Count -eq 0)

    # ---- C: Hidden -> Visible transition (tracked) ----
    Write-Host "`n== C: Hidden -> Visible transition tears down the filter ==" -ForegroundColor Cyan
    $exit = Invoke-Setup @('-RepoPath', $cloneB, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'C: setup exit 0' ($exit -eq 0)
    $mcpC = Read-Mcp (Join-Path $cloneB '.mcp.json')
    $cfgC = (& git -C $cloneB config --local --get-regexp '^filter\.xray-mcp\.') 2>&1 | Out-String
    Assert-True 'C: filter REMOVED from .git/config' ([string]::IsNullOrWhiteSpace($cfgC))
    Assert-True 'C: .git/xray-mcp dir removed'  (-not (Test-Path (Join-Path $cloneB '.git\xray-mcp')))
    Assert-True 'C: marker gone'                (-not ($mcpC -match '_xrayMcpMarker'))
    Assert-True 'C: "//" warning present'       ($mcpC -match '"//"\s*:')
    Assert-True 'C: file MODIFIED in git status' (@(Get-GitStatusToken -Cwd $cloneB -RelToken '.mcp.json').Count -gt 0)
    Assert-True 'C: parses as strict JSON'      (Test-StrictJson $mcpC)

    # ---- D: Untracked Hidden (exclude) -> Visible (exclude lifted) ----
    Write-Host "`n== D: Untracked .mcp.json Hidden -> Visible lifts .git/info/exclude ==" -ForegroundColor Cyan
    $repoD = Join-Path $workRoot 'D-repo'
    git init -q -b main $repoD
    Push-Location $repoD
    try {
        git config user.email 'vis@example.com'; git config user.name 'Vis'
        'readme' | Set-Content README.md -Encoding UTF8; git add README.md; git commit -q -m base
    }
    finally { Pop-Location }
    $exit = Invoke-Setup @('-RepoPath', $repoD, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Hidden')
    Assert-True 'D: hidden setup exit 0' ($exit -eq 0)
    $exclD = (& git -C $repoD rev-parse --git-path info/exclude) 2>&1 | Out-String
    $exclD = $exclD.Trim()
    $exclFull = if ([IO.Path]::IsPathRooted($exclD)) { $exclD } else { Join-Path $repoD $exclD }
    Assert-True 'D: hidden: status clean'       (@(Get-GitStatusToken -Cwd $repoD -RelToken '.mcp.json').Count -eq 0)
    Assert-True 'D: hidden: exclude has .mcp.json' ((Test-Path $exclFull) -and ((Get-Content $exclFull) -contains '.mcp.json'))
    $exit = Invoke-Setup @('-RepoPath', $repoD, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'D: visible setup exit 0' ($exit -eq 0)
    Assert-True 'D: visible: exclude line lifted' (-not ((Test-Path $exclFull) -and ((Get-Content $exclFull) -contains '.mcp.json')))
    Assert-True 'D: visible: file surfaces (??)' (@(Get-GitStatusToken -Cwd $repoD -RelToken '.mcp.json').Count -gt 0)
    $mcpD = Read-Mcp (Join-Path $repoD '.mcp.json')
    Assert-True 'D: visible: "//" warning present' ($mcpD -match '"//"\s*:')

    # ---- E: Uninstall after Visible ----
    Write-Host "`n== E: Uninstall after Visible removes xray, preserves others ==" -ForegroundColor Cyan
    $exit = Invoke-Setup @('-RepoPath', $cloneA, '-Uninstall', '-KeepBinary')
    Assert-True 'E: uninstall exit 0' ($exit -eq 0)
    $mcpE = Read-Mcp (Join-Path $cloneA '.mcp.json')
    Assert-True 'E: xray removed'        (-not ($mcpE -match '"xray"\s*:'))
    Assert-True 'E: warning field gone'  (-not ($mcpE -match '"//"\s*:'))
    Assert-True 'E: playwright preserved' ($mcpE -match '"playwright"')

    # ---- F: Visible on tracked .vscode/mcp.json ----
    Write-Host "`n== F: -GitVisibility Visible on tracked .vscode/mcp.json ==" -ForegroundColor Cyan
    $cloneF = New-TrackedClone -Name 'F' -RelPath '.vscode/mcp.json' -Content $vscodeBaseline
    $exit = Invoke-Setup @('-RepoPath', $cloneF, '-EnableVSCode', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'F: setup exit 0' ($exit -eq 0)
    $mcpF = Read-Mcp (Join-Path $cloneF '.vscode/mcp.json')
    Assert-True 'F: xray entry present'   ($mcpF -match '"xray"\s*:')
    Assert-True 'F: type stdio present'   ($mcpF -match '"type"\s*:\s*"stdio"')
    Assert-True 'F: "//" warning present' ($mcpF -match '"//"\s*:')
    Assert-True 'F: NO marker'            (-not ($mcpF -match '_xrayMcpMarker'))
    Assert-True 'F: playwright preserved' ($mcpF -match '"playwright"')
    Assert-True 'F: NO vscode filter'     (-not (Test-Path (Join-Path $cloneF '.git\xray-vscode-mcp')))
    Assert-True 'F: file MODIFIED'        (@(Get-GitStatusToken -Cwd $cloneF -RelToken '.vscode/mcp.json').Count -gt 0)
    Assert-True 'F: parses as strict JSON' (Test-StrictJson $mcpF)

    # ---- G: Visible re-inject when a prior single-line xray is NOT first ----
    # Regression for the trailing-comma bug: removing a non-first single-line
    # xray by line surgery would orphan the preceding entry's comma. The writer
    # must defer to the JSON reserialize fallback and emit strict-valid JSON.
    Write-Host "`n== G: Visible re-inject when prior single-line xray is last (strict-JSON safe) ==" -ForegroundColor Cyan
    $gContent = @'
{
  "mcpServers": {
    "notes": { "command": "npx", "args": ["-y", "@some/notes"] },
    "xray": { "command": "old-xray", "args": ["serve"], "env": {} }
  }
}
'@
    $cloneG = New-TrackedClone -Name 'G' -RelPath '.mcp.json' -Content $gContent
    $exit = Invoke-Setup @('-RepoPath', $cloneG, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'G: setup exit 0' ($exit -eq 0)
    $mcpG = Read-Mcp (Join-Path $cloneG '.mcp.json')
    Assert-True 'G: output is STRICT-valid JSON (no trailing comma)' (Test-StrictJson $mcpG)
    Assert-True 'G: exactly one xray entry' (([regex]::Matches($mcpG, '"xray"\s*:')).Count -eq 1)
    Assert-True 'G: notes preserved'        ($mcpG -match '"notes"')
    Assert-True 'G: "//" warning present'    ($mcpG -match '"//"\s*:')
    Assert-True 'G: old xray command replaced' (-not ($mcpG -match 'old-xray'))
    # Re-run must stay idempotent and strict-valid.
    $exit = Invoke-Setup @('-RepoPath', $cloneG, '-EnableCopilotCli', '-Force', '-GitVisibility', 'Visible')
    Assert-True 'G: re-run exit 0' ($exit -eq 0)
    $mcpG2 = Read-Mcp (Join-Path $cloneG '.mcp.json')
    Assert-True 'G: re-run STRICT-valid JSON' (Test-StrictJson $mcpG2)
    Assert-True 'G: re-run exactly one xray'  (([regex]::Matches($mcpG2, '"xray"\s*:')).Count -eq 1)

    Write-Host ''
    if ($script:failCount -eq 0) {
        Write-Host "ALL VISIBLE-MODE TESTS PASSED" -ForegroundColor Green
    }
    else {
        Write-Host ("VISIBLE-MODE TESTS: {0} FAILED" -f $script:failCount) -ForegroundColor Red
    }
}
finally {
    if (-not $KeepTempDir) {
        Remove-Item -Path $workRoot -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $fakeInstallDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    else {
        Write-Host "Kept: $workRoot" -ForegroundColor DarkYellow
        Write-Host "Kept: $fakeInstallDir" -ForegroundColor DarkYellow
    }
}

exit $script:failCount
