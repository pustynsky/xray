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
# Each test below rewrites snapshot.txt with the shape that matches the
# fixture being tested (Copilot CLI shape vs VS Code shape).
$snapshotPath = Join-Path $here 'snapshot.txt'
$snapshotCopilotCli = '    "xray":{"command":"C:\\Tools\\xray\\xray.exe","args":["mcp"],"env":{},"_xrayMcpMarker":"managed-by-setup-xray.ps1-do-not-edit"}'
$snapshotVsCode     = '    "xray":{"type":"stdio","command":"C:\\Tools\\xray\\xray.exe","args":["mcp"],"_xrayMcpMarker":"managed-by-setup-xray.ps1-do-not-edit"}'
# Initial value: Copilot CLI shape, matching the default-key smudge call.
[IO.File]::WriteAllText($snapshotPath, $snapshotCopilotCli, [Text.UTF8Encoding]::new($false))

function Invoke-Bash {
    param(
        [string]$ScriptPath,
        [string]$StdinText,
        [string[]]$BashArgs = @()
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
        # NOTE: parameter name is `BashArgs` (not `Args`) because `$Args` is
        # an automatic variable in PowerShell that holds the function's
        # unbound positional arguments. Naming our parameter `Args` causes
        # the bound array to silently drop on entry, which previously made
        # all `-Args @('servers')` calls behave as zero-args (default-key)
        # and broke the entire VS Code container key test.
        $argsPart = ''
        foreach ($a in $BashArgs) {
            # Quote each arg defensively. The args we pass are short
            # alphanumeric tokens (container keys), so simple double-quote
            # wrapping is sufficient.
            $argsPart += " `"$a`""
        }
        $bashCmd = "`"$BashExe`" `"$shellPath`"$argsPart < `"$tmpIn`" > `"$tmpOut`""
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
# (Inject = $true) or passthrough (Inject = $false), and the container key
# the smudge filter should be invoked with (defaults to "mcpServers").
$fixtureExpectations = [ordered]@{
    '01-empty-multiline.canonical.json'              = @{ Inject = $true;  ContainerKey = 'mcpServers'; Snapshot = $snapshotCopilotCli }
    '02-single-server.canonical.json'                = @{ Inject = $true;  ContainerKey = 'mcpServers'; Snapshot = $snapshotCopilotCli }
    '03-multi-server-with-braces-in-args.canonical.json' = @{ Inject = $true;  ContainerKey = 'mcpServers'; Snapshot = $snapshotCopilotCli }
    '04-empty-inline.canonical.json'                 = @{ Inject = $false; ContainerKey = 'mcpServers'; Snapshot = $snapshotCopilotCli }  # inline {} -> passthrough
    '05-with-other-top-level-keys.canonical.json'    = @{ Inject = $true;  ContainerKey = 'mcpServers'; Snapshot = $snapshotCopilotCli }
    '06-vscode-servers.canonical.json'               = @{ Inject = $true;  ContainerKey = 'servers';    Snapshot = $snapshotVsCode    }
    '07-vscode-empty-servers.canonical.json'         = @{ Inject = $true;  ContainerKey = 'servers';    Snapshot = $snapshotVsCode    }
}

# Synthesize a CRLF version of fixture 02 in-memory to verify byte-exact
# round-trip preserves CRLF (not just LF). Uses the Copilot CLI snapshot
# shape and the default "mcpServers" container key.
$crlfCanonical = ([IO.File]::ReadAllText((Join-Path $fixtures '02-single-server.canonical.json'), [Text.UTF8Encoding]::new($false))) -replace "`n", "`r`n"
# Synthesize a CRLF VS Code fixture to verify byte-exact round-trip with
# the "servers" container key + VS Code snapshot shape on CRLF input.
$crlfVsCodeCanonical = ([IO.File]::ReadAllText((Join-Path $fixtures '06-vscode-servers.canonical.json'), [Text.UTF8Encoding]::new($false))) -replace "`n", "`r`n"

$failures = @()

foreach ($fxName in ($fixtureExpectations.Keys | Sort-Object)) {
    $fxPath = Join-Path $fixtures $fxName
    if (-not (Test-Path $fxPath)) {
        $failures += "$fxName : fixture file missing"
        continue
    }

    $canonical = [IO.File]::ReadAllText($fxPath, [Text.UTF8Encoding]::new($false))
    $expectInject = $fixtureExpectations[$fxName].Inject
    $containerKey = $fixtureExpectations[$fxName].ContainerKey
    $fxSnapshot = $fixtureExpectations[$fxName].Snapshot

    # Rewrite snapshot.txt to the shape this fixture expects, so smudge
    # injects the right entry shape (Copilot CLI vs VS Code).
    [IO.File]::WriteAllText($snapshotPath, $fxSnapshot, [Text.UTF8Encoding]::new($false))

    # Step 1: smudge with the per-fixture container key. The default key
    # path is also exercised by passing 'mcpServers' explicitly here, which
    # routes through the same `${1:-mcpServers}` parser branch in smudge.sh
    # (the no-args branch is covered by xray-mcp installs from before this
    # change).
    $enriched = Invoke-Bash -ScriptPath $smudge -StdinText $canonical -BashArgs @($containerKey)
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
        $smudgedTwice = Invoke-Bash -ScriptPath $smudge -StdinText $enriched -BashArgs @($containerKey)
        if ($smudgedTwice -ne $enriched) {
            $failures += "$fxName : smudge is not idempotent on already-enriched input."
            continue
        }
    }

    Write-Host ("PASS  {0}  (container=`"{1}`")" -f $fxName, $containerKey) -ForegroundColor Green
}

# Extra 1: byte-exact CRLF round-trip on the Copilot CLI fixture (cannot be a
# fixture file because git/editors might re-normalize line endings).
[IO.File]::WriteAllText($snapshotPath, $snapshotCopilotCli, [Text.UTF8Encoding]::new($false))
$crlfEnriched = Invoke-Bash -ScriptPath $smudge -StdinText $crlfCanonical -BashArgs @('mcpServers')
if ($crlfEnriched -notmatch '_xrayMcpMarker') {
    $failures += 'CRLF (mcpServers): smudge did not inject marker'
}
elseif ($crlfEnriched -notmatch "`r`n") {
    $failures += 'CRLF (mcpServers): smudge dropped CR characters'
}
else {
    $crlfRecovered = Invoke-Bash -ScriptPath $clean -StdinText $crlfEnriched
    if ($crlfRecovered -ne $crlfCanonical) {
        $canBytes = [Text.Encoding]::UTF8.GetBytes($crlfCanonical)
        $recBytes = [Text.Encoding]::UTF8.GetBytes($crlfRecovered)
        $failures += "CRLF (mcpServers): round-trip mismatch (canonical=$($canBytes.Length) recovered=$($recBytes.Length) bytes)"
    }
    else {
        Write-Host 'PASS  CRLF byte-exact round-trip (mcpServers)' -ForegroundColor Green
    }
}

# Extra 2: byte-exact CRLF round-trip on the VS Code fixture, with the
# 'servers' container key + VS Code snapshot shape.
[IO.File]::WriteAllText($snapshotPath, $snapshotVsCode, [Text.UTF8Encoding]::new($false))
$crlfVsEnriched = Invoke-Bash -ScriptPath $smudge -StdinText $crlfVsCodeCanonical -BashArgs @('servers')
if ($crlfVsEnriched -notmatch '_xrayMcpMarker') {
    $failures += 'CRLF (servers): smudge did not inject marker'
}
elseif ($crlfVsEnriched -notmatch "`r`n") {
    $failures += 'CRLF (servers): smudge dropped CR characters'
}
else {
    $crlfVsRecovered = Invoke-Bash -ScriptPath $clean -StdinText $crlfVsEnriched
    if ($crlfVsRecovered -ne $crlfVsCodeCanonical) {
        $canBytes = [Text.Encoding]::UTF8.GetBytes($crlfVsCodeCanonical)
        $recBytes = [Text.Encoding]::UTF8.GetBytes($crlfVsRecovered)
        $failures += "CRLF (servers): round-trip mismatch (canonical=$($canBytes.Length) recovered=$($recBytes.Length) bytes)"
    }
    else {
        Write-Host 'PASS  CRLF byte-exact round-trip (servers)' -ForegroundColor Green
    }
}

# Extra 3: backward-compat. The pre-vscode-extension installs invoked the
# smudge filter with NO arguments. The default key path must continue to
# inject into 'mcpServers' just like the explicit 'mcpServers' arg does.
[IO.File]::WriteAllText($snapshotPath, $snapshotCopilotCli, [Text.UTF8Encoding]::new($false))
$noargCanonical = [IO.File]::ReadAllText((Join-Path $fixtures '02-single-server.canonical.json'), [Text.UTF8Encoding]::new($false))
$noargEnriched = Invoke-Bash -ScriptPath $smudge -StdinText $noargCanonical -BashArgs @()
if ($noargEnriched -notmatch '_xrayMcpMarker') {
    $failures += 'Backward-compat (no args): smudge did not inject marker; the no-args default branch may be broken.'
}
else {
    $noargRecovered = Invoke-Bash -ScriptPath $clean -StdinText $noargEnriched
    if ($noargRecovered -ne $noargCanonical) {
        $failures += 'Backward-compat (no args): round-trip mismatch.'
    }
    else {
        Write-Host 'PASS  Backward-compat (no args defaults to mcpServers)' -ForegroundColor Green
    }
}

# Extra 4: invalid container key must degrade to passthrough, not crash
# git checkout. We pass a key with a slash in it; the bash validation
# regex /^[A-Za-z_][A-Za-z0-9_]*$/ should reject it and `exec cat` should
# pass the canonical input through byte-identical, leaving NO marker.
[IO.File]::WriteAllText($snapshotPath, $snapshotCopilotCli, [Text.UTF8Encoding]::new($false))
$badKeyEnriched = Invoke-Bash -ScriptPath $smudge -StdinText $noargCanonical -BashArgs @('servers/evil')
if ($badKeyEnriched -ne $noargCanonical) {
    $failures += 'Invalid container key: smudge mutated the input instead of passthrough (validation regex may be wrong).'
}
else {
    Write-Host 'PASS  Invalid container key -> passthrough' -ForegroundColor Green
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
