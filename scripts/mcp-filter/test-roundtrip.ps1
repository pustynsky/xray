#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Verifies the round-trip property of the smudge/clean filter pair.

.DESCRIPTION
    For each *.canonical.json fixture in fixtures/:
      1. Run smudge.sh on the canonical input        -> enriched output.
      2. Verify enriched output contains the marker (or matches expected
         passthrough behavior for fixtures that smudge cannot inject into).
      3. Run clean.sh on the enriched output         -> recovered output.
      4. Assert recovered == canonical (byte-identical).

    Also asserts that clean.sh is idempotent (clean(clean(x)) == clean(x))
    and that smudge.sh is idempotent against an already-enriched input
    (smudge(smudge(x)) == smudge(x)).

    Bash is required. On Windows, Git for Windows ships bash at
    'C:\Program Files\Git\usr\bin\bash.exe'. The script auto-locates it.

.EXAMPLE
    .\test-roundtrip.ps1
#>
param(
    [string]$BashExe
)

$ErrorActionPreference = 'Stop'

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$smudge = Join-Path $here 'smudge.sh'
$clean = Join-Path $here 'clean.sh'
$fixtures = Join-Path $here 'fixtures'

if (-not $BashExe) {
    $candidates = @(
        'C:\Program Files\Git\usr\bin\bash.exe',
        'C:\Program Files\Git\bin\bash.exe',
        'bash'
    )
    foreach ($c in $candidates) {
        $cmd = Get-Command $c -ErrorAction SilentlyContinue
        if ($cmd) { $BashExe = $cmd.Source; break }
    }
}

if (-not $BashExe -or -not (Test-Path $BashExe)) {
    Write-Error 'bash not found. Install Git for Windows or pass -BashExe.'
    exit 1
}

# Git's POSIX utilities (dirname, cat, awk, sed) live next to bash.exe.
# When git itself invokes a smudge filter, it prepends this directory to
# PATH automatically. We replicate that here so direct invocation works.
$bashBinDir = Split-Path -Parent $BashExe
if ($env:PATH -notlike "*$bashBinDir*") {
    $env:PATH = "$bashBinDir;$env:PATH"
}

# Build a snapshot.txt next to smudge.sh for the duration of the test.
# The smudge script reads the snapshot from $(dirname "$0")/snapshot.txt.
$snapshotPath = Join-Path $here 'snapshot.txt'
$snapshotLine = '    "xray":{"command":"C:\\Tools\\xray\\xray.exe","args":["mcp"],"env":{},"_xrayMcpMarker":"managed-by-setup-xray.ps1-do-not-edit"}'
[IO.File]::WriteAllText($snapshotPath, $snapshotLine, [Text.UTF8Encoding]::new($false))

function Invoke-Bash {
    param(
        [string]$ScriptPath,
        [string]$StdinText
    )

    $tmpIn = [IO.Path]::GetTempFileName()
    $tmpOut = [IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllText($tmpIn, $StdinText, [Text.UTF8Encoding]::new($false))

        # Invoke bash.exe directly with the script path. The script does not
        # depend on /usr/bin/env (it's bash-only), and we redirect IO via
        # cmd.exe to avoid PowerShell's pipeline mangling our bytes.
        $shellPath = $ScriptPath -replace '\\', '/'
        $tmpInPath = $tmpIn -replace '\\', '/'
        $tmpOutPath = $tmpOut -replace '\\', '/'
        $bashCmd = "`"$BashExe`" `"$shellPath`" < `"$tmpIn`" > `"$tmpOut`""
        & cmd.exe /c $bashCmd
        if ($LASTEXITCODE -ne 0) {
            throw "bash exited $LASTEXITCODE running $ScriptPath"
        }
        return [IO.File]::ReadAllText($tmpOut, [Text.UTF8Encoding]::new($false))
    }
    finally {
        Remove-Item -Path $tmpIn -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $tmpOut -Force -ErrorAction SilentlyContinue
    }
}

# Each fixture entry: name, canonical text, whether smudge should inject
# (expected = $true) or passthrough (expected = $false).
$fixtureExpectations = @{
    '01-empty-multiline.canonical.json'              = @{ Inject = $true }
    '02-single-server.canonical.json'                = @{ Inject = $true }
    '03-multi-server-with-braces-in-args.canonical.json' = @{ Inject = $true }
    '04-empty-inline.canonical.json'                 = @{ Inject = $false }  # inline {} -> passthrough
    '05-with-other-top-level-keys.canonical.json'    = @{ Inject = $true }
}

# Synthesize a CRLF version of fixture 02 in-memory to verify byte-exact
# round-trip preserves CRLF (not just LF).
$crlfCanonical = ([IO.File]::ReadAllText((Join-Path $fixtures '02-single-server.canonical.json'), [Text.UTF8Encoding]::new($false))) -replace "`n", "`r`n"

$failures = @()

foreach ($fxName in ($fixtureExpectations.Keys | Sort-Object)) {
    $fxPath = Join-Path $fixtures $fxName
    if (-not (Test-Path $fxPath)) {
        $failures += "$fxName : fixture file missing"
        continue
    }

    $canonical = [IO.File]::ReadAllText($fxPath, [Text.UTF8Encoding]::new($false))
    $expectInject = $fixtureExpectations[$fxName].Inject

    # Step 1: smudge.
    $enriched = Invoke-Bash -ScriptPath $smudge -StdinText $canonical
    $hasMarker = $enriched -match '_xrayMcpMarker'

    if ($expectInject -and -not $hasMarker) {
        $failures += "$fxName : expected smudge to inject marker but it did not. Output:`n$enriched"
        continue
    }
    if (-not $expectInject -and $hasMarker) {
        $failures += "$fxName : expected smudge to passthrough but it injected. Output:`n$enriched"
        continue
    }

    # Step 2: clean(enriched) must equal canonical (byte-identical).
    $recovered = Invoke-Bash -ScriptPath $clean -StdinText $enriched
    if ($recovered -ne $canonical) {
        $canBytes = [Text.Encoding]::UTF8.GetBytes($canonical)
        $recBytes = [Text.Encoding]::UTF8.GetBytes($recovered)
        $diffIdx = -1
        for ($i = 0; $i -lt [Math]::Min($canBytes.Length, $recBytes.Length); $i++) {
            if ($canBytes[$i] -ne $recBytes[$i]) { $diffIdx = $i; break }
        }
        $hint = "lengths: canonical=$($canBytes.Length) recovered=$($recBytes.Length); first byte diff at index=$diffIdx"
        if ($diffIdx -ge 0) {
            $ctxStart = [Math]::Max(0, $diffIdx - 5)
            $ctxEnd   = [Math]::Min($canBytes.Length - 1, $diffIdx + 5)
            $hint += "; canonical[$ctxStart..$ctxEnd]= " + (($canBytes[$ctxStart..$ctxEnd] | ForEach-Object { $_.ToString('X2') }) -join ' ')
            $ctxEndR  = [Math]::Min($recBytes.Length - 1, $diffIdx + 5)
            $hint += "; recovered[$ctxStart..$ctxEndR]= " + (($recBytes[$ctxStart..$ctxEndR] | ForEach-Object { $_.ToString('X2') }) -join ' ')
        }
        $failures += "$fxName : round-trip mismatch. $hint"
        continue
    }

    # Step 3: clean idempotency: clean(clean(x)) == clean(x).
    $cleanedTwice = Invoke-Bash -ScriptPath $clean -StdinText $recovered
    if ($cleanedTwice -ne $recovered) {
        $failures += "$fxName : clean is not idempotent. clean(clean(x)) != clean(x)."
        continue
    }

    # Step 4: smudge idempotency on already-enriched: smudge(smudge(x)) == smudge(x).
    if ($expectInject) {
        $smudgedTwice = Invoke-Bash -ScriptPath $smudge -StdinText $enriched
        if ($smudgedTwice -ne $enriched) {
            $failures += "$fxName : smudge is not idempotent on already-enriched input."
            continue
        }
    }

    Write-Host ("PASS  {0}" -f $fxName) -ForegroundColor Green
}

# Extra: byte-exact CRLF round-trip (cannot be a fixture file because git/editors
# might re-normalize line endings).
$crlfEnriched = Invoke-Bash -ScriptPath $smudge -StdinText $crlfCanonical
if ($crlfEnriched -notmatch '_xrayMcpMarker') {
    $failures += 'CRLF: smudge did not inject marker'
}
elseif ($crlfEnriched -notmatch "`r`n") {
    $failures += 'CRLF: smudge dropped CR characters'
}
else {
    $crlfRecovered = Invoke-Bash -ScriptPath $clean -StdinText $crlfEnriched
    if ($crlfRecovered -ne $crlfCanonical) {
        $canBytes = [Text.Encoding]::UTF8.GetBytes($crlfCanonical)
        $recBytes = [Text.Encoding]::UTF8.GetBytes($crlfRecovered)
        $failures += "CRLF: round-trip mismatch (canonical=$($canBytes.Length) recovered=$($recBytes.Length) bytes)"
    }
    else {
        Write-Host 'PASS  CRLF byte-exact round-trip' -ForegroundColor Green
    }
}

# Cleanup snapshot file so it doesn't leak into the repo.
Remove-Item -Path $snapshotPath -Force -ErrorAction SilentlyContinue

if ($failures.Count -gt 0) {
    Write-Host ''
    Write-Host '=== FAILURES ===' -ForegroundColor Red
    foreach ($f in $failures) {
        Write-Host $f -ForegroundColor Red
        Write-Host ''
    }
    exit 1
}

Write-Host ''
Write-Host 'All round-trip tests passed.' -ForegroundColor Cyan
exit 0
