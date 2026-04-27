#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Downloads the latest xray release and configures MCP for a target repository.

.DESCRIPTION
    1. Downloads xray.exe from the latest GitHub release (if not already installed)
    2. Detects file extensions in the target repo heuristically
    3. Creates .vscode/mcp.json (VS Code Copilot) and .roo/mcp.json (Roo)
    All xray tools are enabled by default EXCEPT xray_edit.

.PARAMETER RepoPath
    Path to the target repository. If not specified, will prompt interactively.

.PARAMETER InstallDir
    Where to install xray.exe. Defaults to ~/.cargo/bin/

.PARAMETER SkipDownload
    Skip downloading xray.exe (use existing installation).

.PARAMETER EnableRoo
    Also create .roo/mcp.json for Roo Code (without this, Roo is prompted interactively).

.PARAMETER Force
    Overwrite existing MCP configs without asking.

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject
    .\setup-xray.ps1   # interactive mode
#>
param(
    [string]$RepoPath,
    [string]$InstallDir = "$env:LOCALAPPDATA\xray",
    [string]$GithubRepo = "pustynsky/xray",
    [switch]$SkipDownload,
    [switch]$EnableRoo,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# ── Allowed tools (all except xray_edit) ──────────────────────────────
$AllowedTools = @(
    "xray_fast",
    "xray_git_authors",
    "xray_git_activity",
    "xray_git_history",
    "xray_git_diff",
    "xray_git_blame",
    "xray_info",
    "xray_reindex",
    "xray_reindex_definitions",
    "xray_help",
    "xray_grep",
    "xray_branch_status",
    "xray_definitions",
    "xray_callers"
)

# ── Known text/code extensions grouped by ecosystem ───────────────────
$KnownCodeExtensions = @{
    # .NET / C#
    'cs' = 'C#'; 'csx' = 'C# Script'; 'csproj' = 'C# Project';
    'vb' = 'VB.NET'; 'vbproj' = 'VB Project';
    'sln' = 'Solution'; 'props' = 'MSBuild'; 'targets' = 'MSBuild';
    'xaml' = 'XAML'; 'razor' = 'Razor'; 'cshtml' = 'Razor View';

    # Web / TypeScript / JavaScript
    'ts' = 'TypeScript'; 'tsx' = 'TSX'; 'js' = 'JavaScript'; 'jsx' = 'JSX';
    'mjs' = 'ES Module'; 'cjs' = 'CommonJS'; 'vue' = 'Vue'; 'svelte' = 'Svelte';
    'html' = 'HTML'; 'htm' = 'HTML'; 'css' = 'CSS'; 'scss' = 'SCSS';
    'less' = 'Less'; 'sass' = 'Sass';

    # Rust
    'rs' = 'Rust'; 'toml' = 'TOML';

    # Python
    'py' = 'Python'; 'pyi' = 'Python Stub'; 'pyx' = 'Cython';

    # Go
    'go' = 'Go'; 'mod' = 'Go Module';

    # Java / Kotlin
    'java' = 'Java'; 'kt' = 'Kotlin'; 'kts' = 'Kotlin Script';
    'gradle' = 'Gradle';

    # C / C++
    'c' = 'C'; 'cpp' = 'C++'; 'cc' = 'C++'; 'cxx' = 'C++';
    'h' = 'C Header'; 'hpp' = 'C++ Header'; 'hxx' = 'C++ Header';

    # Ruby / PHP / Perl
    'rb' = 'Ruby'; 'php' = 'PHP'; 'pl' = 'Perl'; 'pm' = 'Perl Module';

    # Swift / Objective-C
    'swift' = 'Swift'; 'm' = 'Objective-C'; 'mm' = 'Objective-C++';

    # Shell / Scripts
    'ps1' = 'PowerShell'; 'psm1' = 'PS Module'; 'psd1' = 'PS Data';
    'sh' = 'Shell'; 'bash' = 'Bash'; 'zsh' = 'Zsh';

    # Config / Data
    'xml' = 'XML'; 'json' = 'JSON'; 'jsonc' = 'JSONC';
    'yaml' = 'YAML'; 'yml' = 'YAML';
    'config' = 'Config'; 'ini' = 'INI'; 'env' = 'Env';
    'manifestxml' = 'Manifest XML';

    # Documentation
    'md' = 'Markdown'; 'txt' = 'Text'; 'rst' = 'reStructuredText';

    # SQL
    'sql' = 'SQL';

    # Scala / Clojure / Elixir / Erlang
    'scala' = 'Scala'; 'clj' = 'Clojure'; 'ex' = 'Elixir'; 'erl' = 'Erlang';

    # Dart / Flutter
    'dart' = 'Dart';

    # Lua
    'lua' = 'Lua';

    # R
    'r' = 'R'; 'rmd' = 'R Markdown';

    # Terraform / HCL
    'tf' = 'Terraform'; 'hcl' = 'HCL';
}

# Directories to skip during extension detection
$SkipDirs = @('.git', 'node_modules', 'bin', 'obj', 'target', 'dist',
              'build', '.vs', '.vscode', '.roo', '.idea', '__pycache__',
              'packages', '.nuget', 'vendor', '.next', '.output', 'coverage',
              'Debug', 'Release', 'x64', 'x86', '.playwright-mcp')

# ── Step 1: Ask for repo path if not provided ────────────────────────
if (-not $RepoPath) {
    $RepoPath = Read-Host "Enter the path to the target repository"
}
$RepoPath = (Resolve-Path $RepoPath).Path

if (-not (Test-Path (Join-Path $RepoPath ".git"))) {
    Write-Warning "$RepoPath does not appear to be a git repository (.git not found)"
    $continue = Read-Host "Continue anyway? (y/N)"
    if ($continue -ne 'y') { exit 0 }
}

Write-Host "`n=== Setting up xray MCP for: $RepoPath ===" -ForegroundColor Cyan

# ── Step 2: Download xray.exe if needed ──────────────────────────────
$xrayPath = Join-Path $InstallDir "xray.exe"

if (-not $SkipDownload) {
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    # Check if gh CLI is available
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Write-Error "GitHub CLI (gh) is required. Install from https://cli.github.com/"
        exit 1
    }

    $tag = gh release list --repo $GithubRepo --limit 1 --json tagName --jq '.[0].tagName'
    if (-not $tag) {
        Write-Error "No releases found in $GithubRepo"
        exit 1
    }

    $needsDownload = $true
    if (Test-Path $xrayPath) {
        Write-Host "xray.exe already exists at $xrayPath" -ForegroundColor Yellow
        $dl = Read-Host "Download latest ($tag) and overwrite? (y/N)"
        if ($dl -ne 'y') { $needsDownload = $false }
    }

    if ($needsDownload) {
        Write-Host "Downloading xray $tag..." -ForegroundColor Cyan
        $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) "xray-download"
        if (Test-Path $tempDir) { Remove-Item $tempDir -Recurse -Force }
        New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

        gh release download $tag --repo $GithubRepo --pattern "xray.exe" --dir $tempDir

        $downloaded = Join-Path $tempDir "xray.exe"
        if (-not (Test-Path $downloaded)) {
            Write-Error "Download failed - xray.exe not found in release assets"
            Remove-Item $tempDir -Recurse -Force
            exit 1
        }
        Move-Item $downloaded $xrayPath -Force
        Remove-Item $tempDir -Recurse -Force
        Write-Host "Installed xray $tag to $xrayPath" -ForegroundColor Green
    }
} else {
    if (-not (Test-Path $xrayPath)) {
        Write-Error "xray.exe not found at $xrayPath. Run without -SkipDownload."
        exit 1
    }
    Write-Host "Using existing xray at $xrayPath" -ForegroundColor Yellow
}

# ── Step 3: Detect file extensions heuristically ─────────────────────
Write-Host "`nScanning repository for file extensions..." -ForegroundColor Cyan

$extCounts = @{}
$skipSet = [System.Collections.Generic.HashSet[string]]::new(
    [StringComparer]::OrdinalIgnoreCase)
$SkipDirs | ForEach-Object { $skipSet.Add($_) | Out-Null }

Get-ChildItem -Path $RepoPath -Recurse -File -Force -ErrorAction SilentlyContinue |
    Where-Object {
        $dominated = $false
        $rel = $_.FullName.Substring($RepoPath.Length + 1)
        foreach ($seg in $rel.Split([IO.Path]::DirectorySeparatorChar)) {
            if ($skipSet.Contains($seg)) { $dominated = $true; break }
        }
        -not $dominated
    } |
    ForEach-Object {
        $ext = $_.Extension.TrimStart('.').ToLower()
        if ($ext -and $KnownCodeExtensions.ContainsKey($ext)) {
            if (-not $extCounts.ContainsKey($ext)) { $extCounts[$ext] = 0 }
            $extCounts[$ext]++
        }
    }

if ($extCounts.Count -eq 0) {
    Write-Error "No recognized code/text files found in $RepoPath"
    exit 1
}

# Sort by count descending, take top extensions
$ranked = $extCounts.GetEnumerator() |
    Sort-Object Value -Descending |
    Select-Object -First 20

Write-Host "`nDetected extensions:" -ForegroundColor Cyan
$i = 1
foreach ($e in $ranked) {
    $lang = $KnownCodeExtensions[$e.Key]
    Write-Host ("  {0,2}. .{1,-12} {2,6} files  ({3})" -f $i, $e.Key, $e.Value, $lang)
    $i++
}

# Auto-select: take extensions with >= 0.5% of total files, minimum 5 files
$totalFiles = ($ranked | Measure-Object -Property Value -Sum).Sum
$threshold = [Math]::Max(5, $totalFiles * 0.005)

$autoSelected = ($ranked | Where-Object { $_.Value -ge $threshold }).Key

# Always include md if present
if ($extCounts.ContainsKey('md') -and 'md' -notin $autoSelected) {
    $autoSelected += 'md'
}

$suggested = ($autoSelected | Sort-Object) -join ','

Write-Host "`nSuggested extensions (>= $([int]$threshold) files): " -NoNewline
Write-Host $suggested -ForegroundColor Green

$userInput = Read-Host "`nAccept suggested extensions, or enter your own (comma-separated) [Enter = accept]"
if ($userInput.Trim()) {
    $selectedExts = $userInput.Trim()
} else {
    $selectedExts = $suggested
}

Write-Host "Using extensions: $selectedExts" -ForegroundColor Green

# ── Step 4: Build xray args ──────────────────────────────────────────

$xrayArgs = @(
    "serve",
    "--dir", $RepoPath,
    "--ext", $selectedExts,
    "--watch",
    "--definitions",
    "--metrics",
    "--debug-log"
)

# ── Step 5: Write .vscode/mcp.json (Copilot) ─────────────────────────
$vscodeMcpDir = Join-Path $RepoPath ".vscode"
$vscodeMcpPath = Join-Path $vscodeMcpDir "mcp.json"

$writeVscode = $true
if ((Test-Path $vscodeMcpPath) -and -not $Force) {
    Write-Warning "$vscodeMcpPath already exists"
    $ow = Read-Host "Overwrite? (y/N)"
    if ($ow -ne 'y') { $writeVscode = $false }
}

if ($writeVscode) {
    if (-not (Test-Path $vscodeMcpDir)) {
        New-Item -ItemType Directory -Path $vscodeMcpDir -Force | Out-Null
    }

    $vscodeConfig = [ordered]@{
        servers = [ordered]@{
            xray = [ordered]@{
                type = 'stdio'
                command = $xrayPath
                args = $xrayArgs
            }
        }
    }
    $vscodeConfig | ConvertTo-Json -Depth 5 | Set-Content -Path $vscodeMcpPath -Encoding UTF8
    Write-Host "Created $vscodeMcpPath" -ForegroundColor Green
}

# ── Step 6: Write .roo/mcp.json (Roo) ────────────────────────────────
$rooMcpDir = Join-Path $RepoPath ".roo"
$rooMcpPath = Join-Path $rooMcpDir "mcp.json"

$writeRoo = $false
if ($EnableRoo) {
    $writeRoo = $true
} elseif (-not $Force) {
    $installRoo = Read-Host "`nAlso configure for Roo Code? (y/N)"
    if ($installRoo -eq 'y') { $writeRoo = $true }
}

if ($writeRoo -and (Test-Path $rooMcpPath) -and -not $Force) {
    Write-Warning "$rooMcpPath already exists"
    $ow = Read-Host "Overwrite? (y/N)"
    if ($ow -ne 'y') { $writeRoo = $false }
}

if ($writeRoo) {
    if (-not (Test-Path $rooMcpDir)) {
        New-Item -ItemType Directory -Path $rooMcpDir -Force | Out-Null
    }

    $rooConfig = [ordered]@{
        mcpServers = [ordered]@{
            xray = [ordered]@{
                command = $xrayPath
                args = $xrayArgs
                alwaysAllow = $AllowedTools
                disabled = $false
                timeout = 300
            }
        }
    }
    $rooConfig | ConvertTo-Json -Depth 5 | Set-Content -Path $rooMcpPath -Encoding UTF8
    Write-Host "Created $rooMcpPath" -ForegroundColor Green
}

# ── Step 7: Protect MCP configs from accidental git push ─────────────
#   Tracked files   → --skip-worktree (local changes invisible to git)
#   Untracked files → .git/info/exclude (local gitignore, not committed)
$isGitRepo = Test-Path (Join-Path $RepoPath ".git")
if ($isGitRepo) {
    Push-Location $RepoPath
    try {
        $skipWorktreeFiles = @()
        $excludedFiles = @()

        $excludePath = (git rev-parse --git-path info/exclude 2>$null)
        if (-not $excludePath -or -not (Test-Path (Split-Path $excludePath))) {
            $excludePath = $null
        }

        # Read existing exclude entries to avoid duplicates
        $existingExcludes = @()
        if (Test-Path $excludePath) {
            $existingExcludes = Get-Content $excludePath -ErrorAction SilentlyContinue
        }

        foreach ($mcpFile in @(
            @{ Path = ".vscode/mcp.json"; Protect = (Test-Path (Join-Path $RepoPath ".vscode/mcp.json")) },
            @{ Path = ".roo/mcp.json";    Protect = ($writeRoo -and (Test-Path (Join-Path $RepoPath ".roo/mcp.json"))) }
        )) {
            if (-not $mcpFile.Protect) { continue }
            $rel = $mcpFile.Path

            $tracked = git ls-files $rel 2>$null
            if ($tracked) {
                # Tracked → skip-worktree
                git update-index --skip-worktree $rel 2>$null
                $skipWorktreeFiles += $rel
            } else {
                # Untracked → .git/info/exclude
                if ($excludePath -and $existingExcludes -notcontains $rel) {
                    Add-Content -Path $excludePath -Value $rel -Encoding UTF8
                    $excludedFiles += $rel
                }
            }
        }

        if ($skipWorktreeFiles.Count -gt 0) {
            Write-Host "`nGit protection (--skip-worktree) applied to:" -ForegroundColor Cyan
            foreach ($f in $skipWorktreeFiles) {
                Write-Host "  $f (tracked, local edits hidden)" -ForegroundColor Green
            }
        }
        if ($excludedFiles.Count -gt 0) {
            Write-Host "`nGit protection (.git/info/exclude) applied to:" -ForegroundColor Cyan
            foreach ($f in $excludedFiles) {
                Write-Host "  $f (untracked, hidden from git status)" -ForegroundColor Green
            }
        }
        if (($skipWorktreeFiles + $excludedFiles).Count -gt 0) {
            Write-Host "  Files will not appear in git status/add/commit." -ForegroundColor DarkGray
        }
    } finally {
        Pop-Location
    }
}

# ── Done ──────────────────────────────────────────────────────────────
Write-Host "`n=== Setup complete ===" -ForegroundColor Cyan
Write-Host "xray binary:    $xrayPath"
Write-Host "Target repo:    $RepoPath"
Write-Host "Extensions:     $selectedExts"
Write-Host "Copilot config: $vscodeMcpPath"
if ($writeRoo) {
    Write-Host "Roo config:     $rooMcpPath"
}
Write-Host "`nReopen the repo in VS Code / Roo to activate xray MCP." -ForegroundColor Yellow
