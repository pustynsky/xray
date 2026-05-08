#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Tracked regression test for the standalone (one-liner / Option A2)
    install path of setup-xray.ps1.

.DESCRIPTION
    Reproduces the user-reported production bug:
      "Filter source missing: <tmp>\mcp-filter\smudge.sh"
    which fired when the bootstrap one-liner
    (`iex (irm .../setup-xray.ps1)`) or Option A2 (`iwr ... -OutFile $tmp;
    & $tmp`) executed setup-xray.ps1 with no `mcp-filter/` directory next
    to it on disk.

    Test plan (mirrors test-e2e.ps1 style):
      1. Stage a copy of setup-xray.ps1 to a temp directory with NO
         `mcp-filter/` sibling — this is the *clean* simulation of the
         standalone install scenario.
      2. Create a fresh git repo with a tracked .mcp.json (the only path
         that triggers Install-McpFilter).
      3. Invoke the staged script with -EnableCopilotCli -SkipDownload
         and assert exit 0 + filter scripts written byte-equal to the
         canonical scripts/mcp-filter/{smudge,clean}.sh.
      4. Verify the filter is wired in .git/config and actually fires:
         a `git checkout HEAD -- .mcp.json` re-injects the xray entry
         via smudge.

    Mutation guards (the reason this test exists, beyond the byte-equality
    check in test-embedded-sync.ps1):
      * Reverting the embedded fallback in Install-McpFilter (so disk-
        absence returns $false again) → step 3 fails: `Filter source
        missing` warning + non-zero exit.
      * Removing the PS 5.1 `\"` escape on the git config args → step 3
        fails on PS 5.1 with `error: no action specified` from git,
        installer aborts.

    Cross-runtime: by default runs the install in the current host. When
    the current host is PS 7 AND `powershell.exe` (PS 5.1) is on PATH,
    the test ALSO runs the install once via PS 5.1. This is the only
    place in the suite that exercises the PS 5.1 install code path —
    test-e2e.ps1 always invokes setup-xray via `pwsh -File`.

    Exits 0 on all-pass, 1 on any failure.
#>
param(
    [switch]$KeepTempDir
)

$ErrorActionPreference = 'Stop'
# Native non-zero exit codes are inspected explicitly via $LASTEXITCODE in
# this test; do not let PowerShell 7's default behavior convert them into
# terminating errors that would short-circuit the test sequence.
$PSNativeCommandUseErrorActionPreference = $false

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent (Split-Path -Parent $here)
$canonicalSetup = Join-Path $repoRoot 'scripts\setup-xray.ps1'
$canonicalSmudge = Join-Path $repoRoot 'scripts\mcp-filter\smudge.sh'
$canonicalClean = Join-Path $repoRoot 'scripts\mcp-filter\clean.sh'

if (-not (Test-Path $canonicalSetup)) { throw "setup-xray.ps1 not found at $canonicalSetup" }
if (-not (Test-Path $canonicalSmudge)) { throw "smudge.sh not found at $canonicalSmudge" }
if (-not (Test-Path $canonicalClean)) { throw "clean.sh not found at $canonicalClean" }

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

function Get-NormalizedFileBytes {
    <#
        Read $Path and return the raw bytes of the LF-normalized +
        trailing-LF-enforced UTF-8-no-BOM payload. Mirrors the
        normalization that Install-McpFilter applies on the install path,
        so we can byte-compare a freshly written filter script against
        the canonical disk source.

        Returns a byte[] (NOT a string) so any future BOM regression or
        invisible-character drift fails this assertion instead of being
        flattened by .NET string coercion.
    #>
    param([Parameter(Mandatory)] [string]$Path)
    $raw = [IO.File]::ReadAllBytes($Path)
    # Strip UTF-8 BOM if present so the comparison stays meaningful even
    # if a future build pipeline accidentally adds one to the canonical
    # source — the install path writes UTF-8 no-BOM and the production
    # bash filter cannot tolerate a BOM (`#!/usr/bin/env bash` must be
    # the first byte).
    if ($raw.Length -ge 3 -and $raw[0] -eq 0xEF -and $raw[1] -eq 0xBB -and $raw[2] -eq 0xBF) {
        $raw = $raw[3..($raw.Length - 1)]
    }
    # Decode -> normalize CRLF to LF -> ensure trailing LF -> re-encode
    # as UTF-8 no-BOM. This canonicalizes both inputs identically without
    # losing the byte-level fidelity guarantee (the final compare is
    # byte-by-byte on the encoded result).
    $text = [Text.UTF8Encoding]::new($false).GetString($raw)
    $text = $text -replace "`r`n", "`n"
    if (-not $text.EndsWith("`n")) { $text += "`n" }
    return [Text.UTF8Encoding]::new($false).GetBytes($text)
}

function Test-BytesEqual {
    param([byte[]]$A, [byte[]]$B)
    if ($null -eq $A -or $null -eq $B) { return $false }
    if ($A.Length -ne $B.Length) { return $false }
    for ($i = 0; $i -lt $A.Length; $i++) {
        if ($A[$i] -ne $B[$i]) { return $false }
    }
    return $true
}
function Invoke-StandaloneInstall {
    <#
        Run a single install round in $RuntimeExe. Returns $true on
        full pass, $false on any failure. Cleans up its temp dirs
        unless $KeepTempDir is set in the parent scope.
    #>
    param(
        [Parameter(Mandatory)] [string]$RuntimeLabel,
        [Parameter(Mandatory)] [string]$RuntimeExe
    )

    Write-Host ("== {0}: standalone install (no mcp-filter sibling) ==" -f $RuntimeLabel) -ForegroundColor Cyan

    $stage = Join-Path ([IO.Path]::GetTempPath()) ("xray-standalone-stage-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    $repo = Join-Path ([IO.Path]::GetTempPath()) ("xray-standalone-repo-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    $fakeInstallDir = Join-Path ([IO.Path]::GetTempPath()) ("xray-standalone-bin-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))

    $localFails = 0
    function Local-Assert {
        param([string]$Msg, [bool]$Cond)
        if ($Cond) { Write-Host ("  PASS  " + $Msg) -ForegroundColor Green }
        else { Write-Host ("  FAIL  " + $Msg) -ForegroundColor Red; $script:localFails++; $script:failCount++ }
    }
    $script:localFails = 0

    try {
        # Stage: setup-xray.ps1 alone in a temp dir, no mcp-filter sibling.
        New-Item -ItemType Directory -Path $stage -Force | Out-Null
        Copy-Item $canonicalSetup -Destination $stage
        Local-Assert 'staged dir has no mcp-filter sibling (clean simulation)' (-not (Test-Path (Join-Path $stage 'mcp-filter')))

        # Fresh git repo with tracked .mcp.json so the filter strategy is
        # triggered (Set-McpFile passes through both branches, but only the
        # tracked path goes through Install-McpFilter).
        New-Item -ItemType Directory -Path $repo -Force | Out-Null
        Push-Location $repo
        try {
            git init -q
            git config user.email 'standalone@example.com'
            git config user.name 'Standalone'
            $minimal = @'
{
  "mcpServers": {
  }
}
'@
            $raw = $minimal -replace "`r`n", "`n"
            [IO.File]::WriteAllText((Join-Path $repo '.mcp.json'), $raw, [Text.UTF8Encoding]::new($false))
            git add .mcp.json
            git commit -q -m 'baseline mcp config' | Out-Null
        }
        finally {
            Pop-Location
        }

        # Fake xray binary so -SkipDownload finds it.
        New-Item -ItemType Directory -Path $fakeInstallDir -Force | Out-Null
        $fakeXray = Join-Path $fakeInstallDir 'xray.exe'
        'fake xray binary for standalone-install test' | Out-File -FilePath $fakeXray -Encoding ASCII

        $stagedSetup = Join-Path $stage 'setup-xray.ps1'
        $log = Join-Path $stage 'install.log'

        # Invoke the staged script through the chosen runtime.
        & $RuntimeExe -NoProfile -File $stagedSetup `
            -RepoPath $repo `
            -InstallDir $fakeInstallDir `
            -SkipDownload `
            -EnableCopilotCli `
            -Extensions cs, md `
            -Force *> $log
        $exit = $LASTEXITCODE

        Local-Assert "$RuntimeLabel : install exit code 0 (got $exit)" ($exit -eq 0)

        # If the install aborted, dump the tail for diagnostics.
        if ($exit -ne 0 -and (Test-Path $log)) {
            Write-Host "  --- install log tail ---" -ForegroundColor Yellow
            Get-Content $log | Select-Object -Last 20 | ForEach-Object { Write-Host "  | $_" -ForegroundColor Yellow }
        }

        # Filter scripts: present + byte-equal to canonical disk source.
        $filterDir = Join-Path $repo '.git\xray-mcp'
        Local-Assert "$RuntimeLabel : filter dir created" (Test-Path $filterDir)

        foreach ($pair in @(
                @{ Name = 'smudge.sh'; Canonical = $canonicalSmudge },
                @{ Name = 'clean.sh'; Canonical = $canonicalClean }
            )) {
            $written = Join-Path $filterDir $pair.Name
            Local-Assert "$RuntimeLabel : $($pair.Name) written via embedded fallback" (Test-Path $written)

            if (Test-Path $written) {
                $a = Get-NormalizedFileBytes -Path $written
                $b = Get-NormalizedFileBytes -Path $pair.Canonical
                Local-Assert "$RuntimeLabel : $($pair.Name) byte-equal to canonical disk source" (Test-BytesEqual $a $b)
            }
        }

        # snapshot.txt: written.
        Local-Assert "$RuntimeLabel : snapshot.txt written" (Test-Path (Join-Path $filterDir 'snapshot.txt'))

        # filter wired in .git/config.
        Push-Location $repo
        try {
            $smudgeCmd = & git config --local 'filter.xray-mcp.smudge' 2>$null
            $cleanCmd = & git config --local 'filter.xray-mcp.clean' 2>$null
            $requiredVal = & git config --local 'filter.xray-mcp.required' 2>$null
            Local-Assert "$RuntimeLabel : filter.xray-mcp.smudge wired" ($smudgeCmd -and $smudgeCmd -match 'smudge\.sh')
            Local-Assert "$RuntimeLabel : filter.xray-mcp.clean wired" ($cleanCmd -and $cleanCmd -match 'clean\.sh')
            # Critical safety invariant: filter.<name>.required = false so
            # any failure (e.g. bash unavailable on a contributor machine)
            # degrades to passthrough rather than aborting `git checkout`.
            # Production sets this via `git config --local --bool ... false`,
            # which serializes as the literal string "false".
            Local-Assert "$RuntimeLabel : filter.xray-mcp.required = false (passthrough-on-failure invariant)" ($requiredVal -eq 'false')

            # The filter MUST actually fire on checkout. Force a checkout
            # of the tracked .mcp.json — smudge should re-inject the xray
            # entry. This is the mutation-killing assertion for the PS 5.1
            # quoting fix: if the stored filter command is malformed (e.g.
            # quotes were stripped), git will not invoke smudge here and
            # the file will not contain the marker line.
            #
            # Need bash on PATH for the filter to run; if absent on this
            # machine we skip the check rather than fail. Many Windows
            # systems have only `git\cmd` on PATH (gives `git.exe` but not
            # `bash.exe`); look in the well-known Git-for-Windows install
            # locations as a fallback.
            $bash = Get-Command bash -ErrorAction SilentlyContinue
            if (-not $bash) {
                foreach ($candidate in @(
                        "$env:ProgramFiles\Git\bin\bash.exe",
                        "${env:ProgramFiles(x86)}\Git\bin\bash.exe",
                        "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe"
                    )) {
                    if ($candidate -and (Test-Path $candidate)) {
                        # Inject the bash dir into PATH for this checkout
                        # invocation so git can find it.
                        $env:PATH = (Split-Path -Parent $candidate) + ';' + $env:PATH
                        $bash = Get-Command bash -ErrorAction SilentlyContinue
                        break
                    }
                }
            }
            # Bash is required: this is the central mutation guard for
            # the PS 5.1 quoting fix. Skipping it would give a false
            # green on machines without Git for Windows installed.
            # `test-roundtrip.ps1` takes the same hard-fail posture.
            Local-Assert "$RuntimeLabel : bash discovered for smudge-fires assertion" ($null -ne $bash)
            if ($bash) {
                # Remove the working-tree file so checkout actually re-smudges.
                Remove-Item '.mcp.json' -Force
                & git checkout HEAD -- .mcp.json 2>&1 | Out-Null
                $contents = ''
                if (Test-Path '.mcp.json') {
                    $contents = [IO.File]::ReadAllText((Join-Path $repo '.mcp.json'))
                }
                Local-Assert "$RuntimeLabel : smudge filter actually fires on checkout (xray entry injected)" ($contents -match '_xrayMcpMarker')
            }
        }
        finally {
            Pop-Location
        }

        return ($script:localFails -eq 0)
    }
    finally {
        if (-not $KeepTempDir) {
            Remove-Item -Path $stage -Recurse -Force -ErrorAction SilentlyContinue
            Remove-Item -Path $repo -Recurse -Force -ErrorAction SilentlyContinue
            Remove-Item -Path $fakeInstallDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        else {
            Write-Host "  Kept: $stage" -ForegroundColor DarkYellow
            Write-Host "  Kept: $repo" -ForegroundColor DarkYellow
            Write-Host "  Kept: $fakeInstallDir" -ForegroundColor DarkYellow
        }
    }
}

# Round 1: current host runtime.
$currentLabel = "PS $($PSVersionTable.PSVersion)"
$currentExe = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell.exe' }
[void](Invoke-StandaloneInstall -RuntimeLabel $currentLabel -RuntimeExe $currentExe)

# Round 2: cross-runtime. The PS 5.1 install path has its own quoting
# hazard; if we are running under PS 7 AND powershell.exe (5.1) is on
# PATH, install via 5.1 too. Skip when we are already on PS 5.1.
if ($PSVersionTable.PSVersion.Major -ge 6) {
    $ps51 = Get-Command powershell.exe -ErrorAction SilentlyContinue
    if ($ps51) {
        Write-Host ''
        [void](Invoke-StandaloneInstall -RuntimeLabel 'PS 5.1' -RuntimeExe 'powershell.exe')
    }
    else {
        Write-Host ''
        Write-Host 'SKIP  PS 5.1 round (powershell.exe not on PATH)' -ForegroundColor DarkYellow
    }
}
else {
    # Running under PS 5.1 already — try pwsh as the cross-runtime check.
    $pwsh = Get-Command pwsh -ErrorAction SilentlyContinue
    if ($pwsh) {
        Write-Host ''
        [void](Invoke-StandaloneInstall -RuntimeLabel 'PS 7+' -RuntimeExe 'pwsh')
    }
    else {
        Write-Host ''
        Write-Host 'SKIP  PS 7+ round (pwsh not on PATH)' -ForegroundColor DarkYellow
    }
}

Write-Host ''
if ($script:failCount -eq 0) {
    Write-Host 'All standalone-install checks PASSED.' -ForegroundColor Green
    exit 0
}
Write-Host ("standalone-install: $script:failCount FAILED.") -ForegroundColor Red
exit 1
