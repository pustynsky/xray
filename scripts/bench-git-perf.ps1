# bench-git-perf.ps1
#
# PERF-AUDIT-2026-04-24 git-bound benchmarks. Measures the spawn-bound
# code paths that we deliberately keep OUT of criterion (criterion +
# `git` subprocess = high-variance noise + repo-state coupling we don't
# want baked into the harness).
#
# Targets:
#   PERF-02 — `detect_main_branch_name` (up to 4× `git rev-parse`)
#   PERF-03 — `get_commit_diff` (parent-probe + diff per commit)
#   PERF-09 — `parse_blame_porcelain` (blame + interning)
#   PERF-04 — `top_authors` (50k commit aggregation)
#
# Usage:
#   pwsh scripts/bench-git-perf.ps1 -Repo C:\path\to\real\repo
#   pwsh scripts/bench-git-perf.ps1 -Repo . -SaveBaseline pre-perf-audit
#
# Output: prints a one-line summary per bench plus optional JSON dump
# under `target/git-perf/<baseline>.json` for diffing across commits.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Repo,

    [Parameter()]
    [string]$XrayBin = "xray",

    [Parameter()]
    [ValidateRange(1, [int]::MaxValue)]
    [int]$Iterations = 5,

    [Parameter()]
    [string]$SaveBaseline,

    [Parameter()]
    [string]$BlameFile,

    [Parameter()]
    [string]$HistoryFile
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Repo)) {
    throw "Repo path not found: $Repo"
}
$Repo = (Resolve-Path $Repo).Path

function Measure-Cmd {
    param(
        [string]$Name,
        [scriptblock]$Block,
        [int]$Iterations = 5
    )
    if ($Iterations -lt 1) {
        throw "Iterations must be >= 1 (got $Iterations) — a 0-iteration measurement is meaningless"
    }
    # Warm-up: prime FS cache + git internal caches so we measure steady state.
    & $Block | Out-Null
    $samples = @()
    for ($i = 0; $i -lt $Iterations; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $Block | Out-Null
        $sw.Stop()
        $samples += $sw.Elapsed.TotalMilliseconds
    }
    $sorted = @($samples | Sort-Object)
    # Classical median: average of the two middle elements for even N,
    # middle element for odd N. The previous one-line `sorted[Count/2]`
    # picked the upper-middle for even N (e.g. 4 samples returned the 3rd,
    # not the average of 2nd+3rd) which biased reported medians upward.
    $n = $sorted.Count
    if ($n % 2 -eq 1) {
        $median = $sorted[[int](($n - 1) / 2)]
    } else {
        $median = ($sorted[$n / 2 - 1] + $sorted[$n / 2]) / 2.0
    }
    $min = $sorted[0]
    $max = $sorted[-1]
    [PSCustomObject]@{
        bench      = $Name
        median_ms  = [math]::Round($median, 2)
        min_ms     = [math]::Round($min, 2)
        max_ms     = [math]::Round($max, 2)
        iterations = $Iterations
    }
}

$results = @()

# ─── PERF-02: detect_main_branch_name ─────────────────────────────────
# Reproduces the worst case (4 sequential rev-parse calls) by directly
# invoking git the same way `detect_main_branch_name` does.
$results += Measure-Cmd -Name "PERF-02 detect_main_branch (4x rev-parse worst case)" -Iterations $Iterations -Block {
    git -C $Repo rev-parse --verify main 2>$null | Out-Null
    git -C $Repo rev-parse --verify refs/remotes/origin/main 2>$null | Out-Null
    git -C $Repo rev-parse --verify master 2>$null | Out-Null
    git -C $Repo rev-parse --verify refs/remotes/origin/master 2>$null | Out-Null
}

# Best-case (1 spawn) for reference — what PERF-02's combined for-each-ref would cost.
$results += Measure-Cmd -Name "PERF-02 best-case (1x for-each-ref combined)" -Iterations $Iterations -Block {
    git -C $Repo for-each-ref --format='%(refname:short)' `
        refs/heads/main refs/heads/master `
        refs/remotes/origin/main refs/remotes/origin/master 2>$null | Out-Null
}

# ─── PERF-03: get_commit_diff ─────────────────────────────────────────
# Pick the last 50 commits touching a representative file to amortise.
if (-not $HistoryFile) {
    $HistoryFile = (git -C $Repo ls-tree -r HEAD --name-only | Select-Object -First 1)
}
if ($HistoryFile) {
    Write-Host "PERF-03 using file: $HistoryFile"
    $hashes = git -C $Repo log --format=%H --max-count=50 -- $HistoryFile
    $hashList = @($hashes)

    $results += Measure-Cmd -Name "PERF-03 get_commit_diff old (rev-parse + diff per commit, 50 commits)" -Iterations $Iterations -Block {
        foreach ($h in $hashList) {
            git -C $Repo rev-parse --verify "$h^" 2>$null | Out-Null
            git -C $Repo diff "$h^..$h" -- $HistoryFile 2>$null | Out-Null
        }
    }

    $results += Measure-Cmd -Name "PERF-03 get_commit_diff new (git show only, 50 commits)" -Iterations $Iterations -Block {
        foreach ($h in $hashList) {
            git -C $Repo show $h --format= --patch -- $HistoryFile 2>$null | Out-Null
        }
    }
} else {
    Write-Warning "PERF-03 skipped: no file found in repo HEAD"
}

# ─── PERF-09: blame ──────────────────────────────────────────────────
if (-not $BlameFile) {
    # Pick the largest indexed file (rough heuristic for "5k lines, many authors").
    $BlameFile = git -C $Repo ls-tree -r HEAD --name-only | ForEach-Object {
        $p = Join-Path $Repo $_
        if (Test-Path $p -PathType Leaf) {
            [PSCustomObject]@{ path = $_; lines = (Get-Content $p -ErrorAction SilentlyContinue | Measure-Object -Line).Lines }
        }
    } | Sort-Object lines -Descending | Select-Object -First 1 -ExpandProperty path
}
if ($BlameFile) {
    Write-Host "PERF-09 using file: $BlameFile"
    $results += Measure-Cmd -Name "PERF-09 git blame --porcelain (large file)" -Iterations $Iterations -Block {
        git -C $Repo blame --porcelain $BlameFile 2>$null | Out-Null
    }
} else {
    Write-Warning "PERF-09 skipped: no file selected"
}

# ─── PERF-04: top_authors ────────────────────────────────────────────
$results += Measure-Cmd -Name "PERF-04 top_authors raw git log (max 50k commits)" -Iterations $Iterations -Block {
    git -C $Repo log --format=%H --max-count=50000 2>$null | Out-Null
}

# ─── Output ──────────────────────────────────────────────────────────
Write-Host ""
Write-Host "=== git-perf results (median of $Iterations iterations, repo=$Repo) ==="
$results | Format-Table -AutoSize

if ($SaveBaseline) {
    $outDir = Join-Path (Get-Location) "target\git-perf"
    if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }
    $outFile = Join-Path $outDir "$SaveBaseline.json"
    $results | ConvertTo-Json -Depth 4 | Set-Content -Path $outFile -Encoding UTF8
    Write-Host "Saved baseline to $outFile"
}
