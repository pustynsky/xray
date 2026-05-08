#!/usr/bin/env pwsh
<#
.SYNOPSIS
    End-to-end test for the .vscode/mcp.json smudge/clean filter installed
    by setup-xray.ps1 -EnableVSCode.

.DESCRIPTION
    Mirror of test-e2e.ps1 but exercises the second filter (xray-vscode-mcp)
    that targets .vscode/mcp.json (VS Code's "servers" container shape).
    This is the exact scenario that originally failed on Q:\Repos\Shared:
    a tracked .vscode/mcp.json with upstream changes aborted git pull
    because the legacy install used skip-worktree.

    Lifecycle:
      1. Build upstream with tracked .vscode/mcp.json (servers container,
         two MCP entries).
      2. Clone, run setup-xray.ps1 -EnableVSCode -Force on the clone.
         Verify:
           - .vscode/mcp.json contains xray entry with marker.
           - git status clean, git diff empty.
           - .git/xray-vscode-mcp/{smudge,clean,snapshot} exist.
           - .git/info/attributes has the .vscode/mcp.json filter=xray-vscode-mcp line.
           - .git/config has [filter "xray-vscode-mcp"].
      3. Upstream adds another server. Clone runs git pull.
         Verify pull succeeds (this is the regression guard for the
         original bug) and both upstream additions and xray entry survive.
      4. git stash + pop, git reset --hard, branch switch -- xray survives.
      5. Setup-xray.ps1 -Uninstall -Force on the clone.
         Verify the filter is fully removed and .vscode/mcp.json reverts
         to upstream-only form.

.EXAMPLE
    .\test-vscode-tracked.ps1
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

$fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) "xray-vscode-e2e-bin-$(Get-Random)"
New-Item -ItemType Directory -Path $fakeInstallDir | Out-Null
$fakeXray = Join-Path $fakeInstallDir 'xray.exe'
'fake xray binary for vscode e2e test' | Out-File -FilePath $fakeXray -Encoding ASCII

$workRoot = Join-Path ([IO.Path]::GetTempPath()) "xray-vscode-e2e-$(Get-Random)"
New-Item -ItemType Directory -Path $workRoot | Out-Null
$upstreamDir = Join-Path $workRoot 'upstream'
$cloneDir    = Join-Path $workRoot 'clone'

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
    Write-Host '== Step 1: prepare upstream and clone (.vscode/mcp.json tracked) ==' -ForegroundColor Cyan

    git init -q -b main $upstreamDir
    Push-Location $upstreamDir
    try {
        git config user.email 'e2e@example.com'
        git config user.name 'E2E'
        New-Item -ItemType Directory -Path '.vscode' -Force | Out-Null
        @'
{
  "servers": {
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
'@ | Set-Content -Path '.vscode/mcp.json' -Encoding UTF8 -NoNewline
        $vscodePath = Resolve-Path '.vscode/mcp.json'
        $raw = [IO.File]::ReadAllText($vscodePath, [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($vscodePath, ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add .vscode/mcp.json
        git commit -q -m 'baseline vscode mcp config'
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

    Assert-True 'upstream initialized'                (Test-Path (Join-Path $upstreamDir '.git'))
    Assert-True 'clone initialized'                   (Test-Path (Join-Path $cloneDir '.git'))
    Assert-True 'clone has .vscode/mcp.json'          (Test-Path (Join-Path $cloneDir '.vscode\mcp.json'))

    # ---- Step 2: install xray via filter strategy ----
    Write-Host ''
    Write-Host '== Step 2: setup-xray.ps1 -EnableVSCode on clone ==' -ForegroundColor Cyan

    & pwsh -NoProfile -File $setupScript `
        -RepoPath $cloneDir `
        -InstallDir $fakeInstallDir `
        -SkipDownload `
        -EnableVSCode `
        -Force `
        -Extensions 'cs,md' | Out-Host

    if ($LASTEXITCODE -ne 0) {
        throw "setup-xray.ps1 install exited with code $LASTEXITCODE"
    }

    $mcpAfterInstall = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
    Assert-True 'install: .vscode/mcp.json contains xray entry'   ($mcpAfterInstall -match '"xray"\s*:')
    Assert-True 'install: marker present'                         ($mcpAfterInstall -match '_xrayMcpMarker')
    Assert-True 'install: notes preserved'                        ($mcpAfterInstall -match '"notes"')
    Assert-True 'install: playwright preserved'                   ($mcpAfterInstall -match '"playwright"')
    Assert-True 'install: xray entry has type=stdio'              ($mcpAfterInstall -match '"xray"\s*:\s*\{[^}]*"type"\s*:\s*"stdio"')
    Assert-True 'install: smudge.sh exists'                       (Test-Path (Join-Path $cloneDir '.git\xray-vscode-mcp\smudge.sh'))
    Assert-True 'install: clean.sh exists'                        (Test-Path (Join-Path $cloneDir '.git\xray-vscode-mcp\clean.sh'))
    Assert-True 'install: snapshot.txt exists'                    (Test-Path (Join-Path $cloneDir '.git\xray-vscode-mcp\snapshot.txt'))

    $attrs = Get-Content (Join-Path $cloneDir '.git\info\attributes') -ErrorAction SilentlyContinue
    Assert-True 'install: attributes has filter line'             ($attrs -match '\.vscode\/mcp\.json\s+filter=xray-vscode-mcp')
    Assert-True 'install: attributes has NO eol attribute'        (-not ($attrs -match '\.vscode\/mcp\.json\s+.*eol='))

    $cfgFilter = (& git -C $cloneDir config --local --get-regexp '^filter\.xray-vscode-mcp\.') 2>&1 | Out-String
    Assert-True 'install: .git/config has filter section'         ($cfgFilter -match 'filter.xray-vscode-mcp.smudge')

    $st = Run-Git $cloneDir status --porcelain
    $vscDirty = ($st.Output -split "`n" | Where-Object { $_ -match '\.vscode\/mcp\.json\s*$' }).Count -gt 0
    if ($vscDirty) {
        Write-Host ('  [diag] git status output: <' + $st.Output + '>') -ForegroundColor DarkYellow
    }
    Assert-True 'install: git status clean (.vscode/mcp.json not dirty)'    (-not $vscDirty)
    $df = Run-Git $cloneDir diff -- '.vscode/mcp.json'
    Assert-True 'install: git diff .vscode/mcp.json empty'                  ([string]::IsNullOrWhiteSpace($df.Output))

    # ---- Step 3: upstream changes .vscode/mcp.json, clone pulls ----
    # This is the REGRESSION GUARD for the original bug. Before the filter
    # extension, this exact sequence aborted with:
    #   "error: Your local changes to the following files would be
    #    overwritten by merge: .vscode/mcp.json"
    # because skip-worktree masked the local xray entry from git's diff
    # logic but not from the merge engine.
    Write-Host ''
    Write-Host '== Step 3: upstream modifies .vscode/mcp.json, clone pulls ==' -ForegroundColor Cyan
    Push-Location $upstreamDir
    try {
        @'
{
  "servers": {
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
'@ | Set-Content -Path '.vscode/mcp.json' -Encoding UTF8 -NoNewline
        $vscodePath = Resolve-Path '.vscode/mcp.json'
        $raw = [IO.File]::ReadAllText($vscodePath, [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($vscodePath, ($raw -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))
        git add .vscode/mcp.json
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

    $mcpAfterPull = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
    Assert-True 'pull: notes present'           ($mcpAfterPull -match '"notes"')
    Assert-True 'pull: playwright present'      ($mcpAfterPull -match '"playwright"')
    Assert-True 'pull: tasks added'             ($mcpAfterPull -match '"tasks"')
    Assert-True 'pull: xray STILL present'      ($mcpAfterPull -match '"xray"')
    Assert-True 'pull: marker still present'    ($mcpAfterPull -match '_xrayMcpMarker')

    $st2 = Run-Git $cloneDir status --porcelain
    $vscDirty2 = ($st2.Output -split "`n" | Where-Object { $_ -match '\.vscode\/mcp\.json\s*$' }).Count -gt 0
    if ($vscDirty2) {
        Write-Host ('  [diag] post-pull status: <' + $st2.Output + '>') -ForegroundColor DarkYellow
    }
    Assert-True 'pull: status clean afterwards' (-not $vscDirty2)

    # ---- Step 4: stash + pop, reset --hard, branch switch ----
    Write-Host ''
    Write-Host '== Step 4: git stash + pop ==' -ForegroundColor Cyan
    New-Item -ItemType File -Path (Join-Path $cloneDir 'dummy.txt') | Out-Null
    'edit' | Set-Content -Path (Join-Path $cloneDir 'dummy.txt')
    Push-Location $cloneDir
    try {
        git add dummy.txt
        git commit -q -m 'add dummy'
        'edit2' | Set-Content -Path 'dummy.txt'
        & git stash --include-untracked 2>&1 | Out-Null
        Assert-True 'stash: succeeded' ($LASTEXITCODE -eq 0)
        $mcpAfterStash = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
        Assert-True 'stash: xray still present in working tree' ($mcpAfterStash -match '"xray"')
        $popOut = & git stash pop 2>&1 | Out-String
        $popOk = ($LASTEXITCODE -eq 0)
        if (-not $popOk) {
            Write-Host ('  [diag] git stash pop output: ' + $popOut) -ForegroundColor DarkYellow
        }
        Assert-True 'stash pop: succeeded' $popOk
        $mcpAfterPop = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
        Assert-True 'stash pop: xray still present' ($mcpAfterPop -match '"xray"')
    }
    finally {
        Pop-Location
    }

    Write-Host ''
    Write-Host '== Step 5: git reset --hard ==' -ForegroundColor Cyan
    $reset = Run-Git $cloneDir reset --hard HEAD
    Assert-True 'reset: succeeded' ($reset.ExitCode -eq 0)
    $mcpAfterReset = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
    Assert-True 'reset: xray still present' ($mcpAfterReset -match '"xray"')
    Assert-True 'reset: marker still present' ($mcpAfterReset -match '_xrayMcpMarker')

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
        $mcpAfterCheckout = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
        Assert-True 'checkout main: xray still present' ($mcpAfterCheckout -match '"xray"')
        git checkout -q feature
        $mcpAfterCheckout2 = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
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

    $mcpAfterUninstall = Read-Mcp (Join-Path $cloneDir '.vscode\mcp.json')
    Assert-True 'uninstall: xray gone'                          ($mcpAfterUninstall -notmatch '"xray"')
    Assert-True 'uninstall: marker gone'                        ($mcpAfterUninstall -notmatch '_xrayMcpMarker')
    Assert-True 'uninstall: notes preserved'                    ($mcpAfterUninstall -match '"notes"')
    Assert-True 'uninstall: tasks preserved'                    ($mcpAfterUninstall -match '"tasks"')
    Assert-True 'uninstall: .git/xray-vscode-mcp removed'       (-not (Test-Path (Join-Path $cloneDir '.git\xray-vscode-mcp')))
    $attrs2 = @()
    $attrPath = Join-Path $cloneDir '.git\info\attributes'
    if (Test-Path $attrPath) { $attrs2 = Get-Content $attrPath -ErrorAction SilentlyContinue }
    Assert-True 'uninstall: attributes line removed'            (($attrs2 | Where-Object { $_ -match '\.vscode\/mcp\.json\s+filter=xray-vscode-mcp' }).Count -eq 0)
    $cfgAfter = & git -C $cloneDir config --local --get-regexp '^filter\.xray-vscode-mcp\.' 2>&1 | Out-String
    Assert-True 'uninstall: .git/config filter section gone'    ([string]::IsNullOrWhiteSpace($cfgAfter))

    $st3 = Run-Git $cloneDir status --porcelain
    $vscDirty3 = ($st3.Output -split "`n" | Where-Object { $_ -match '\.vscode\/mcp\.json\s*$' }).Count -gt 0
    if ($vscDirty3) {
        Write-Host ('  [diag] post-uninstall status: <' + $st3.Output + '>') -ForegroundColor DarkYellow
    }
    Assert-True 'uninstall: .vscode/mcp.json not dirty'         (-not $vscDirty3)
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
    Write-Host 'All VS Code tracked-file E2E checks PASSED.' -ForegroundColor Cyan
    exit 0
}
else {
    Write-Host ("{0} VS Code E2E check(s) FAILED." -f $script:failCount) -ForegroundColor Red
    exit 1
}
