#!/usr/bin/env pwsh
<#
.SYNOPSIS
    End-to-end test for the .mcp.json smudge/clean filter installed by
    setup-xray.ps1 -EnableCopilotCli.

.DESCRIPTION
    Sets up two temp git repos:
      * 'upstream' (bare-ish) playing the role of origin.
      * 'clone'    cloned from upstream, simulating the user's working copy.

    Walks through the lifecycle:
      1. Clone has tracked .mcp.json with two upstream MCP servers.
      2. Run setup-xray.ps1 -EnableCopilotCli -Force on the clone.
         Verify:
           - .mcp.json now contains the xray entry with marker.
           - git status is clean.
           - git diff is empty.
           - .git/xray-mcp/{smudge.sh,clean.sh,snapshot.txt} exist.
           - .git/info/attributes has the filter line.
           - .git/config has [filter "xray-mcp"].
      3. Upstream modifies .mcp.json (adds another server).
         Verify clone's git pull succeeds silently and the working tree
         after pull contains BOTH the new upstream server AND xray entry.
      4. git stash; git stash pop. Verify xray entry survives.
      5. git reset --hard. Verify xray entry survives (smudge re-injects).
      6. git checkout -b feature; touch a file; commit; checkout main.
         Verify xray entry survives across branch switches.
      7. Run setup-xray.ps1 -Uninstall -Force on the clone.
         Verify:
           - .mcp.json reverted to upstream-only form.
           - .git/xray-mcp/ removed.
           - .git/info/attributes line removed.
           - .git/config filter section removed.
           - git status clean.

    Does not download anything; uses a fake xray.exe path. We never run
    the actual xray binary; we only test setup-xray's wiring.

.EXAMPLE
    .\test-e2e.ps1
#>
param(
    [switch]$KeepTempDir
)

$ErrorActionPreference = 'Stop'
# Native non-zero exit codes are inspected explicitly via $LASTEXITCODE in this
# test; do not let PowerShell 7's default behavior convert them into terminating
# errors that would short-circuit the test sequence.
$PSNativeCommandUseErrorActionPreference = $false

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $here)
$setupScript = Join-Path $repoRoot 'scripts\setup-xray.ps1'

if (-not (Test-Path $setupScript)) {
    throw "setup-xray.ps1 not found at $setupScript"
}

# Guarantee we have a fake binary path that exists (so setup-xray's
# Test-Path check passes when we use -SkipDownload).
$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-e2e-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for e2e test' | Out-File -FilePath $fakeXray -Encoding ASCII

# Working area.
$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-mcp-e2e-$(Get-Random)"
New-Item -ItemType Directory -Path $workRoot | Out-Null
$upstreamDir = Join-Path $workRoot 'upstream'
$cloneDir    = Join-Path $workRoot 'clone'

# Track failures rather than throwing immediately so we get a full report.
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

function Read-Mcp {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $null }
    return [IO.File]::ReadAllText($Path, [Text.UTF8Encoding]::new($false))
}

function Run-Git {
    param(
        [string]$Cwd,
        [Parameter(ValueFromRemainingArguments)] [string[]]$Args
    )
    Push-Location $Cwd
    try {
        $output = & git @Args 2>&1 | Out-String
        return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output.Trim() }
    }
    finally {
        Pop-Location
    }
}

try {
    Write-Host "Work dir:   $workRoot" -ForegroundColor Cyan
    Write-Host "Fake xray:  $fakeXray" -ForegroundColor Cyan
    Write-Host ''

    # ---- Step 1: build upstream + clone ----
    Write-Host '== Step 1: prepare upstream and clone ==' -ForegroundColor Cyan

    git init -q -b main $upstreamDir
    Push-Location $upstreamDir
    try {
        git config user.email 'e2e@example.com'
        git config user.name 'E2E'
        # Two baseline MCP servers tracked in .mcp.json.
        @'
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
'@ | Set-Content -Path '.mcp.json' -Encoding UTF8 -NoNewline
        # Convert any CRLF to LF so the test mirrors the production text eol=lf attribute.
        $raw = [IO.File]::ReadAllText((Resolve-Path '.mcp.json'), [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText((Resolve-Path '.mcp.json'), ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add .mcp.json
        git commit -q -m 'baseline mcp config'
    }
    finally {
        Pop-Location
    }

    git clone -q $upstreamDir $cloneDir | Out-Null
    Push-Location $cloneDir
    try {
        git config user.email 'e2e@example.com'
        git config user.name 'E2E'
    }
    finally {
        Pop-Location
    }

    Assert-True 'upstream initialized' (Test-Path (Join-Path $upstreamDir '.git'))
    Assert-True 'clone initialized'    (Test-Path (Join-Path $cloneDir '.git'))
    Assert-True 'clone has .mcp.json'  (Test-Path (Join-Path $cloneDir '.mcp.json'))

    # ---- Step 2: install xray via filter strategy ----
    Write-Host ''
    Write-Host '== Step 2: setup-xray.ps1 -EnableCopilotCli on clone ==' -ForegroundColor Cyan

    & pwsh -NoProfile -File $setupScript `
        -RepoPath $cloneDir `
        -InstallDir $fakeInstallDir `
        -SkipDownload `
        -EnableCopilotCli `
        -Force `
        -Extensions 'cs,md' | Out-Host

    if ($LASTEXITCODE -ne 0) {
        throw "setup-xray.ps1 install exited with code $LASTEXITCODE"
    }

    $mcpAfterInstall = Read-Mcp (Join-Path $cloneDir '.mcp.json')
    Assert-True 'install: .mcp.json contains xray entry'      ($mcpAfterInstall -match '"xray"\s*:')
    Assert-True 'install: marker present'                     ($mcpAfterInstall -match '_xrayMcpMarker')
    Assert-True 'install: notes preserved'                    ($mcpAfterInstall -match '"notes"')
    Assert-True 'install: playwright preserved'               ($mcpAfterInstall -match '"playwright"')
    Assert-True 'install: smudge.sh exists'                   (Test-Path (Join-Path $cloneDir '.git\xray-mcp\smudge.sh'))
    Assert-True 'install: clean.sh exists'                    (Test-Path (Join-Path $cloneDir '.git\xray-mcp\clean.sh'))
    Assert-True 'install: snapshot.txt exists'                (Test-Path (Join-Path $cloneDir '.git\xray-mcp\snapshot.txt'))

    $attrs = Get-Content (Join-Path $cloneDir '.git\info\attributes') -ErrorAction SilentlyContinue
    Assert-True 'install: attributes has filter line'         ($attrs -match '\.mcp\.json\s+filter=xray-mcp')
    # We deliberately do NOT add 'text eol=lf' - the perl-based filter
    # preserves CRLF bytes, so forcing LF would create a permanent
    # staged-renormalization mark on CRLF-stored upstream blobs.
    Assert-True 'install: attributes has NO eol attribute'    (-not ($attrs -match '\.mcp\.json\s+.*eol='))

    $cfgFilter = (& git -C $cloneDir config --local --get-regexp '^filter\.xray-mcp\.') 2>&1 | Out-String
    Assert-True 'install: .git/config has filter section'     ($cfgFilter -match 'filter.xray-mcp.smudge')

    # Status / diff must be clean.
    # Match exactly '.mcp.json' (end of token) so we don't false-trigger on '.mcp.json.bak'.
    $st = Run-Git $cloneDir status --porcelain
    $mcpDirty = ($st.Output -split "`n" | Where-Object { $_ -match '\s\.mcp\.json\s*$' }).Count -gt 0
    if ($mcpDirty) {
        Write-Host ('  [diag] git status output: <' + $st.Output + '>') -ForegroundColor DarkYellow
        $rawWt = [IO.File]::ReadAllBytes((Join-Path $cloneDir '.mcp.json'))
        Write-Host ('  [diag] working tree byte count: ' + $rawWt.Length) -ForegroundColor DarkYellow
    }
    Assert-True 'install: git status clean (.mcp.json not dirty)'    (-not $mcpDirty)
    $df = Run-Git $cloneDir diff -- '.mcp.json'
    Assert-True 'install: git diff .mcp.json empty'                  ([string]::IsNullOrWhiteSpace($df.Output))

    # ---- Step 3: upstream changes .mcp.json, clone pulls ----
    Write-Host ''
    Write-Host '== Step 3: upstream modifies .mcp.json, clone pulls ==' -ForegroundColor Cyan
    Push-Location $upstreamDir
    try {
        @'
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
    },
    "tasks": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@some/tasks-mcp"]
    }
  }
}
'@ | Set-Content -Path '.mcp.json' -Encoding UTF8 -NoNewline
        $raw = [IO.File]::ReadAllText((Resolve-Path '.mcp.json'), [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText((Resolve-Path '.mcp.json'), ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add .mcp.json
        git commit -q -m 'add tasks server'
    }
    finally {
        Pop-Location
    }

    $pull = Run-Git $cloneDir pull
    Assert-True 'pull: succeeded (exit 0)' ($pull.ExitCode -eq 0)
    if ($pull.ExitCode -ne 0) {
        Write-Host '  pull output:' -ForegroundColor DarkGray
        Write-Host $pull.Output -ForegroundColor DarkGray
    }

    $mcpAfterPull = Read-Mcp (Join-Path $cloneDir '.mcp.json')
    Assert-True 'pull: notes present'           ($mcpAfterPull -match '"notes"')
    Assert-True 'pull: playwright present'      ($mcpAfterPull -match '"playwright"')
    Assert-True 'pull: tasks added'             ($mcpAfterPull -match '"tasks"')
    Assert-True 'pull: xray STILL present'      ($mcpAfterPull -match '"xray"')
    Assert-True 'pull: marker still present'    ($mcpAfterPull -match '_xrayMcpMarker')

    $st2 = Run-Git $cloneDir status --porcelain
    $mcpDirty2 = ($st2.Output -split "`n" | Where-Object { $_ -match '\s\.mcp\.json\s*$' }).Count -gt 0
    if ($mcpDirty2) {
        Write-Host ('  [diag] post-pull status: <' + $st2.Output + '>') -ForegroundColor DarkYellow
    }
    Assert-True 'pull: status clean afterwards' (-not $mcpDirty2)

    # ---- Step 4: stash + pop ----
    Write-Host ''
    Write-Host '== Step 4: git stash + git stash pop ==' -ForegroundColor Cyan
    # We need an actual local edit to stash. Touch an unrelated file.
    New-Item -ItemType File -Path (Join-Path $cloneDir 'dummy.txt') | Out-Null
    'edit' | Set-Content -Path (Join-Path $cloneDir 'dummy.txt')
    Push-Location $cloneDir
    try {
        git add dummy.txt
        git commit -q -m 'add dummy'
        'edit2' | Set-Content -Path 'dummy.txt'
        $stashOut = & git stash --include-untracked 2>&1 | Out-String
        Assert-True 'stash: succeeded' ($LASTEXITCODE -eq 0)
        $mcpAfterStash = Read-Mcp (Join-Path $cloneDir '.mcp.json')
        Assert-True 'stash: xray still present in working tree' ($mcpAfterStash -match '"xray"')
        $popOut = & git stash pop 2>&1 | Out-String
        $popOk = ($LASTEXITCODE -eq 0)
        if (-not $popOk) {
            Write-Host ('  [diag] git stash pop output: ' + $popOut) -ForegroundColor DarkYellow
        }
        Assert-True 'stash pop: succeeded' $popOk
        $mcpAfterPop = Read-Mcp (Join-Path $cloneDir '.mcp.json')
        Assert-True 'stash pop: xray still present' ($mcpAfterPop -match '"xray"')
    }
    finally {
        Pop-Location
    }

    # ---- Step 5: reset --hard ----
    Write-Host ''
    Write-Host '== Step 5: git reset --hard ==' -ForegroundColor Cyan
    $reset = Run-Git $cloneDir reset --hard HEAD
    Assert-True 'reset: succeeded' ($reset.ExitCode -eq 0)
    $mcpAfterReset = Read-Mcp (Join-Path $cloneDir '.mcp.json')
    Assert-True 'reset: xray still present' ($mcpAfterReset -match '"xray"')
    Assert-True 'reset: marker still present' ($mcpAfterReset -match '_xrayMcpMarker')

    # ---- Step 6: branch switch ----
    Write-Host ''
    Write-Host '== Step 6: branch switch ==' -ForegroundColor Cyan
    Push-Location $cloneDir
    try {
        git checkout -q -b feature
        New-Item -ItemType File -Path 'feature.txt' -Force | Out-Null
        'feat' | Set-Content -Path 'feature.txt'
        git add feature.txt
        git commit -q -m 'feature commit'
        git checkout -q main
        $mcpAfterCheckout = Read-Mcp (Join-Path $cloneDir '.mcp.json')
        Assert-True 'checkout main: xray still present' ($mcpAfterCheckout -match '"xray"')
        git checkout -q feature
        $mcpAfterCheckout2 = Read-Mcp (Join-Path $cloneDir '.mcp.json')
        Assert-True 'checkout feature: xray still present' ($mcpAfterCheckout2 -match '"xray"')
        git checkout -q main
    }
    finally {
        Pop-Location
    }

    # ---- Step 7: uninstall ----
    Write-Host ''
    Write-Host '== Step 7: setup-xray.ps1 -Uninstall ==' -ForegroundColor Cyan
    & pwsh -NoProfile -File $setupScript `
        -RepoPath $cloneDir `
        -InstallDir $fakeInstallDir `
        -Uninstall `
        -KeepBackups `
        -KeepBinary | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "setup-xray.ps1 -Uninstall exited with code $LASTEXITCODE"
    }

    $mcpAfterUninstall = Read-Mcp (Join-Path $cloneDir '.mcp.json')
    Assert-True 'uninstall: xray gone'                        ($mcpAfterUninstall -notmatch '"xray"')
    Assert-True 'uninstall: marker gone'                      ($mcpAfterUninstall -notmatch '_xrayMcpMarker')
    Assert-True 'uninstall: notes preserved'                  ($mcpAfterUninstall -match '"notes"')
    Assert-True 'uninstall: .git/xray-mcp removed'            (-not (Test-Path (Join-Path $cloneDir '.git\xray-mcp')))
    $attrs2 = @()
    $attrPath = Join-Path $cloneDir '.git\info\attributes'
    if (Test-Path $attrPath) { $attrs2 = Get-Content $attrPath -ErrorAction SilentlyContinue }
    Assert-True 'uninstall: attributes line removed'          (($attrs2 | Where-Object { $_ -match '\.mcp\.json\s+filter=xray-mcp' }).Count -eq 0)
    $cfgAfter = & git -C $cloneDir config --local --get-regexp '^filter\.xray-mcp\.' 2>&1 | Out-String
    Assert-True 'uninstall: .git/config filter section gone'  ([string]::IsNullOrWhiteSpace($cfgAfter))

    $st3 = Run-Git $cloneDir status --porcelain
    $mcpDirty3 = ($st3.Output -split "`n" | Where-Object { $_ -match '\s\.mcp\.json\s*$' }).Count -gt 0
    if ($mcpDirty3) {
        Write-Host ('  [diag] post-uninstall status: <' + $st3.Output + '>') -ForegroundColor DarkYellow
        $bytesAfter = [IO.File]::ReadAllBytes((Join-Path $cloneDir '.mcp.json'))
        $hasCrlf = $false
        for ($k = 1; $k -lt $bytesAfter.Length; $k++) {
            if ($bytesAfter[$k - 1] -eq 13 -and $bytesAfter[$k] -eq 10) { $hasCrlf = $true; break }
        }
        Write-Host ('  [diag] WT byte count: ' + $bytesAfter.Length + ', CRLF present: ' + $hasCrlf) -ForegroundColor DarkYellow
        $diff = Run-Git $cloneDir diff -- '.mcp.json'
        Write-Host ('  [diag] git diff: ' + $diff.Output) -ForegroundColor DarkYellow
    }
    Assert-True 'uninstall: .mcp.json not dirty'              (-not $mcpDirty3)
}
finally {
    if (-not $KeepTempDir) {
        Remove-Item -Path $workRoot -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $fakeInstallDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    else {
        Write-Host ''
        Write-Host "Kept work dir: $workRoot" -ForegroundColor DarkYellow
        Write-Host "Kept fake xray: $fakeXray" -ForegroundColor DarkYellow
    }
}

Write-Host ''
if ($script:failCount -eq 0) {
    Write-Host 'All E2E checks PASSED.' -ForegroundColor Cyan
    exit 0
}
else {
    Write-Host ("{0} E2E check(s) FAILED." -f $script:failCount) -ForegroundColor Red
    exit 1
}
