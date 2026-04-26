# measure-ac4-shared.ps1
#
# AC-4 end-to-end perf measurement for `xray_grep` `lineRegex` mode
# with the literal-trigram prefilter wired in.
#
# Replays the 3 canonical calls from the 2026-04-26 user story
# (`user-story_xray-grep-lineRegex-perf-hints_2026-04-26.md`):
#   1. constant-name regex   `OrgAppTypeId\s*=\s*\d+`           (was 76 362 ms cold)
#   2. constant-name regex   `App\s*=\s*[0-9]+`                  (was 48 136 ms warm)
#   3. OR alternation        `OrgApp.*TypeId|App.*TypeId\s*=\s*\d` (was 76 362 ms cold)
#
# For each call: 3 runs, drop the first (cold-cache), report the median
# of the remaining two so OS file-cache effects are normalised.
#
# Usage:
#   pwsh scripts/measure-ac4-shared.ps1 -Repo C:\path\to\Shared
#   pwsh scripts/measure-ac4-shared.ps1 -Repo . -XrayBin .\target\release\xray.exe
#
# The script does NOT compute speedup vs. a baseline binary on its own —
# build the baseline binary on `main` separately, run this once with each
# binary, and diff the medians manually into
# `docs/measurements/ac4-literal-extraction-bench.md`.
#
# Output: per-call table on stdout, optional JSON dump to disk.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Repo,

    [Parameter()]
    [string]$XrayBin = "xray",

    [Parameter()]
    [string]$Ext = "cs",

    [Parameter()]
    [ValidateRange(3, 20)]
    [int]$Runs = 3,

    [Parameter()]
    [string]$JsonOut,

    [Parameter()]
    [int]$IndexLoadDelaySec = 8
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Repo)) {
    throw "Repo path not found: $Repo"
}
$Repo = (Resolve-Path $Repo).Path

# Verify the binary exists on PATH (or as an explicit path).
$xrayCmd = Get-Command $XrayBin -ErrorAction SilentlyContinue
if (-not $xrayCmd) {
    throw "xray binary not found: '$XrayBin'. Pass -XrayBin <path> or add to PATH."
}
$xrayResolved = $xrayCmd.Source

# Canonical calls from the user story. `id` is the JSON-RPC request id we
# use to correlate the response back to the call label.
$calls = @(
    @{
        Label   = "OrgAppTypeId_constant"
        Term    = 'OrgAppTypeId\s*=\s*\d+'
        Id      = 100
    },
    @{
        Label   = "App_constant"
        Term    = 'App\s*=\s*[0-9]+'
        Id      = 101
    },
    @{
        Label   = "OrgApp_OR_App_typeid"
        # Matches the slowest call from the story: OR alternation that
        # extract_required_literals can shrink to a usable trigram set
        # only via the second branch's literal `App` prefix.
        Term    = 'OrgApp.*TypeId|App.*TypeId\s*=\s*\d'
        Id      = 102
    }
)

# Build one MCP message stream per (call, run) pair and pipe it into a
# single `xray serve` invocation. Reusing the server amortises index load
# across the 3 runs of one call (the cold-cache run is still run #1, but
# index load + handler dispatch warm-up is paid once per call).

function Invoke-LineRegexCall {
    param(
        [hashtable]$Call,
        [int]$Run
    )

    # Each call-run gets its own short-lived `xray serve` so any internal
    # caches from the previous run cannot bias this one. Cost: ~$IndexLoadDelaySec
    # extra seconds per run for index load. Acceptable for a 3-run measurement;
    # for 10+ runs consider a long-lived server with explicit cache flushes.
    $tmpDir = Join-Path $env:TEMP "ac4-measure-$([System.Guid]::NewGuid().ToString('N').Substring(0,8))"
    New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null
    $stdinFile  = Join-Path $tmpDir "stdin.txt"
    $stdoutFile = Join-Path $tmpDir "stdout.json"
    $stderrFile = Join-Path $tmpDir "stderr.txt"

    $argJson = ConvertTo-Json -Compress -Depth 4 @{
        terms     = @($Call.Term)
        regex     = $true
        lineRegex = $true
    }
    $callMsg = ConvertTo-Json -Compress -Depth 6 @{
        jsonrpc = "2.0"
        id      = $Call.Id
        method  = "tools/call"
        params  = @{
            name      = "xray_grep"
            arguments = (ConvertFrom-Json $argJson)
        }
    }

    $helper = Join-Path $tmpDir "send.ps1"
    @"
'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ac4-measure","version":"1.0"}}}'
'{"jsonrpc":"2.0","method":"notifications/initialized"}'
Start-Sleep -Seconds $IndexLoadDelaySec
'$($callMsg -replace "'", "''")'
Start-Sleep -Seconds 2
"@ | Out-File $helper -Encoding utf8

    & cmd /c "powershell -NoProfile -File `"$helper`" | `"$xrayResolved`" serve --dir `"$Repo`" --ext $Ext 1>`"$stdoutFile`" 2>`"$stderrFile`""

    $responses = Get-Content $stdoutFile -ErrorAction SilentlyContinue | ForEach-Object {
        try { $_ | ConvertFrom-Json } catch { $null }
    } | Where-Object { $_ -ne $null -and $_.id -eq $Call.Id }

    if (-not $responses) {
        Write-Host "  ERROR: no JSON-RPC response for id=$($Call.Id) on run $Run" -ForegroundColor Red
        if (Test-Path $stderrFile) {
            Get-Content $stderrFile | Select-Object -Last 5 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        }
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }

    $rpc = $responses[0]
    if ($rpc.PSObject.Properties.Match('error').Count -gt 0 -and $null -ne $rpc.error) {
        Write-Host ("  ERROR: JSON-RPC error for id={0} on run {1}: {2}" -f $Call.Id, $Run, $rpc.error.message) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }
    if ($null -eq $rpc.result -or $null -eq $rpc.result.content -or $rpc.result.content.Count -eq 0) {
        Write-Host ("  ERROR: malformed result envelope for id={0} on run {1}" -f $Call.Id, $Run) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }
    if ($rpc.result.isError -eq $true) {
        $errText = $rpc.result.content[0].text
        Write-Host ("  ERROR: tool returned isError=true for id={0} on run {1}: {2}" -f $Call.Id, $Run, $errText) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }

    $payload = $rpc.result.content[0].text | ConvertFrom-Json
    $summary = $payload.summary
    if ($null -eq $summary) {
        Write-Host ("  ERROR: response has no summary block (id={0} run {1})" -f $Call.Id, $Run) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }

    # The grep summary uses `searchTimeMs` and `indexFiles` (NOT
    # `searchElapsedMs` / `indexedFiles` — those names belonged to an
    # earlier draft of this script and silently coerced to 0 on every run).
    # See `build_grep_base_summary` in src/mcp/handlers/grep.rs.
    if ($null -eq $summary.searchTimeMs) {
        Write-Host ("  ERROR: summary missing 'searchTimeMs' (id={0} run {1}); is this a pre-AC-4 binary?" -f $Call.Id, $Run) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }
    if ($null -eq $summary.literalPrefilter) {
        Write-Host ("  ERROR: summary missing 'literalPrefilter' (id={0} run {1}); binary is pre-AC-4 or regression — measurement aborted." -f $Call.Id, $Run) -ForegroundColor Red
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        return $null
    }

    $result = [ordered]@{
        Run                 = $Run
        Label               = $Call.Label
        Pattern             = $Call.Term
        SearchMode          = $summary.searchMode
        SearchTimeMs        = [int][math]::Round([double]$summary.searchTimeMs)
        TotalFiles          = [int]$summary.totalFiles
        TotalOccurrences    = [int]$summary.totalOccurrences
        IndexFiles          = [int]$summary.indexFiles
        PrefilterUsed       = [bool]$summary.literalPrefilter.used
        PrefilterCandidates = [int]$summary.literalPrefilter.candidateFiles
        PrefilterReason     = [string]$summary.literalPrefilter.reason
        PerfHintFired       = [bool]($null -ne $summary.perfHint)
    }

    Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    return [pscustomobject]$result
}

Write-Host "AC-4 measurement: repo=$Repo, ext=$Ext, runs/call=$Runs (drop run #1, median of remaining)" -ForegroundColor Cyan
Write-Host "Binary: $xrayResolved" -ForegroundColor DarkGray

$allRows = @()
$summaries = @()

foreach ($call in $calls) {
    Write-Host ""
    Write-Host "── $($call.Label) :: $($call.Term)" -ForegroundColor Yellow

    $rows = @()
    for ($r = 1; $r -le $Runs; $r++) {
        Write-Host "  run $r/$Runs ..." -NoNewline
        $row = Invoke-LineRegexCall -Call $call -Run $r
        if ($row) {
            $rows += $row
            Write-Host (" {0,7} ms  ({1} files, prefilterUsed={2})" -f $row.SearchTimeMs, $row.TotalFiles, $row.PrefilterUsed) -ForegroundColor Green
        } else {
            Write-Host " FAILED" -ForegroundColor Red
        }
    }

    $allRows += $rows

    if ($rows.Count -lt 2) {
        Write-Host "  not enough successful runs to compute median (need ≥2 after dropping cold)" -ForegroundColor Red
        continue
    }

    # Drop run #1 (cold-cache), median of the rest.
    $warm = $rows | Where-Object { $_.Run -gt 1 } | Sort-Object SearchTimeMs
    $mid  = [math]::Floor($warm.Count / 2)
    $medianMs = if ($warm.Count % 2 -eq 1) {
        $warm[$mid].SearchTimeMs
    } else {
        [math]::Round((($warm[$mid - 1].SearchTimeMs + $warm[$mid].SearchTimeMs) / 2))
    }

    $summary = [pscustomobject]@{
        Label              = $call.Label
        Pattern            = $call.Term
        ColdMs             = $rows[0].SearchTimeMs
        WarmMedianMs       = $medianMs
        TotalFiles         = $rows[0].TotalFiles
        TotalOccurrences   = $rows[0].TotalOccurrences
        IndexFiles         = $rows[0].IndexFiles
        PrefilterUsed      = $rows[0].PrefilterUsed
        PrefilterCandidates= $rows[0].PrefilterCandidates
        PrefilterReason    = $rows[0].PrefilterReason
        PerfHintFired      = $rows[0].PerfHintFired
    }
    $summaries += $summary
}

Write-Host ""
Write-Host "── Summary (median of warm runs)" -ForegroundColor Cyan
$summaries | Format-Table -AutoSize Label, ColdMs, WarmMedianMs, TotalFiles, TotalOccurrences, PrefilterUsed, PrefilterCandidates, PerfHintFired

if ($JsonOut) {
    $bundle = [ordered]@{
        repo            = $Repo
        ext             = $Ext
        binary          = $xrayResolved
        runsPerCall     = $Runs
        indexLoadDelay  = $IndexLoadDelaySec
        timestampUtc    = (Get-Date).ToUniversalTime().ToString("o")
        rawRows         = $allRows
        summaries       = $summaries
    }
    $bundle | ConvertTo-Json -Depth 8 | Out-File $JsonOut -Encoding utf8
    Write-Host "Raw + summary written to: $JsonOut" -ForegroundColor DarkGray
}
