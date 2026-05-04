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
    Where to install xray.exe. Defaults to %LOCALAPPDATA%\xray.

.PARAMETER GithubRepo
    GitHub repository that publishes xray.exe releases.

.PARAMETER Extensions
    Optional comma-separated list of extensions to index. When provided,
    skips repository scanning and interactive extension selection.

.PARAMETER SkipDownload
    Skip downloading xray.exe and use the existing installation.

.PARAMETER EnableRoo
    Also create .roo/mcp.json for Roo Code. Without this, Roo is prompted interactively
    unless -Force is used, in which case Roo setup is skipped.

.PARAMETER Force
    Run non-interactively where possible: overwrite existing xray entry, overwrite
    existing xray.exe, accept suggested extensions, and skip Roo setup unless -EnableRoo is passed.

.PARAMETER Restore
    Restore .vscode/mcp.json (and .roo/mcp.json if present) from the .bak files
    created on the previous setup run, then exit. Skips download and extension
    detection. Use this to undo a setup-xray run.

    Backup behavior: every regular setup run (without -Restore) copies the
    existing .vscode/mcp.json to .vscode/mcp.json.bak (and the same for
    .roo/mcp.json) before overwriting. The .bak file is replaced on each run,
    so it always reflects the file state immediately before the most recent
    setup. If no .bak exists when -Restore is invoked, the script exits with
    an error.

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Extensions cs,sql,md -Force

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Restore
#>
param(
    [string]$RepoPath,
    [string]$InstallDir = "$env:LOCALAPPDATA\xray",
    [string]$GithubRepo = 'pustynsky/xray',
    [string]$Extensions,
    [switch]$SkipDownload,
    [switch]$EnableRoo,
    [switch]$Force,
    [switch]$Restore
)

$ErrorActionPreference = 'Stop'

$AllowedTools = @(
    'xray_fast',
    'xray_git_authors',
    'xray_git_activity',
    'xray_git_history',
    'xray_git_diff',
    'xray_git_blame',
    'xray_info',
    'xray_reindex',
    'xray_reindex_definitions',
    'xray_help',
    'xray_grep',
    'xray_branch_status',
    'xray_definitions',
    'xray_callers'
)

$KnownCodeExtensions = @{
    'cs' = 'C#'; 'csx' = 'C# Script'; 'csproj' = 'C# Project';
    'vb' = 'VB.NET'; 'vbproj' = 'VB Project';
    'sln' = 'Solution'; 'props' = 'MSBuild'; 'targets' = 'MSBuild';
    'xaml' = 'XAML'; 'razor' = 'Razor'; 'cshtml' = 'Razor View';
    'ts' = 'TypeScript'; 'tsx' = 'TSX'; 'js' = 'JavaScript'; 'jsx' = 'JSX';
    'mjs' = 'ES Module'; 'cjs' = 'CommonJS'; 'vue' = 'Vue'; 'svelte' = 'Svelte';
    'html' = 'HTML'; 'htm' = 'HTML'; 'css' = 'CSS'; 'scss' = 'SCSS';
    'less' = 'Less'; 'sass' = 'Sass';
    'rs' = 'Rust'; 'toml' = 'TOML';
    'py' = 'Python'; 'pyi' = 'Python Stub'; 'pyx' = 'Cython';
    'go' = 'Go'; 'mod' = 'Go Module';
    'java' = 'Java'; 'kt' = 'Kotlin'; 'kts' = 'Kotlin Script';
    'gradle' = 'Gradle';
    'c' = 'C'; 'cpp' = 'C++'; 'cc' = 'C++'; 'cxx' = 'C++';
    'h' = 'C Header'; 'hpp' = 'C++ Header'; 'hxx' = 'C++ Header';
    'rb' = 'Ruby'; 'php' = 'PHP'; 'pl' = 'Perl'; 'pm' = 'Perl Module';
    'swift' = 'Swift'; 'm' = 'Objective-C'; 'mm' = 'Objective-C++';
    'ps1' = 'PowerShell'; 'psm1' = 'PS Module'; 'psd1' = 'PS Data';
    'sh' = 'Shell'; 'bash' = 'Bash'; 'zsh' = 'Zsh';
    'xml' = 'XML'; 'json' = 'JSON'; 'jsonc' = 'JSONC';
    'yaml' = 'YAML'; 'yml' = 'YAML';
    'config' = 'Config'; 'ini' = 'INI'; 'env' = 'Env';
    'manifestxml' = 'Manifest XML';
    'md' = 'Markdown'; 'txt' = 'Text'; 'rst' = 'reStructuredText';
    'sql' = 'SQL';
    'scala' = 'Scala'; 'clj' = 'Clojure'; 'ex' = 'Elixir'; 'erl' = 'Erlang';
    'dart' = 'Dart';
    'lua' = 'Lua';
    'r' = 'R'; 'rmd' = 'R Markdown';
    'tf' = 'Terraform'; 'hcl' = 'HCL'
}

$SkipDirs = @(
    '.git', 'node_modules', 'bin', 'obj', 'target', 'dist',
    'build', '.vs', '.vscode', '.roo', '.idea', '__pycache__',
    'packages', '.nuget', 'vendor', '.next', '.output', 'coverage',
    'Debug', 'Release', 'x64', 'x86', '.playwright-mcp',
    'TestResults', 'testbin', 'shared.obj', 'shared.obj.x64Debug', 'shared.obj.x86Debug'
)

function Normalize-ExtensionList {
    param([string]$Value)

    return (($Value -split ',') |
            ForEach-Object { $_.Trim().TrimStart('.').ToLowerInvariant() } |
            Where-Object { $_ } |
            Sort-Object -Unique) -join ','
}

function Read-YesNo {
    param(
        [string]$Prompt,
        [bool]$Default = $false,
        [switch]$ForceYes,
        [switch]$ForceNo
    )

    if ($ForceYes) { return $true }
    if ($ForceNo) { return $false }

    $suffix = if ($Default) { ' (Y/n)' } else { ' (y/N)' }
    $answer = Read-Host ($Prompt + $suffix)
    if ([string]::IsNullOrWhiteSpace($answer)) {
        return $Default
    }

    return $answer.Trim().ToLowerInvariant() -eq 'y'
}

function Backup-McpJson {
    param(
        [string]$Path
    )

    if (-not (Test-Path $Path)) {
        return
    }

    $bakPath = "$Path.bak"
    Copy-Item -Path $Path -Destination $bakPath -Force
    Write-Host "Backed up existing config to $bakPath" -ForegroundColor DarkGray
}

function Restore-McpJson {
    param(
        [string]$Path
    )

    $bakPath = "$Path.bak"
    if (-not (Test-Path $bakPath)) {
        return $false
    }

    Copy-Item -Path $bakPath -Destination $Path -Force
    Remove-Item -Path $bakPath -Force
    Write-Host "Restored $Path from backup" -ForegroundColor Green
    return $true
}

function Get-DetectedExtensions {
    param(
        [string]$RootPath,
        [hashtable]$KnownExtensions,
        [string[]]$SkipDirectoryNames
    )

    $extCounts = @{}
    $skipSet = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $SkipDirectoryNames | ForEach-Object { $null = $skipSet.Add($_) }

    Get-ChildItem -Path $RootPath -Recurse -File -Force -ErrorAction SilentlyContinue |
        Where-Object {
            $relativePath = $_.FullName.Substring($RootPath.Length + 1)
            foreach ($segment in $relativePath.Split([IO.Path]::DirectorySeparatorChar)) {
                if ($skipSet.Contains($segment)) {
                    return $false
                }
            }
            return $true
        } |
        ForEach-Object {
            $ext = $_.Extension.TrimStart('.').ToLowerInvariant()
            if ($ext -and $KnownExtensions.ContainsKey($ext)) {
                if (-not $extCounts.ContainsKey($ext)) {
                    $extCounts[$ext] = 0
                }
                $extCounts[$ext]++
            }
        }

    return $extCounts
}

if (-not $RepoPath) {
    $RepoPath = Read-Host 'Enter the path to the target repository'
}

try {
    $RepoPath = (Resolve-Path $RepoPath).Path
}
catch {
    Write-Error "Repository path not found: $RepoPath"
    exit 1
}

if ($Restore) {
    Write-Host "`n=== Restoring MCP configs in: $RepoPath ===" -ForegroundColor Cyan

    $vscodeMcpPath = Join-Path $RepoPath '.vscode\mcp.json'
    $rooMcpPath = Join-Path $RepoPath '.roo\mcp.json'

    $restoredAny = $false
    if (Restore-McpJson -Path $vscodeMcpPath) { $restoredAny = $true }
    if (Restore-McpJson -Path $rooMcpPath) { $restoredAny = $true }

    if (-not $restoredAny) {
        Write-Error 'No .bak files found. Nothing to restore.'
        exit 1
    }

    Write-Host "`n=== Restore complete ===" -ForegroundColor Cyan
    exit 0
}

$isGitRepo = Test-Path (Join-Path $RepoPath '.git')
if (-not $isGitRepo) {
    Write-Warning "$RepoPath does not appear to be a git repository (.git not found)"
    $continue = Read-YesNo -Prompt 'Continue anyway?' -Default $false -ForceYes:$Force
    if (-not $continue) {
        exit 0
    }
}

Write-Host "`n=== Setting up xray MCP for: $RepoPath ===" -ForegroundColor Cyan

$xrayPath = Join-Path $InstallDir 'xray.exe'
if (-not $SkipDownload) {
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Write-Error 'GitHub CLI (gh) is required. Install from https://cli.github.com/'
        exit 1
    }

    $tag = (gh api "repos/$GithubRepo/releases/latest" --jq '.tag_name') 2>$null
    if (-not $tag) {
        Write-Error "No releases found in $GithubRepo (check gh auth status and repo name)"
        exit 1
    }

    $needsDownload = $true
    if (Test-Path $xrayPath) {
        Write-Host "xray.exe already exists at $xrayPath" -ForegroundColor Yellow
        $needsDownload = Read-YesNo -Prompt "Download latest ($tag) and overwrite?" -Default $false -ForceYes:$Force
        if ($needsDownload -and $Force) {
            Write-Host 'Force enabled; overwriting existing xray.exe.' -ForegroundColor Yellow
        }
    }

    if ($needsDownload) {
        Write-Host "Downloading xray $tag..." -ForegroundColor Cyan
        $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) 'xray-download'
        if (Test-Path $tempDir) {
            Remove-Item $tempDir -Recurse -Force
        }
        New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

        gh release download $tag --repo $GithubRepo --pattern 'xray.exe' --dir $tempDir

        $downloaded = Join-Path $tempDir 'xray.exe'
        if (-not (Test-Path $downloaded)) {
            Write-Error 'Download failed - xray.exe not found in release assets'
            Remove-Item $tempDir -Recurse -Force
            exit 1
        }

        Move-Item $downloaded $xrayPath -Force
        Remove-Item $tempDir -Recurse -Force
        Write-Host "Installed xray $tag to $xrayPath" -ForegroundColor Green
    }
}
else {
    if (-not (Test-Path $xrayPath)) {
        Write-Error "xray.exe not found at $xrayPath. Run without -SkipDownload."
        exit 1
    }
    Write-Host "Using existing xray at $xrayPath" -ForegroundColor Yellow
}

if ($Extensions) {
    $selectedExts = Normalize-ExtensionList -Value $Extensions
    if (-not $selectedExts) {
        Write-Error '-Extensions was provided but no valid extensions were found'
        exit 1
    }
    Write-Host "`nUsing extensions from -Extensions: $selectedExts" -ForegroundColor Green
}
else {
    Write-Host "`nScanning repository for file extensions..." -ForegroundColor Cyan
    $extCounts = Get-DetectedExtensions -RootPath $RepoPath -KnownExtensions $KnownCodeExtensions -SkipDirectoryNames $SkipDirs

    if ($extCounts.Count -eq 0) {
        Write-Error "No recognized code/text files found in $RepoPath"
        exit 1
    }

    $ranked = $extCounts.GetEnumerator() |
        Sort-Object Value -Descending |
        Select-Object -First 20

    Write-Host "`nDetected extensions:" -ForegroundColor Cyan
    $index = 1
    foreach ($item in $ranked) {
        $lang = $KnownCodeExtensions[$item.Key]
        Write-Host ('  {0,2}. .{1,-12} {2,6} files  ({3})' -f $index, $item.Key, $item.Value, $lang)
        $index++
    }

    $totalFiles = ($extCounts.Values | Measure-Object -Sum).Sum
    $threshold = [Math]::Max(5, [Math]::Ceiling($totalFiles * 0.005))

    $autoSelected = ($extCounts.GetEnumerator() |
            Where-Object { $_.Value -ge $threshold } |
            Select-Object -ExpandProperty Key)

    if ($extCounts.ContainsKey('md') -and 'md' -notin $autoSelected) {
        $autoSelected += 'md'
    }

    $suggested = Normalize-ExtensionList -Value (($autoSelected | Sort-Object) -join ',')
    Write-Host "`nSuggested extensions (>= $([int]$threshold) files): " -NoNewline
    Write-Host $suggested -ForegroundColor Green

    if ($Force) {
        $selectedExts = $suggested
        Write-Host 'Force enabled; accepting suggested extensions.' -ForegroundColor Yellow
    }
    else {
        $userInput = Read-Host '`nAccept suggested extensions, or enter your own (comma-separated) [Enter = accept]'
        if ([string]::IsNullOrWhiteSpace($userInput)) {
            $selectedExts = $suggested
        }
        else {
            $selectedExts = Normalize-ExtensionList -Value $userInput
        }
    }

    if (-not $selectedExts) {
        Write-Error 'No extensions were selected'
        exit 1
    }

    Write-Host "Using extensions: $selectedExts" -ForegroundColor Green
}

$selectedExts = Normalize-ExtensionList -Value $selectedExts
Write-Host "Normalized extensions: $selectedExts" -ForegroundColor Green

$xrayArgs = @(
    'serve',
    '--dir', $RepoPath,
    '--ext', $selectedExts,
    '--watch',
    '--definitions',
    '--metrics',
    '--debug-log'
)

$vscodeMcpDir = Join-Path $RepoPath '.vscode'
$vscodeMcpPath = Join-Path $vscodeMcpDir 'mcp.json'
$writeVscode = $true
$vscodeAction = 'create'

if (Test-Path $vscodeMcpPath) {
    try {
        $existingVscode = Get-Content -Path $vscodeMcpPath -Raw | ConvertFrom-Json
    }
    catch {
        $existingVscode = $null
    }

    if ($existingVscode -and $existingVscode.servers -and $existingVscode.servers.xray) {
        if (-not $Force) {
            Write-Warning "$vscodeMcpPath already has an 'xray' server entry"
            $writeVscode = Read-YesNo -Prompt 'Update it?' -Default $false
        }
        $vscodeAction = 'merge'
    }
    elseif ($existingVscode -and $existingVscode.servers) {
        $vscodeAction = 'merge'
    }
    else {
        if (-not $Force) {
            Write-Warning "$vscodeMcpPath exists but has no 'servers' key"
            $writeVscode = Read-YesNo -Prompt 'Overwrite?' -Default $false
        }
    }
}

if ($writeVscode) {
    if (-not (Test-Path $vscodeMcpDir)) {
        New-Item -ItemType Directory -Path $vscodeMcpDir -Force | Out-Null
    }

    Backup-McpJson -Path $vscodeMcpPath

    $xrayEntry = [ordered]@{
        type    = 'stdio'
        command = $xrayPath
        args    = $xrayArgs
    }

    if ($vscodeAction -eq 'merge' -and $existingVscode) {
        $existingVscode.servers | Add-Member -NotePropertyName 'xray' -NotePropertyValue $xrayEntry -Force
        $existingVscode | ConvertTo-Json -Depth 10 | Set-Content -Path $vscodeMcpPath -Encoding UTF8
        Write-Host "Updated xray entry in $vscodeMcpPath (other servers preserved)" -ForegroundColor Green
    }
    else {
        $vscodeConfig = [ordered]@{
            servers = [ordered]@{
                xray = $xrayEntry
            }
        }
        $vscodeConfig | ConvertTo-Json -Depth 5 | Set-Content -Path $vscodeMcpPath -Encoding UTF8
        Write-Host "Created $vscodeMcpPath" -ForegroundColor Green
    }
}

$rooMcpDir = Join-Path $RepoPath '.roo'
$rooMcpPath = Join-Path $rooMcpDir 'mcp.json'
$writeRoo = $false
$rooAction = 'create'

if ($EnableRoo) {
    $writeRoo = $true
}
elseif (-not $Force) {
    $writeRoo = Read-YesNo -Prompt 'Also configure for Roo Code?' -Default $false
}

if ($writeRoo -and (Test-Path $rooMcpPath)) {
    try {
        $existingRoo = Get-Content -Path $rooMcpPath -Raw | ConvertFrom-Json
    }
    catch {
        $existingRoo = $null
    }

    if ($existingRoo -and $existingRoo.mcpServers -and $existingRoo.mcpServers.xray) {
        if (-not $Force) {
            Write-Warning "$rooMcpPath already has an 'xray' server entry"
            $writeRoo = Read-YesNo -Prompt 'Update it?' -Default $false
        }
        $rooAction = 'merge'
    }
    elseif ($existingRoo -and $existingRoo.mcpServers) {
        $rooAction = 'merge'
    }
    else {
        if (-not $Force) {
            Write-Warning "$rooMcpPath exists but has no 'mcpServers' key"
            $writeRoo = Read-YesNo -Prompt 'Overwrite?' -Default $false
        }
    }
}

if ($writeRoo) {
    if (-not (Test-Path $rooMcpDir)) {
        New-Item -ItemType Directory -Path $rooMcpDir -Force | Out-Null
    }

    Backup-McpJson -Path $rooMcpPath

    $rooXrayEntry = [ordered]@{
        command     = $xrayPath
        args        = $xrayArgs
        alwaysAllow = $AllowedTools
        disabled    = $false
        timeout     = 300
    }

    if ($rooAction -eq 'merge' -and $existingRoo) {
        $existingRoo.mcpServers | Add-Member -NotePropertyName 'xray' -NotePropertyValue $rooXrayEntry -Force
        $existingRoo | ConvertTo-Json -Depth 10 | Set-Content -Path $rooMcpPath -Encoding UTF8
        Write-Host "Updated xray entry in $rooMcpPath (other servers preserved)" -ForegroundColor Green
    }
    else {
        $rooConfig = [ordered]@{
            mcpServers = [ordered]@{
                xray = $rooXrayEntry
            }
        }
        $rooConfig | ConvertTo-Json -Depth 5 | Set-Content -Path $rooMcpPath -Encoding UTF8
        Write-Host "Created $rooMcpPath" -ForegroundColor Green
    }
}

if ($isGitRepo) {
    Push-Location $RepoPath
    try {
        $skipWorktreeFiles = @()
        $excludedFiles = @()

        $excludePath = (git rev-parse --git-path info/exclude 2>$null)
        if (-not $excludePath -or -not (Test-Path (Split-Path $excludePath))) {
            $excludePath = $null
        }

        $existingExcludes = @()
        if ($excludePath -and (Test-Path $excludePath)) {
            $existingExcludes = Get-Content $excludePath -ErrorAction SilentlyContinue
        }

        foreach ($mcpFile in @(
                @{ Path = '.vscode/mcp.json'; Protect = (Test-Path (Join-Path $RepoPath '.vscode/mcp.json')) },
                @{ Path = '.roo/mcp.json'; Protect = ($writeRoo -and (Test-Path (Join-Path $RepoPath '.roo/mcp.json'))) }
            )) {
            if (-not $mcpFile.Protect) {
                continue
            }

            $rel = $mcpFile.Path
            $tracked = git ls-files $rel 2>$null
            if ($tracked) {
                git update-index --skip-worktree $rel 2>$null
                $skipWorktreeFiles += $rel
            }
            elseif ($excludePath -and $existingExcludes -notcontains $rel) {
                Add-Content -Path $excludePath -Value $rel -Encoding UTF8
                $excludedFiles += $rel
            }
        }

        if ($skipWorktreeFiles.Count -gt 0) {
            Write-Host "`nGit protection (--skip-worktree) applied to:" -ForegroundColor Cyan
            foreach ($file in $skipWorktreeFiles) {
                Write-Host "  $file (tracked, local edits hidden)" -ForegroundColor Green
            }
        }

        if ($excludedFiles.Count -gt 0) {
            Write-Host "`nGit protection (.git/info/exclude) applied to:" -ForegroundColor Cyan
            foreach ($file in $excludedFiles) {
                Write-Host "  $file (untracked, hidden from git status)" -ForegroundColor Green
            }
        }

        if (($skipWorktreeFiles + $excludedFiles).Count -gt 0) {
            Write-Host '  Files will not appear in git status/add/commit.' -ForegroundColor DarkGray
        }
    }
    finally {
        Pop-Location
    }
}

Write-Host "`n=== Setup complete ===" -ForegroundColor Cyan
Write-Host "xray binary:    $xrayPath"
Write-Host "Target repo:    $RepoPath"
Write-Host "Extensions:     $selectedExts"
Write-Host "Copilot config: $vscodeMcpPath"
if ($writeRoo) {
    Write-Host "Roo config:     $rooMcpPath"
}
Write-Host "`nReopen the repo in VS Code / Roo to activate xray MCP." -ForegroundColor Yellow
