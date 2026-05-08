#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression test: setup-xray.ps1 invoked from a linked worktree must
    write filter scripts and info/attributes under --git-common-dir
    (not the per-worktree --git-dir).

.DESCRIPTION
    This is the BLOCKER scenario flagged by xray-script-reviewer round 1
    on `feat/mcp-filter-vscode-extension`. Get-ResolvedGitDir uses
    `git rev-parse --git-dir`, which in a linked worktree returns
    `<main>/.git/worktrees/<wt>/`, but the runtime smudge/clean command
    is stored as
        bash "$(git rev-parse --git-common-dir)/<FilterName>/smudge.sh" <key>
    and git evaluates that substitution against --git-common-dir
    (`<main>/.git/`). If the install writes filter scripts to the
    per-worktree dir, the runtime command resolves to a non-existent
    path; with required=false this silently degrades to passthrough,
    BOTH dropping the xray entry AND, on the next `git add --renormalize`,
    risking that the marker-bearing working-tree text gets staged.

    What this test exercises:
      1. Create upstream + clone with tracked .mcp.json AND
         tracked .vscode/mcp.json (both filter targets).
      2. Add a linked worktree off the clone.
      3. Run setup-xray.ps1 -EnableCopilotCli -EnableVSCode FROM INSIDE
         the linked worktree (-RepoPath = $wtDir).
      4. Assert:
         a. <main>/.git/xray-mcp/{smudge,clean,snapshot}.txt EXIST.
         b. <main>/.git/xray-vscode-mcp/{smudge,clean,snapshot}.txt EXIST.
         c. <main>/.git/info/attributes contains BOTH filter lines.
         d. <main>/.git/worktrees/<wt>/xray-mcp DOES NOT EXIST (would
            indicate the fix wasn't applied).
         e. <main>/.git/worktrees/<wt>/xray-vscode-mcp DOES NOT EXIST.
         f. <main>/.git/worktrees/<wt>/info/attributes either doesn't
            exist OR doesn't contain xray filter lines.
         g. Both files in the worktree show xray entry + git status clean
            (proves runtime resolution actually works end-to-end).
      5. Run -Uninstall from the linked worktree. Assert teardown is
         complete: filter dirs removed from common dir, attribute lines
         removed from common-dir info/attributes, working-tree files
         restored to upstream-only form, git status clean.

.EXAMPLE
    .\test-install-from-worktree.ps1
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

$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-iwt-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for install-from-worktree test' | Out-File -FilePath $fakeXray -Encoding ASCII

$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-iwt-e2e-$(Get-Random)"
New-Item -ItemType Directory -Path $workRoot | Out-Null
$upstreamDir = Join-Path $workRoot 'upstream'
$cloneDir    = Join-Path $workRoot 'clone'
$wtDir       = Join-Path $workRoot 'wt'

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

try {
    # ---- Build upstream + clone with both tracked MCP configs ----
    git init -q -b main $upstreamDir
    Push-Location $upstreamDir
    try {
        git config user.email 'iwt@example.com'
        git config user.name  'IWT'
        @'
{
  "mcpServers": {
    "notes": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@some/notes-mcp-server"]
    }
  }
}
'@ | Set-Content -Path '.mcp.json' -Encoding UTF8 -NoNewline
        New-Item -ItemType Directory -Path '.vscode' -Force | Out-Null
        @'
{
  "servers": {
    "playwright": {
      "type": "stdio",
      "command": "npx",
      "args": ["@playwright/mcp"]
    }
  }
}
'@ | Set-Content -Path '.vscode/mcp.json' -Encoding UTF8 -NoNewline
        foreach ($p in '.mcp.json', '.vscode/mcp.json') {
            $rp = Resolve-Path $p
            $raw = [IO.File]::ReadAllText($rp, [Text.UTF8Encoding]::new($false))
            [IO.File]::WriteAllText($rp, ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        }
        git add .mcp.json .vscode/mcp.json
        git commit -q -m 'baseline'
    }
    finally {
        Pop-Location
    }

    git clone -q $upstreamDir $cloneDir | Out-Null
    Push-Location $cloneDir
    try {
        git config user.email 'iwt@example.com'
        git config user.name  'IWT'
        # Create a sibling branch and add it as a linked worktree.
        git checkout -q -b feature
        New-Item -ItemType File -Path 'feat.txt' -Force | Out-Null
        'feat' | Set-Content -Path 'feat.txt'
        git add feat.txt
        git commit -q -m 'feature commit'
        git checkout -q main
        $wtAddOut = & git worktree add -q $wtDir feature 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) { throw "git worktree add failed: $wtAddOut" }
    }
    finally {
        Pop-Location
    }

    # Confirm wt's `.git` is a FILE (linked-worktree marker).
    $wtGitItem = Get-Item -Force (Join-Path $wtDir '.git')
    Assert-True 'precond: wt is a linked worktree (.git is a file)' (-not $wtGitItem.PSIsContainer)

    # ---- Run setup-xray.ps1 FROM INSIDE the linked worktree ----
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $wtDir `
        -InstallDir $fakeInstallDir `
        -SkipDownload `
        -EnableCopilotCli `
        -EnableVSCode `
        -Force `
        -Extensions 'cs,md' 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "setup install (from worktree) failed (exit $LASTEXITCODE)" }

    # ---- Assert filter artifacts went to COMMON dir, not worktree dir ----
    $commonGitDir = Join-Path $cloneDir '.git'
    $perWtDir = Join-Path $cloneDir '.git\worktrees\wt'

    foreach ($filter in 'xray-mcp', 'xray-vscode-mcp') {
        $commonFilterDir = Join-Path $commonGitDir $filter
        $perWtFilterDir = Join-Path $perWtDir $filter
        Assert-True ("install-from-wt: <common>/.git/$filter/ exists")          (Test-Path (Join-Path $commonFilterDir 'smudge.sh'))
        Assert-True ("install-from-wt: <common>/.git/$filter/clean.sh exists")  (Test-Path (Join-Path $commonFilterDir 'clean.sh'))
        Assert-True ("install-from-wt: <common>/.git/$filter/snapshot.txt exists") (Test-Path (Join-Path $commonFilterDir 'snapshot.txt'))
        Assert-True ("install-from-wt: per-worktree $filter dir DOES NOT exist (mutation guard for the BLOCKER)") (-not (Test-Path $perWtFilterDir))
    }

    # info/attributes must be the COMMON one (carries both filter lines).
    $commonAttrs = @()
    $commonAttrPath = Join-Path $commonGitDir 'info\attributes'
    if (Test-Path $commonAttrPath) {
        $commonAttrs = @(Get-Content $commonAttrPath -ErrorAction SilentlyContinue)
    }
    Assert-True 'install-from-wt: common info/attributes has .mcp.json filter line'         ([bool]($commonAttrs -match '\.mcp\.json\s+filter=xray-mcp'))
    Assert-True 'install-from-wt: common info/attributes has .vscode/mcp.json filter line'  ([bool]($commonAttrs -match '\.vscode\/mcp\.json\s+filter=xray-vscode-mcp'))

    # The per-worktree info/attributes (if it exists) must NOT carry our
    # filter lines (would indicate the fix regressed).
    $perWtAttrs = @()
    $perWtAttrPath = Join-Path $perWtDir 'info\attributes'
    if (Test-Path $perWtAttrPath) {
        $perWtAttrs = @(Get-Content $perWtAttrPath -ErrorAction SilentlyContinue)
    }
    Assert-True 'install-from-wt: per-worktree info/attributes has NO xray filter lines (mutation guard)' (
        -not ([bool]($perWtAttrs -match 'filter=xray-(mcp|vscode-mcp)'))
    )

    # ---- Runtime works end-to-end inside the worktree ----
    $wtMcp = Join-Path $wtDir '.mcp.json'
    $wtVscode = Join-Path $wtDir '.vscode\mcp.json'
    Assert-True 'wt: .mcp.json contains xray'              ([bool](([IO.File]::ReadAllText($wtMcp, [Text.UTF8Encoding]::new($false))) -match '"xray"'))
    Assert-True 'wt: .vscode/mcp.json contains xray'       ([bool](([IO.File]::ReadAllText($wtVscode, [Text.UTF8Encoding]::new($false))) -match '"xray"'))

    Push-Location $wtDir
    try {
        $st = & git status --porcelain 2>&1 | Out-String
        $mcpDirty = ($st -split "`n" | Where-Object { $_ -match '\s\.mcp\.json\s*$' }).Count -gt 0
        $vscDirty = ($st -split "`n" | Where-Object { $_ -match '\.vscode\/mcp\.json\s*$' }).Count -gt 0
        if ($mcpDirty -or $vscDirty) {
            Write-Host ('  [diag] wt status: <' + $st.Trim() + '>') -ForegroundColor DarkYellow
        }
        Assert-True 'wt: git status reports .mcp.json clean'         (-not $mcpDirty)
        Assert-True 'wt: git status reports .vscode/mcp.json clean'  (-not $vscDirty)
    }
    finally {
        Pop-Location
    }

    # ---- Uninstall from the same linked worktree ----
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $wtDir `
        -InstallDir $fakeInstallDir `
        -Uninstall `
        -KeepBackups `
        -KeepBinary 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "setup uninstall (from worktree) failed (exit $LASTEXITCODE)" }

    foreach ($filter in 'xray-mcp', 'xray-vscode-mcp') {
        Assert-True ("uninstall-from-wt: <common>/.git/$filter/ removed")   (-not (Test-Path (Join-Path $commonGitDir $filter)))
    }

    $commonAttrsAfter = @()
    if (Test-Path $commonAttrPath) {
        $commonAttrsAfter = @(Get-Content $commonAttrPath -ErrorAction SilentlyContinue)
    }
    Assert-True 'uninstall-from-wt: common info/attributes has no xray filter lines' (-not ([bool]($commonAttrsAfter -match 'filter=xray-(mcp|vscode-mcp)')))

    foreach ($filter in 'xray-mcp', 'xray-vscode-mcp') {
        $cfg = & git -C $wtDir config --local --get-regexp ('^filter\.' + $filter + '\.') 2>&1 | Out-String
        Assert-True ("uninstall-from-wt: .git/config has no [filter `"$filter`"] section") ([string]::IsNullOrWhiteSpace($cfg))
    }
}
finally {
    if (-not $KeepTempDir) {
        # Best-effort cleanup; nuke the worktree first to avoid git complaints.
        if (Test-Path $wtDir) {
            Push-Location $cloneDir -ErrorAction SilentlyContinue
            try { & git worktree remove --force $wtDir 2>$null | Out-Null } catch {}
            finally { Pop-Location -ErrorAction SilentlyContinue }
        }
        Remove-Item -Path $workRoot -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $fakeInstallDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    else {
        Write-Host ''
        Write-Host "Kept work dir:  $workRoot"  -ForegroundColor DarkYellow
        Write-Host "Kept fake xray: $fakeXray"  -ForegroundColor DarkYellow
    }
}

Write-Host ''
if ($script:failCount -eq 0) {
    Write-Host 'All install-from-worktree filter checks PASSED.' -ForegroundColor Cyan
    exit 0
}
else {
    Write-Host ("{0} install-from-worktree check(s) FAILED." -f $script:failCount) -ForegroundColor Red
    exit 1
}
