#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Verifies that the embedded MCP filter scripts in setup-xray.ps1 are
    byte-identical to the canonical scripts/mcp-filter/{smudge,clean}.sh
    sources.

.DESCRIPTION
    setup-xray.ps1 ships two embedded constants ($Script:EmbeddedSmudgeSh
    and $Script:EmbeddedCleanSh) so that the bootstrap one-liner
    (`iex (irm .../setup-xray.ps1)`) and Option A2 (download then run)
    install paths can wire git smudge/clean filters without needing the
    `scripts/mcp-filter/` directory next to the script on disk.

    Install-McpFilter prefers the on-disk source when present (clone
    case), so local edits to scripts/mcp-filter/*.sh take effect without
    re-embedding. This test ensures the embedded copies do NOT drift
    from the canonical sources — a drift means clone-mode users and
    one-liner users would silently get DIFFERENT filter behavior.

    Comparison is byte-equal after LF normalization (the embedded
    constants live inside a .ps1 file that may be saved with CRLF on
    Windows; both are normalized to LF before comparison, matching the
    LF-normalize step that Install-McpFilter applies on the install
    path).

    Exits 0 on byte-equal, 1 on any drift.

.NOTES
    Run after editing scripts/mcp-filter/{smudge,clean}.sh — copy the
    updated body verbatim into the corresponding here-string in
    setup-xray.ps1.
#>

$ErrorActionPreference = 'Stop'
$failures = 0
$passes = 0

function Get-EmbeddedConstantBody {
    <#
        Walk the AST to find a single-quoted here-string assigned to
        $ConstantName, then return its evaluated value (using the AST's
        own .Value property — equivalent to dot-sourcing the assignment
        without executing the rest of the script). Normalizes line
        endings to LF and ensures a trailing LF, matching the
        normalization that Install-McpFilter applies on the install
        path.

        Returns $null if the constant is not found or is not a
        single-quoted here-string.
    #>
    param(
        [Parameter(Mandatory)] $ScriptAst,
        [Parameter(Mandatory)] [string]$ConstantName
    )

    $assigns = $ScriptAst.FindAll({
            param($n)
            $n -is [Management.Automation.Language.AssignmentStatementAst] -and
            $n.Left.Extent.Text -eq $ConstantName
        }, $true)
    if (-not $assigns -or $assigns.Count -eq 0) { return $null }

    # The right-hand side of the assignment may be wrapped in a
    # CommandExpressionAst / PipelineAst — drill down to the
    # StringConstantExpressionAst.
    $strExpr = $assigns[0].Right.Find({
            param($n) $n -is [Management.Automation.Language.StringConstantExpressionAst]
        }, $true)
    if (-not $strExpr) { return $null }
    if ($strExpr.StringConstantType -ne 'SingleQuotedHereString') {
        Write-Host "WARN  $ConstantName is not a single-quoted here-string (found $($strExpr.StringConstantType))" -ForegroundColor Yellow
        return $null
    }

    $body = $strExpr.Value -replace "`r`n", "`n"
    if (-not $body.EndsWith("`n")) { $body += "`n" }
    return $body
}

# Locate setup-xray.ps1. This test lives at scripts/mcp-filter/test-embedded-sync.ps1,
# so the repo root is two directories up.
$repoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
$scriptPath = Join-Path $repoRoot 'scripts\setup-xray.ps1'
if (-not (Test-Path $scriptPath)) {
    Write-Error "setup-xray.ps1 not found: $scriptPath"
    exit 1
}

# Parse setup-xray.ps1 once.
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors -and $parseErrors.Count -gt 0) {
    Write-Error "setup-xray.ps1 has parse errors: $($parseErrors -join '; ')"
    exit 1
}

$pairs = @(
    @{ Constant = '$Script:EmbeddedSmudgeSh'; Disk = 'scripts\mcp-filter\smudge.sh' }
    @{ Constant = '$Script:EmbeddedCleanSh';  Disk = 'scripts\mcp-filter\clean.sh' }
)

foreach ($p in $pairs) {
    $constantName = $p.Constant
    $diskPath = Join-Path $repoRoot $p.Disk

    if (-not (Test-Path $diskPath)) {
        Write-Host "FAIL  $constantName : on-disk source not found at $diskPath" -ForegroundColor Red
        $failures++
        continue
    }

    $embedded = Get-EmbeddedConstantBody -ScriptAst $ast -ConstantName $constantName
    if ($null -eq $embedded) {
        Write-Host "FAIL  $constantName : not found in setup-xray.ps1 OR not a single-quoted here-string" -ForegroundColor Red
        $failures++
        continue
    }

    $disk = ([IO.File]::ReadAllText($diskPath)) -replace "`r`n", "`n"
    if (-not $disk.EndsWith("`n")) { $disk += "`n" }

    if ($embedded -eq $disk) {
        Write-Host "PASS  $constantName <-> $($p.Disk) byte-equal ($([Text.Encoding]::UTF8.GetByteCount($disk)) bytes)" -ForegroundColor Green
        $passes++
    }
    else {
        Write-Host "FAIL  $constantName <-> $($p.Disk) DRIFT" -ForegroundColor Red
        Write-Host ("    embedded length={0}  disk length={1}" -f $embedded.Length, $disk.Length) -ForegroundColor Red
        $minLen = [Math]::Min($embedded.Length, $disk.Length)
        for ($i = 0; $i -lt $minLen; $i++) {
            if ($embedded[$i] -ne $disk[$i]) {
                # Find line/column of the first mismatch in disk content.
                $upTo = $disk.Substring(0, $i)
                $lineNo = ($upTo -split "`n").Count
                $colNo = $i - ($upTo.LastIndexOf("`n"))
                Write-Host ("    first diff @ line {0} col {1}: embedded=0x{2:X2} disk=0x{3:X2}" -f $lineNo, $colNo, [int][char]$embedded[$i], [int][char]$disk[$i]) -ForegroundColor Red
                # Show the offending line from disk.
                $lines = $disk -split "`n"
                if ($lineNo -le $lines.Count) {
                    Write-Host ("    disk line {0}: {1}" -f $lineNo, $lines[$lineNo - 1]) -ForegroundColor Red
                }
                break
            }
        }
        if ($embedded.Length -ne $disk.Length) {
            Write-Host "    (length-only diff at end-of-file)" -ForegroundColor Red
        }
        Write-Host "    Fix: copy the body of $($p.Disk) verbatim into the $constantName here-string in scripts\setup-xray.ps1." -ForegroundColor Yellow
        $failures++
    }
}

Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Cyan
Write-Host "Passed: $passes" -ForegroundColor Green
if ($failures -gt 0) {
    Write-Host "Failed: $failures" -ForegroundColor Red
    exit 1
}
Write-Host "Failed: 0" -ForegroundColor Green
exit 0
