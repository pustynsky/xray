#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression tests for Get-DetectedExtensions in setup-xray.ps1.

.DESCRIPTION
    Loads the production Get-DetectedExtensions function via AST extraction
    (the script has top-level imperative code, so plain dot-source is not
    possible) and runs it against fixture trees that exercise the
    invariants the function must preserve:

      * Skip-set membership pruned BEFORE descending (prune-at-boundary,
        not prune-after-recurse). Verified via case-insensitive match
        on directory leaves.
      * Reparse points (junctions / symlinks / mount points) NOT followed.
        Verified by creating a junction to a "poison" tree and asserting
        none of the poison files appear in the result.
      * Hidden files ARE counted (matches the previous Get-ChildItem -Force
        behavior).
      * Files with no extension OR a bare-dot extension are NOT tallied.
      * Unknown extensions are NOT tallied.
      * Known extensions are tallied with the correct count.

    The test does NOT measure performance; perf characteristics are
    covered by the dev-time harness in target/tmp/bench-scan.ps1 (not
    committed, recreated on demand).

    Exits 0 on all-pass, 1 on any failure.
#>

$ErrorActionPreference = 'Stop'
$Global:_failures = 0
$Global:_passes = 0

function Assert-Equal {
    param([Parameter(Mandatory)] $Actual, [Parameter(Mandatory)] $Expected, [Parameter(Mandatory)] [string]$Label)
    if ($Expected -is [System.Collections.IDictionary] -or $Expected -is [hashtable]) {
        $a = ($Actual.GetEnumerator() | Sort-Object Key | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ','
        $e = ($Expected.GetEnumerator() | Sort-Object Key | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ','
        if ($a -eq $e) {
            Write-Host "PASS  $Label" -ForegroundColor Green
            $Global:_passes++
        }
        else {
            Write-Host "FAIL  $Label" -ForegroundColor Red
            Write-Host "  expected: $e" -ForegroundColor Red
            Write-Host "  actual:   $a" -ForegroundColor Red
            $Global:_failures++
        }
    }
    else {
        if ($Actual -eq $Expected) {
            Write-Host "PASS  $Label" -ForegroundColor Green
            $Global:_passes++
        }
        else {
            Write-Host "FAIL  $Label" -ForegroundColor Red
            Write-Host "  expected: $Expected" -ForegroundColor Red
            Write-Host "  actual:   $Actual" -ForegroundColor Red
            $Global:_failures++
        }
    }
}

function New-TempDir {
    $d = Join-Path ([IO.Path]::GetTempPath()) ('xray-detect-test-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $d -Force | Out-Null
    return $d
}

function Remove-TempDir {
    param([string]$Path)
    if (Test-Path $Path) {
        # Junctions: remove via Remove-Item -Force, NOT recursive — recursive
        # delete on a junction can wipe the target. Walk top-level entries
        # first and unlink junctions before recursing.
        Get-ChildItem $Path -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                $attrs = [IO.File]::GetAttributes($_.FullName)
                if ($_.PSIsContainer -and ($attrs -band [IO.FileAttributes]::ReparsePoint)) {
                    # Remove the junction without following it.
                    [IO.Directory]::Delete($_.FullName, $false)
                }
            }
            catch { }
        }
        Remove-Item -Path $Path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Load Get-DetectedExtensions from production setup-xray.ps1 via AST.
$scriptPath = Join-Path (Split-Path -Parent $PSCommandPath) 'setup-xray.ps1'
if (-not (Test-Path $scriptPath)) {
    Write-Error "setup-xray.ps1 not found next to this test: $scriptPath"
    exit 1
}
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors -and $parseErrors.Count -gt 0) {
    Write-Error "Parse errors in setup-xray.ps1: $($parseErrors -join '; ')"
    exit 1
}
$funcAst = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-DetectedExtensions' }, $true)
if (-not $funcAst) {
    Write-Error "Get-DetectedExtensions not found in $scriptPath"
    exit 1
}
. ([scriptblock]::Create($funcAst.Extent.Text))

# Fixture knowns and skips for tests below.
$KnownExt = @{ 'cs' = 'C#'; 'rs' = 'Rust'; 'md' = 'MD'; 'ps1' = 'PS' }
$SkipDirs = @('.git', 'node_modules', 'target', 'bin', 'obj')

# ---------------------------------------------------------------
# Test 1: basic count of known extensions, ignoring unknown ones.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'src') -Force | Out-Null
    'fn main() {}' | Set-Content (Join-Path $root 'src\main.rs') -Encoding UTF8
    'pub mod x;' | Set-Content (Join-Path $root 'src\lib.rs') -Encoding UTF8
    '# Title' | Set-Content (Join-Path $root 'README.md') -Encoding UTF8
    '<svg/>' | Set-Content (Join-Path $root 'logo.svg') -Encoding UTF8     # unknown ext
    'no ext at all' | Set-Content (Join-Path $root 'LICENSE') -Encoding UTF8   # no ext
    'bare dot' | Set-Content (Join-Path $root 'oddfile.') -Encoding UTF8        # bare dot
    '.dotfile content' | Set-Content (Join-Path $root '.dotfile') -Encoding UTF8 # dot prefix, no ext

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 2; 'md' = 1 } -Label 'T1 known/unknown/no-ext basic counting'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 2: prune-at-boundary — case-insensitive skip-set match on
#         dir leaf names. Files inside skipped dirs MUST NOT be tallied.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'node_modules\react') -Force | Out-Null
    'real x' | Set-Content (Join-Path $root 'node_modules\react\index.md') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'NODE_MODULES\angular') -Force | Out-Null
    'should be skipped (case)' | Set-Content (Join-Path $root 'NODE_MODULES\angular\poison.md') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'target\release') -Force | Out-Null
    'rust build artifact' | Set-Content (Join-Path $root 'target\release\poison.rs') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'kept') -Force | Out-Null
    'kept' | Set-Content (Join-Path $root 'kept\real.rs') -Encoding UTF8

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T2 prune skipped dirs (case-insensitive)'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 3: hidden files MUST be counted (matches old -Force).
# ---------------------------------------------------------------
$root = New-TempDir
try {
    'visible' | Set-Content (Join-Path $root 'visible.md') -Encoding UTF8
    $hiddenFile = Join-Path $root 'hidden.md'
    'hidden' | Set-Content $hiddenFile -Encoding UTF8
    [IO.File]::SetAttributes($hiddenFile, [IO.FileAttributes]::Hidden)

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'md' = 2 } -Label 'T3 hidden files are counted'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 4: reparse-point (junction) MUST NOT be followed.
#         Windows-only — Linux/macOS skip (test-suite still passes).
# ---------------------------------------------------------------
if ($PSVersionTable.Platform -eq 'Win32NT' -or $env:OS -eq 'Windows_NT') {
    $root = New-TempDir
    $poisonRoot = New-TempDir
    try {
        'real' | Set-Content (Join-Path $root 'real.rs') -Encoding UTF8
        'poison' | Set-Content (Join-Path $poisonRoot 'poison.rs') -Encoding UTF8
        'poison2' | Set-Content (Join-Path $poisonRoot 'poison.md') -Encoding UTF8

        # Create a junction (NTFS, no admin needed) from $root\linked → $poisonRoot.
        # Use cmd's mklink /J because PowerShell New-Item -ItemType Junction
        # is not available on Windows PowerShell 5.1 in all configurations.
        $junction = Join-Path $root 'linked'
        & cmd.exe /c mklink /J "$junction" "$poisonRoot" 2>&1 | Out-Null
        if (-not (Test-Path $junction)) {
            Write-Host "SKIP  T4 reparse-point skip (could not create junction; need NTFS)" -ForegroundColor DarkYellow
        }
        else {
            $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
            Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T4 reparse-point (junction) not followed'
        }
    }
    finally {
        Remove-TempDir $root
        Remove-TempDir $poisonRoot
    }
}
else {
    Write-Host "SKIP  T4 reparse-point skip (non-Windows; test would need symlink permissions)" -ForegroundColor DarkYellow
}

# ---------------------------------------------------------------
# Test 5: deeply nested layout (sanity that DFS handles depth).
# ---------------------------------------------------------------
$root = New-TempDir
try {
    $deep = $root
    for ($i = 0; $i -lt 8; $i++) {
        $deep = Join-Path $deep ('lvl' + $i)
        New-Item -ItemType Directory -Path $deep -Force | Out-Null
        "fn x() {}" | Set-Content (Join-Path $deep ("file$i.rs")) -Encoding UTF8
    }
    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 8 } -Label 'T5 deeply nested DFS counts every level'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 6: empty repo.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    if ($r.Count -eq 0) {
        Write-Host "PASS  T6 empty repo returns empty hashtable" -ForegroundColor Green
        $Global:_passes++
    }
    else {
        Write-Host "FAIL  T6 empty repo returned $($r.Count) entries" -ForegroundColor Red
        $Global:_failures++
    }
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 7: dir whose name STARTS with a skip-dir name (e.g. 'target-old')
#         must NOT be skipped — the skip-set is leaf-equality, not prefix.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'target-old') -Force | Out-Null
    'kept' | Set-Content (Join-Path $root 'target-old\real.rs') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'target') -Force | Out-Null
    'pruned' | Set-Content (Join-Path $root 'target\poison.rs') -Encoding UTF8

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T7 prefix-of-skip-name not falsely skipped'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Cyan
Write-Host "Passed: $Global:_passes" -ForegroundColor Green
if ($Global:_failures -gt 0) {
    Write-Host "Failed: $Global:_failures" -ForegroundColor Red
    exit 1
}
Write-Host "Failed: 0" -ForegroundColor Green
exit 0
