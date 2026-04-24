#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Check documentation for product-specific names that should be neutralized.

.DESCRIPTION
    Scans git-tracked markdown files in docs/ and user-stories/ for
    PowerBI/Shared-specific identifiers that should not appear in
    public-facing documentation.

    Only scans files tracked by git (git ls-files).

    Excludes:
    - demo/ (test reports with real data)
    - tests/ (test case examples)
    - .roo/ (project config)
    - git-filter-by-author-and-message.md (use cases with real examples)
    - TODO-angular-html-template-parser.md (design doc with real Angular examples)

.EXAMPLE
    .\scripts\check-product-names.ps1
    .\scripts\check-product-names.ps1 -Fix  # show suggested replacements
#>

param(
    [switch]$Fix  # Show suggested replacements
)

$ErrorActionPreference = 'Stop'

# ── Product-specific patterns to detect ──
$patterns = @(
    @{ Pattern = 'BlockServiceProvider'; Replacement = 'ServiceProvider' }
    @{ Pattern = 'BlockServiceDependency'; Replacement = 'ServiceDependency' }
    @{ Pattern = 'MonitoredBlock'; Replacement = 'MonitoredService' }
    @{ Pattern = 'MonitoredScope'; Replacement = 'MonitoredTask' }
    @{ Pattern = 'IFlightResolver'; Replacement = 'IFeatureResolver' }
    @{ Pattern = 'FlightInput'; Replacement = 'FeatureInput' }
    @{ Pattern = 'CatalogQueryManager'; Replacement = 'StorageIndexManager' }
    @{ Pattern = 'TenantMapperCache'; Replacement = 'UserMapperCache' }
    @{ Pattern = 'FabricSearch'; Replacement = 'PlatformSearch' }
    @{ Pattern = 'GenericIndexer'; Replacement = 'SearchIndexer' }
    @{ Pattern = 'FabricResilientSearchClient'; Replacement = 'ResilientSearchClient' }
    @{ Pattern = 'AzureAISearchConnectionFactory'; Replacement = 'SearchConnectionFactory' }
    @{ Pattern = 'OnelakeCatalog'; Replacement = 'DataCatalog' }
    @{ Pattern = 'MetadataService'; Replacement = 'ApiService' }
    @{ Pattern = 'ShimClient'; Replacement = 'ProxyClient' }
    @{ Pattern = 'ShimIndexClient'; Replacement = 'ProxyIndexClient' }
    @{ Pattern = 'GenericIndexerSecurityAuditor'; Replacement = 'SecurityAuditor' }
    @{ Pattern = 'PowerBIExtended'; Replacement = 'ExtendedStore' }
    @{ Pattern = 'Sql/CloudBI'; Replacement = 'src/Services' }
    @{ Pattern = 'C:\\Repos\\Shared'; Replacement = 'C:\Projects\MainApp' }
    @{ Pattern = 'C:/Repos/Shared'; Replacement = 'C:/Projects/MainApp' }
    # Username leak (real internal corp username, NOT the public GitHub
    # owner — see note below). Hardcoded in private PS scripts the user
    # may copy from their workspace into the public repo by mistake.
    @{ Pattern = 'sepustyn'; Replacement = '<user>' }
    # NOTE: deliberately do NOT flag the public GitHub owner `pustynsky`.
    # README / install.md / clone URLs cite it as the canonical owner,
    # which is correct and not a leak. If a future repo fork makes the
    # owner sensitive, add it here.
    # Index-prefix example using a real private repo name ("Shared").
    @{ Pattern = 'repos_shared'; Replacement = 'repos_<repo>' }
    # Internal ADO orgs / telemetry stores referenced in docs.
    @{ Pattern = 'msasg'; Replacement = '<internal-ado-org>' }
    @{ Pattern = 'ado-mcp'; Replacement = '<ado-mcp-server>' }
)

# Path-shaped regex patterns (matched as regex, not literal substring).
# Catch hardcoded user-profile paths so a future PS-script copy/paste does
# not leak the local username into git-tracked output.
#
# The captured username segment is checked against a small whitelist of
# documentation placeholders (you, user, username, <user>, $env:USERNAME)
# so generic install docs do not produce false positives.
$pathPlaceholders = @('you', 'user', 'username', '<user>', '<username>', '$env:USERNAME')
$pathPatterns = @(
    @{ Pattern = 'C:\\Users\\([a-zA-Z0-9._-]+)\\AppData\\Local\\xray'; Replacement = '$env:LOCALAPPDATA\xray' }
    @{ Pattern = 'C:/Users/([a-zA-Z0-9._-]+)/AppData/Local/xray';        Replacement = '$env:LOCALAPPDATA/xray' }
)

# Broad word-boundary patterns (checked via regex \b...\b)
# These catch any usage of the word, not just exact compound identifiers
$broadPatterns = @(
    @{ Pattern = 'PowerBI'; Replacement = "(remove or use 'enterprise app')" }
    @{ Pattern = 'datahub'; Replacement = '(replace with neutral component name)' }
    @{ Pattern = 'pbi-'; Replacement = "(replace with neutral prefix, e.g. 'app-')" }
    @{ Pattern = 'Trident'; Replacement = 'Platform' }
    @{ Pattern = 'Microsoft'; Replacement = "(replace with 'Contoso' or remove)" }
    @{ Pattern = 'TenantMapping'; Replacement = 'UserMapping' }
    # Microsoft data-platform product names that leak as casual references.
    @{ Pattern = 'OneLake';  Replacement = '(replace with neutral storage name)' }
    @{ Pattern = 'Synapse';  Replacement = '(replace with neutral analytics store)' }
    @{ Pattern = 'Kusto';    Replacement = '(replace with neutral telemetry store)' }
    @{ Pattern = 'Headway';  Replacement = '(internal codename — remove or replace)' }
    # Corporate / hosted-identity leaks.
    @{ Pattern = '@microsoft\.com'; Replacement = "(replace with neutral example, e.g. '@example.com')" }
    @{ Pattern = 'dev\.azure\.com/[a-zA-Z0-9._-]+'; Replacement = '(strip ADO org slug from URLs in docs)' }
    @{ Pattern = '[a-zA-Z0-9._-]+\.visualstudio\.com'; Replacement = '(strip legacy ADO tenant from URLs in docs)' }
)

# ── Get git-tracked files (docs + scripts + sources) ──
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

$gitFiles = git -C $repoRoot ls-files 'docs/*.md' 'user-stories/*.md' 'README.md' 'CHANGELOG.md' 'src/*.rs' 'src/**/*.rs' 'scripts/*.ps1' 'scripts/**/*.ps1' 'prompts/*.md' 2>$null
if (-not $gitFiles) {
    Write-Host 'No git-tracked files found. Is this a git repository?' -ForegroundColor Red
    exit 1
}

# Files to exclude (contain real product data by design or are the
# pattern catalog itself — self-match would be infinite recursion).
$excludePatterns = @(
    'git-filter-by-author-and-message.md'
    'TODO-angular-html-template-parser.md'
    'demo/'
    'tests/'
    'check-product-names.ps1'
)

# ── Build pre-compiled combined regexes ──
$combinedExact = [regex]::new(
    '(' + (($patterns | ForEach-Object { [regex]::Escape($_.Pattern) }) -join '|') + ')',
    'Compiled'
)
$combinedPath = [regex]::new(
    '(' + (($pathPatterns | ForEach-Object { $_.Pattern }) -join '|') + ')',
    [System.Text.RegularExpressions.RegexOptions]('Compiled, IgnoreCase')
)
$combinedBroad = [regex]::new(
    '(' + (($broadPatterns | ForEach-Object { $_.Pattern }) -join '|') + ')',
    [System.Text.RegularExpressions.RegexOptions]('Compiled, IgnoreCase')
)
$urlRegex = [regex]::new('https?://', 'Compiled')

$totalFindings = 0
$fileFindings = @{}

foreach ($gitFile in $gitFiles) {
    # Skip excluded files/dirs
    $skip = $false
    foreach ($ep in $excludePatterns) {
        if ($gitFile -like "*$ep*") { $skip = $true; break }
    }
    if ($skip) { continue }

    $fullPath = Join-Path $repoRoot $gitFile
    if (-not (Test-Path $fullPath)) { continue }

    # Fast file read (avoids Get-Content pipeline overhead)
    $content = [System.IO.File]::ReadAllText($fullPath)
    $relativePath = $gitFile

    # Quick-check: skip file entirely if no pattern matches the whole content
    $hasExact = $combinedExact.IsMatch($content)
    $hasPath  = $combinedPath.IsMatch($content)
    $hasBroad = $combinedBroad.IsMatch($content)
    if (-not $hasExact -and -not $hasPath -and -not $hasBroad) { continue }

    # Only split into lines for files with matches (rare path)
    $lines = $content.Split("`n")
    $lineNum = 0
    foreach ($line in $lines) {
        $lineNum++

        if ($hasExact) {
            $m = $combinedExact.Matches($line)
            foreach ($hit in $m) {
                $matched = $hit.Value
                foreach ($p in $patterns) {
                    if ($matched -eq $p.Pattern -or $matched.Contains($p.Pattern)) {
                        $totalFindings++
                        if (-not $fileFindings.ContainsKey($relativePath)) {
                            $fileFindings[$relativePath] = @()
                        }
                        $finding = "  Line ${lineNum}: $($p.Pattern)"
                        if ($Fix) {
                            $finding += " -> $($p.Replacement)"
                        }
                        $fileFindings[$relativePath] += $finding
                        break
                    }
                }
            }
        }

        if ($hasPath) {
            $mp = $combinedPath.Matches($line)
            foreach ($hit in $mp) {
                $matched = $hit.Value
                foreach ($pp in $pathPatterns) {
                    if ($matched -match $pp.Pattern) {
                        # Username segment captured as group 1; whitelist of
                        # documentation placeholders avoids flagging install
                        # docs that use "you"/"user"/etc. as a stand-in.
                        $captured = if ($Matches.Count -gt 1) { $Matches[1] } else { '' }
                        if ($pathPlaceholders -contains $captured) { continue }
                        $totalFindings++
                        if (-not $fileFindings.ContainsKey($relativePath)) {
                            $fileFindings[$relativePath] = @()
                        }
                        $finding = "  Line ${lineNum}: $matched"
                        if ($Fix) {
                            $finding += " -> $($pp.Replacement)"
                        }
                        $fileFindings[$relativePath] += $finding
                        break
                    }
                }
            }
        }

        if ($hasBroad -and -not $urlRegex.IsMatch($line)) {
            $mb = $combinedBroad.Matches($line)
            foreach ($hit in $mb) {
                $matched = $hit.Value
                foreach ($bp in $broadPatterns) {
                    if ($matched -match $bp.Pattern) {
                        $totalFindings++
                        if (-not $fileFindings.ContainsKey($relativePath)) {
                            $fileFindings[$relativePath] = @()
                        }
                        $finding = "  Line ${lineNum}: $($bp.Pattern)"
                        if ($Fix) {
                            $finding += " -> $($bp.Replacement)"
                        }
                        $fileFindings[$relativePath] += $finding
                        break
                    }
                }
            }
        }
    }
}
# ── Output ──
if ($totalFindings -eq 0) {
    Write-Host 'No product-specific names found in git-tracked documentation.' -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host "Found $totalFindings product-specific name(s) in documentation:" -ForegroundColor Yellow
Write-Host ''

foreach ($file in ($fileFindings.Keys | Sort-Object)) {
    Write-Host "  $file" -ForegroundColor Cyan
    foreach ($finding in $fileFindings[$file]) {
        Write-Host $finding -ForegroundColor White
    }
    Write-Host ''
}

if (-not $Fix) {
    Write-Host 'Run with -Fix to see suggested replacements.' -ForegroundColor DarkGray
}

Write-Host "Total: $totalFindings finding(s) in $($fileFindings.Count) file(s)" -ForegroundColor Yellow
exit 1