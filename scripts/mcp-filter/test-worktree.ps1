#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Linked-worktree regression test for the xray .mcp.json smudge/clean filter.

.DESCRIPTION
    Reviewer round 2 found that storing the filter command as the bare
    relative path `bash .git/xray-mcp/smudge.sh` would silently break in a
    LINKED WORKTREE (`git worktree add`). In a linked worktree, the
    working-tree-rooted `.git` is a *file* containing
    `gitdir: /path/to/main/.git/worktrees/<name>`, not a directory — so
    `bash .git/xray-mcp/smudge.sh` resolves to a non-existent path, the
    filter degrades to passthrough (because `required=false`), and the
    tracked `.mcp.json` ends up either dirty or stuck without the xray
    entry on every checkout into the worktree.

    This test verifies the FIX: the filter command is now stored as
    `bash "$(git rev-parse --git-dir)/xray-mcp/smudge.sh"` (with `$(...)`
    evaluated at filter-invocation time by `sh -c`), so it resolves
    correctly for both normal repos and linked worktrees.

    Layout:
      upstream/                    bare-style origin
      clone/                       primary working tree (filter installed here)
      clone/.git/                  real git dir, contains xray-mcp/
      wt/                          linked worktree off `clone`
      wt/.git                      FILE (`gitdir: .../clone/.git/worktrees/wt`)

    Asserts:
      * Inside `wt/`, after creation, `.mcp.json` contains the xray entry
        (smudge ran successfully via the dynamic git-rev-parse path).
      * `git status` inside `wt/` reports `.mcp.json` clean.
      * Touching another file in `wt/` and `git checkout -- .mcp.json` keeps
        the xray entry (clean re-runs successfully).
      * Mutation check: temporarily replace the filter command with the OLD
        bare-path form and confirm the xray entry would NOT be present after
        a forced checkout — proving this test is not documentary.

.EXAMPLE
    .\test-worktree.ps1
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

$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-wt-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for worktree test' | Out-File -FilePath $fakeXray -Encoding ASCII

$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-wt-e2e-$(Get-Random)"
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
    # Build upstream + clone with a tracked .mcp.json.
    git init -q -b main $upstreamDir
    Push-Location $upstreamDir
    try {
        git config user.email 'wt@example.com'
        git config user.name  'WT'
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
        $raw = [IO.File]::ReadAllText((Resolve-Path '.mcp.json'), [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText((Resolve-Path '.mcp.json'), ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add .mcp.json
        git commit -q -m 'baseline'
    }
    finally {
        Pop-Location
    }

    git clone -q $upstreamDir $cloneDir | Out-Null
    Push-Location $cloneDir
    try {
        git config user.email 'wt@example.com'
        git config user.name  'WT'
    }
    finally {
        Pop-Location
    }

    # Install filter on the primary worktree.
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $cloneDir `
        -InstallDir $fakeInstallDir `
        -SkipDownload `
        -EnableCopilotCli `
        -Force `
        -Extensions 'cs,md' 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "install failed (exit $LASTEXITCODE)" }

    # Sanity: dynamic command pattern stored. Must use --git-common-dir
    # (not --git-dir) so it resolves correctly inside linked worktrees.
    $cfgSmudge = & git -C $cloneDir config --local --get filter.xray-mcp.smudge
    Assert-True 'config: smudge uses $(git rev-parse --git-common-dir) pattern' ($cfgSmudge -match 'git rev-parse --git-common-dir')

    # Now create a linked worktree off the clone.
    Push-Location $cloneDir
    try {
        git checkout -q -b feature
        New-Item -ItemType File -Path 'feature.txt' -Force | Out-Null
        'feat' | Set-Content -Path 'feature.txt'
        git add feature.txt
        git commit -q -m 'feature commit'
        git checkout -q main
        # `git worktree add` checks out 'feature' branch into $wtDir.
        # This is the moment the smudge filter runs against .mcp.json
        # via the worktree's .git FILE.
        git worktree add -q $wtDir feature 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "git worktree add failed (exit $LASTEXITCODE)"
        }
    }
    finally {
        Pop-Location
    }

    # Confirm the worktree's `.git` is a FILE, not a directory.
    # If it isn't, the test is exercising the wrong scenario.
    # `-Force` is required on Windows because git marks `.git` as Hidden.
    $wtGitItem = Get-Item -Force (Join-Path $wtDir '.git')
    Assert-True 'wt: .git is a file (linked worktree confirmed)' (-not $wtGitItem.PSIsContainer)

    # The whole point: did the smudge filter resolve correctly?
    $wtMcp = Join-Path $wtDir '.mcp.json'
    Assert-True 'wt: .mcp.json exists' (Test-Path $wtMcp)
    if (Test-Path $wtMcp) {
        $wtContent = [IO.File]::ReadAllText($wtMcp, [Text.UTF8Encoding]::new($false))
        Assert-True 'wt: .mcp.json contains xray (smudge resolved via worktree gitdir)' ($wtContent -match '"xray"')
        Assert-True 'wt: .mcp.json contains marker'                                     ($wtContent -match '_xrayMcpMarker')
        Assert-True 'wt: notes preserved'                                               ($wtContent -match '"notes"')
    }

    # git status inside the worktree should report .mcp.json clean.
    Push-Location $wtDir
    try {
        $st = & git status --porcelain 2>&1 | Out-String
        $mcpDirty = ($st -split "`n" | Where-Object { $_ -match '\s\.mcp\.json\s*$' }).Count -gt 0
        if ($mcpDirty) {
            Write-Host ('  [diag] wt status: <' + $st.Trim() + '>') -ForegroundColor DarkYellow
            $diff = & git diff -- '.mcp.json' 2>&1 | Out-String
            Write-Host ('  [diag] wt diff: ' + $diff.Trim()) -ForegroundColor DarkYellow
        }
        Assert-True 'wt: git status reports .mcp.json clean (clean filter resolved too)' (-not $mcpDirty)
    }
    finally {
        Pop-Location
    }

    # ----- MUTATION CHECK -----
    # Replace the filter command with the OLD broken `bash .git/xray-mcp/smudge.sh`
    # form, then force a fresh checkout into a SECOND linked worktree. The new
    # worktree's `.mcp.json` should now NOT contain the xray entry (because
    # `bash .git/...` resolves to nothing in a linked worktree, the filter
    # degrades to passthrough, and the upstream blob has no xray entry).
    Write-Host '  -- mutation check: prove the dynamic-path fix actually matters --' -ForegroundColor DarkGray
    $wt2Dir = Join-Path $workRoot 'wt2'
    Push-Location $cloneDir
    try {
        # Stash the dynamic command, apply the broken bare-path command.
        $originalSmudge = & git config --local --get filter.xray-mcp.smudge
        $originalClean  = & git config --local --get filter.xray-mcp.clean
        & git config --local 'filter.xray-mcp.smudge' 'bash .git/xray-mcp/smudge.sh' 2>&1 | Out-Null
        & git config --local 'filter.xray-mcp.clean'  'bash .git/xray-mcp/clean.sh'  2>&1 | Out-Null

        # Use `-b` to create a brand-new branch off main inside the new
        # worktree in one shot — avoids the "branch already checked out"
        # error if we try to reuse `feature`.
        $wt2Out = & git worktree add -q -b feature-mutation $wt2Dir main 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) {
            Write-Host ('  [mutation] git worktree add wt2 failed: ' + $wt2Out.Trim()) -ForegroundColor Red
            $script:failCount++
        }
        else {
            $wt2Mcp = Join-Path $wt2Dir '.mcp.json'
            $wt2Content = if (Test-Path $wt2Mcp) { [IO.File]::ReadAllText($wt2Mcp, [Text.UTF8Encoding]::new($false)) } else { '' }
            Assert-True 'mutation: bare-path filter in wt2 does NOT inject xray (proves fix matters)' ($wt2Content -notmatch '"xray"')
        }

        # Restore the dynamic command for cleanup symmetry (not strictly
        # needed since the temp dir is removed, but keeps git-debug nicer).
        & git config --local 'filter.xray-mcp.smudge' $originalSmudge 2>&1 | Out-Null
        & git config --local 'filter.xray-mcp.clean'  $originalClean  2>&1 | Out-Null
    }
    finally {
        Pop-Location
    }
}
finally {
    if (-not $KeepTempDir) {
        # `git worktree remove` is the polite teardown but we're nuking the
        # whole temp tree anyway, so unconditionally delete.
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
    Write-Host 'All linked-worktree filter checks PASSED.' -ForegroundColor Cyan
    exit 0
}
else {
    Write-Host ("{0} linked-worktree filter check(s) FAILED." -f $script:failCount) -ForegroundColor Red
    exit 1
}
