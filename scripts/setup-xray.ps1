#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Downloads the latest xray release and configures MCP for a target repository.

.DESCRIPTION
    1. Downloads xray.exe from the latest GitHub release (if not already installed)
    2. Detects file extensions in the target repo heuristically
    3. Asks which MCP clients to configure and creates the corresponding mcp.json:
         - .vscode/mcp.json          (VS Code GitHub Copilot)
         - .roo/mcp.json             (Roo Code -- DISABLED: install/restore/uninstall paths commented out)
         - .mcp.json                 (GitHub Copilot CLI - canonical project-level path)
       No client is configured by default - each must be opted in either
       interactively or via -EnableVSCode / -EnableRoo / -EnableCopilotCli.
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

.PARAMETER EnableVSCode
    Create .vscode/mcp.json for VS Code GitHub Copilot. Without this, VS Code
    is prompted interactively unless -Force is used, in which case VS Code
    setup is skipped.

.PARAMETER EnableRoo
    DISABLED. Kept as a no-op switch for backward compat with caller scripts.
    The Roo install/restore/uninstall code paths in this script are commented
    out. Re-enable by uncommenting the Roo block in the script body and the
    .roo/mcp.json entries in the -Restore and -Uninstall blocks.

.PARAMETER EnableCopilotCli
    Create .mcp.json (in the repo root) for GitHub Copilot CLI. This is the
    canonical project-level path documented for Copilot CLI; .github/copilot/mcp.json
    and .github/mcp.json are NOT read by Copilot CLI as of writing.
    Without this, Copilot CLI is prompted interactively unless -Force is used,
    in which case Copilot CLI setup is skipped.

.PARAMETER Force
    Run non-interactively where possible: overwrite existing xray entry, overwrite
    existing xray.exe, accept suggested extensions, and skip every MCP client
    unless its corresponding -EnableVSCode / -EnableRoo / -EnableCopilotCli switch
    is passed. At least one of those switches must be supplied with -Force, otherwise
    the script exits with an error.
    Implies -KillRunning when xray.exe is locked by a running process.

.PARAMETER KillRunning
    If xray.exe is in use (e.g. running as an MCP server in another VS Code instance),
    terminate all running xray.exe processes without prompting before overwriting the
    binary. Without this switch, the script asks for confirmation interactively.

.PARAMETER Restore
    Restore .vscode/mcp.json and .mcp.json from the .bak files created on the
    previous setup run, then exit. Skips download and extension detection.
    Use this to undo the most recent setup-xray run.
    (.roo/mcp.json restore disabled along with the Roo install path.)

    Backup behavior: every regular setup run (without -Restore) copies the
    existing mcp.json files to <name>.bak before overwriting. The .bak file
    is replaced on each run, so it always reflects the file state immediately
    before the most recent setup. If no .bak exists when -Restore is invoked,
    the script exits with an error.

.PARAMETER Uninstall
    Fully remove xray from the target repository:
      - Strip the 'xray' server entry from .vscode/mcp.json and .mcp.json
        (other servers preserved). .roo/mcp.json is no longer touched (Roo
        path disabled); legacy Roo installs can still be cleaned up via
        -Restore (uses .bak files) or by hand.
      - If a config file would become empty AND was created by this script
        (no upstream content), delete it.
      - Lift git protection: 'git update-index --no-skip-worktree' on tracked
        files, and remove our entries from .git/info/exclude on untracked files.
      - Delete .bak files (unless -KeepBackups).
      - Delete the xray.exe binary in -InstallDir (unless -KeepBinary).
    Idempotent: re-running on a clean state is a no-op.
    Does NOT touch:
      - upstream-managed servers in any mcp.json
      - ~/.copilot/mcp-config.json (we never write there)
      - any other customization files

.PARAMETER KeepBackups
    With -Uninstall: do not delete .bak files.

.PARAMETER KeepBinary
    With -Uninstall: do not delete xray.exe.

.PARAMETER DryRun
    With -Uninstall: list what would be done without making changes.

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Extensions cs,sql,md -EnableVSCode -Force

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -EnableCopilotCli

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Restore

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall -DryRun

.EXAMPLE
    .\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall -KeepBinary -KeepBackups
#>
param(
    [string]$RepoPath,
    [string]$InstallDir = "$env:LOCALAPPDATA\xray",
    [string]$GithubRepo = 'pustynsky/xray',
    [string]$Extensions,
    [switch]$SkipDownload,
    [switch]$EnableVSCode,
    [switch]$EnableRoo,
    [switch]$EnableCopilotCli,
    [switch]$Force,
    [switch]$KillRunning,
    [switch]$Restore,
    [switch]$Uninstall,
    [switch]$KeepBackups,
    [switch]$KeepBinary,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

# PowerShell 7.4+ defaults `$PSNativeCommandUseErrorActionPreference` to $true,
# which turns any non-zero exit from a native command (including git probes
# like `git ls-files --error-unmatch <missing>`) into a terminating error and
# aborts the script even when stderr is redirected with `2>$null`. We always
# branch on `$LASTEXITCODE` explicitly, so opt out of the auto-throw.
$PSNativeCommandUseErrorActionPreference = $false

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

function Warn-McpMergeLossyFields {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Client,
        $ExistingXrayEntry,
        [string[]]$ScriptManagedFields
    )

    if (-not $ExistingXrayEntry) { return }

    $existingFields = @($ExistingXrayEntry.PSObject.Properties.Name)
    $managedSet = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $ScriptManagedFields | ForEach-Object { $null = $managedSet.Add($_) }

    $extra = $existingFields | Where-Object { -not $managedSet.Contains($_) }
    if ($extra) {
        Write-Warning ("$Client xray entry in $Path has extra field(s) [{0}] that will be REMOVED on merge. Backup is in $Path.bak." -f ($extra -join ', '))
    }

    $rawText = $null
    try { $rawText = Get-Content -Path $Path -Raw -ErrorAction Stop } catch {}
    if ($rawText -and ($rawText -match '(?m)^\s*//' -or $rawText -match '/\*')) {
        Write-Warning ("$Path contains JSONC comments which will be lost on rewrite. Backup is in $Path.bak.")
    }
}

function Remove-XrayServerEntry {
    <#
        Removes only the 'xray' server entry from an mcp.json file.
        Preserves any other servers.
        Returns one of: 'removed', 'removed-and-deleted-empty-file', 'no-xray-entry', 'absent', 'error'.

        $ContainerKey is the JSON key holding the servers map:
            'servers'    for VS Code
            'mcpServers' for Roo and Copilot CLI
    #>
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$ContainerKey,
        [switch]$DryRun
    )

    if (-not (Test-Path $Path)) {
        return 'absent'
    }

    try {
        $json = Get-Content -Path $Path -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        Write-Warning ("Cannot parse {0}: {1}. Leaving untouched." -f $Path, $_.Exception.Message)
        return 'error'
    }

    $container = $json.$ContainerKey
    if (-not $container -or -not ($container.PSObject.Properties.Name -contains 'xray')) {
        return 'no-xray-entry'
    }

    if ($DryRun) {
        Write-Host ("  [DryRun] Would remove 'xray' from {0}" -f $Path) -ForegroundColor DarkYellow
        return 'removed'
    }

    $container.PSObject.Properties.Remove('xray')

    # PSObject.Properties.Name returns $null when the collection is empty.
    # Wrapping null in @() yields an array of length 1 containing $null, so
    # an unfiltered .Count check would never report 'empty'. Filter $null out
    # so the auto-delete-empty-file branch fires correctly.
    $remainingServers = @($container.PSObject.Properties.Name | Where-Object { $null -ne $_ })
    if ($remainingServers.Count -eq 0) {
        # Container is now empty. If the file ALSO has no other top-level keys,
        # the file is purely ours and can be deleted entirely.
        $otherTopLevelKeys = @($json.PSObject.Properties.Name | Where-Object { $_ -ne $ContainerKey })
        if ($otherTopLevelKeys.Count -eq 0) {
            Remove-Item -Path $Path -Force
            return 'removed-and-deleted-empty-file'
        }
    }

    $json | ConvertTo-Json -Depth 10 | Set-Content -Path $Path -Encoding UTF8
    return 'removed'
}

function Remove-GitProtection {
    <#
        Lifts the git protection installed by setup-xray:
          - For tracked files under skip-worktree: clears the bit.
          - For untracked files in .git/info/exclude: removes our line.
        Run from inside the repo working tree.
        Returns one of: 'lifted-skip-worktree', 'removed-from-exclude', 'not-protected', 'error'.
    #>
    param(
        [Parameter(Mandatory)] [string]$RelativePath,
        [string]$ExcludePath,
        [switch]$DryRun
    )

    # Tracked? Check skip-worktree state via `git ls-files -v`.
    $tracked = $null
    try {
        $tracked = & git ls-files $RelativePath 2>$null
        if ($LASTEXITCODE -ne 0) { $tracked = $null }
    }
    catch {
        $tracked = $null
    }

    if ($tracked) {
        $verbose = & git ls-files -v $RelativePath 2>&1
        # `git ls-files -v` flags: 'S' = skip-worktree, 'h' = assume-unchanged,
        # 'H' = normal tracked. We want to clear protection on either of the
        # first two. PowerShell's `-match` is case-insensitive by default, so
        # the explicit `[Sh]` (rather than the looser `[a-z]`) avoids picking
        # up any future flag letter that might be added by git.
        if ($verbose -match '^[Sh] ') {
            if ($DryRun) {
                Write-Host ("  [DryRun] Would 'git update-index --no-skip-worktree' {0}" -f $RelativePath) -ForegroundColor DarkYellow
                return 'lifted-skip-worktree'
            }
            & git update-index --no-skip-worktree $RelativePath 2>&1 | Out-Null
            if ($LASTEXITCODE -eq 0) {
                return 'lifted-skip-worktree'
            }
            Write-Warning ("git update-index --no-skip-worktree {0} failed (exit {1})" -f $RelativePath, $LASTEXITCODE)
            return 'error'
        }
        return 'not-protected'
    }

    # Untracked: check .git/info/exclude.
    if (-not $ExcludePath -or -not (Test-Path $ExcludePath)) {
        return 'not-protected'
    }

    $lines = Get-Content -Path $ExcludePath -ErrorAction SilentlyContinue
    if (-not $lines -or $lines -notcontains $RelativePath) {
        return 'not-protected'
    }

    if ($DryRun) {
        Write-Host ("  [DryRun] Would remove '{0}' from {1}" -f $RelativePath, $ExcludePath) -ForegroundColor DarkYellow
        return 'removed-from-exclude'
    }

    $newLines = $lines | Where-Object { $_ -ne $RelativePath }
    Set-Content -Path $ExcludePath -Value $newLines -Encoding UTF8
    return 'removed-from-exclude'
}

# ----------------------------------------------------------------------------
# .mcp.json smudge/clean filter (Copilot CLI canonical config, tracked upstream)
# ----------------------------------------------------------------------------
# See docs/mcp-filter-design.md for the full design.
# Summary:
#   - .mcp.json is tracked upstream (the project ships baseline MCP servers).
#   - We need to add a per-machine 'xray' entry without committing it AND
#     without breaking 'git pull' when upstream modifies the file.
#   - skip-worktree/assume-unchanged FAIL this (pull aborts with 'local
#     changes would be overwritten' when upstream modifies the file).
#   - Solution: install a smudge/clean filter that injects the 'xray' entry
#     on checkout and strips it on commit. Round-trip is byte-exact.
#
# Sentinel marker: every injected line carries the field
#     "_xrayMcpMarker": "managed-by-setup-xray.ps1-do-not-edit"
# clean.sh removes any line containing this marker.

$Script:XrayMcpMarker = 'managed-by-setup-xray.ps1-do-not-edit'

function New-XraySnapshotLine {
    <#
        Builds the single-line JSON entry that the smudge filter injects into
        .mcp.json. Indented with 4 spaces to match common .mcp.json formatting
        (mcpServers content nested two levels deep).
    #>
    param(
        [Parameter(Mandatory)] [string]$XrayPath,
        [Parameter(Mandatory)] [string[]]$XrayArgs
    )

    $argsJson = (@($XrayArgs) | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress }) -join ','
    $cmdJson  = ConvertTo-Json -InputObject $XrayPath -Compress
    $markerJson = ConvertTo-Json -InputObject $Script:XrayMcpMarker -Compress
    return ('    "xray":{"command":' + $cmdJson + ',"args":[' + $argsJson + '],"env":{},"_xrayMcpMarker":' + $markerJson + '}')
}

function Get-ResolvedGitDir {
    param([Parameter(Mandatory)] [string]$RepoRoot)

    Push-Location $RepoRoot
    try {
        $gd = & git rev-parse --git-dir 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $gd) { return $null }
        if (-not [IO.Path]::IsPathRooted($gd)) { $gd = Join-Path $RepoRoot $gd }
        return (Resolve-Path $gd).Path
    }
    finally {
        Pop-Location
    }
}

function Test-IsTrackedFile {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$RelativePath
    )

    Push-Location $RepoRoot
    try {
        # `git ls-files --error-unmatch` is the standard probe for "is this
        # path tracked?". It exits 0 for tracked, 1 (with stderr "error:
        # pathspec ... did not match") for untracked. The stderr message is
        # the *expected* answer, not a real error.
        #
        # On PowerShell 7.3+ a plain `2>$null` does NOT silence native-command
        # stderr — PowerShell wraps it in a NativeCommandError ErrorRecord
        # before the redirect can swallow it, and the user sees the leak on
        # the console even though the script continues normally (the parent
        # `$PSNativeCommandUseErrorActionPreference = $false` set near the
        # top of this script suppresses the *terminating* abort, but not the
        # write to the error stream).
        #
        # `2>&1` merges stderr into the success stream BEFORE the wrapping
        # happens, and `$null = …` discards both streams. This works on
        # PowerShell 5.1, 7.0, 7.3, 7.4+ identically.
        $null = & git ls-files --error-unmatch -- $RelativePath 2>&1
        return $LASTEXITCODE -eq 0
    }
    finally {
        Pop-Location
    }
}

function Set-McpFileWithSnapshot {
    <#
        Writes .mcp.json so that it contains the snapshot line as the FIRST
        entry of mcpServers, preserving any existing servers and any other
        top-level keys verbatim.

        Behavior:
          - File missing: create a fresh minimal .mcp.json containing only xray.
          - Marker already present: replace existing snapshot line in place.
          - mcpServers present (multi-line): inject snapshot line after the
            line that opens the object.
          - mcpServers inline ({} on one line) or missing: warn and bail.
            The filter will passthrough; user's xray entry will not be
            registered until they reformat the file or re-run with no
            existing file.

        Output preserves the file's existing dominant line separator (CRLF
        if any CRLF is present in the input, otherwise LF). This matches the
        smudge filter's separator-detection behavior, so the round-trip
        property `clean(smudge(canonical)) == canonical` is preserved
        byte-exact even on repos whose tracked .mcp.json uses CRLF.
        New files (no existing input) default to LF.
    #>
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$SnapshotLine
    )

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    if (-not (Test-Path $Path)) {
        $body = "{`n  ""mcpServers"": {`n$SnapshotLine`n  }`n}`n"
        [IO.File]::WriteAllText($Path, $body, $utf8NoBom)
        return $true
    }

    $raw = [IO.File]::ReadAllText($Path, $utf8NoBom)
    $sep = if ($raw -match "`r`n") { "`r`n" } else { "`n" }

    # Normalize internally to LF for the line-walking logic, but remember
    # $sep so we can emit with the original separator.
    $normalized = $raw -replace "`r`n", "`n"

    # Strip any pre-existing marker line(s).
    if ($normalized -match '_xrayMcpMarker') {
        $kept = @(($normalized -split "`n") | Where-Object { $_ -notmatch '_xrayMcpMarker' })
        $normalized = ($kept -join "`n")
    }

    $lines = $normalized -split "`n"
    $newLines = New-Object System.Collections.Generic.List[string]
    $injected = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $newLines.Add($lines[$i])
        if (-not $injected -and $lines[$i] -match '"mcpServers"\s*:\s*\{\s*$') {
            # Determine whether mcpServers is empty by peeking ahead.
            $j = $i + 1
            while ($j -lt $lines.Length -and $lines[$j] -match '^\s*$') { $j++ }
            $emptyServers = ($j -lt $lines.Length -and $lines[$j] -match '^\s*\}\s*,?\s*$')
            if ($emptyServers) {
                $newLines.Add($SnapshotLine)
            }
            else {
                $newLines.Add($SnapshotLine + ',')
            }
            $injected = $true
        }
    }

    if (-not $injected) {
        Write-Warning ("Could not find an injection point for xray in {0} (mcpServers may be inline `{{}}` or absent). Filter will passthrough; xray entry NOT registered." -f $Path)
        return $false
    }

    [IO.File]::WriteAllText($Path, ($newLines -join $sep), $utf8NoBom)
    return $true
}

function Install-McpFilter {
    <#
        Installs the smudge/clean filter for .mcp.json in the given repo:
          - Creates <git-common-dir>/xray-mcp/ with smudge.sh, clean.sh,
            snapshot.txt (LF-normalized; bash heredocs / set -e are
            sensitive to CRLF in script bodies).
          - Sets .git/config [filter "xray-mcp"] smudge/clean/required.
            The smudge/clean commands are stored as
            `bash "$(git rev-parse --git-common-dir)/xray-mcp/<name>.sh"`
            (NOT bare paths and NOT --git-dir): explicit `bash` avoids the
            need for the executable bit on macOS/Linux, and
            --git-common-dir resolves correctly inside linked worktrees
            where the per-worktree --git-dir does NOT contain the scripts.
          - Adds '.mcp.json filter=xray-mcp' to <git-common-dir>/info/attributes
            (idempotent; replaces any prior .mcp.json line). NO `text eol=lf`
            attribute: the perl-based filter preserves CRLF byte-exact.
        Returns $true on success, $false on failure (with a warning). Each
        git config write checks $LASTEXITCODE; a failure short-circuits and
        triggers the caller's fail-closed rollback path.

        Source scripts are copied from $ScriptDir/mcp-filter/{smudge,clean}.sh
        so both production install and dev iteration use the same files.
    #>
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$ScriptDir,
        [Parameter(Mandatory)] [string]$SnapshotLine
    )

    $resolvedGitDir = Get-ResolvedGitDir -RepoRoot $RepoRoot
    if (-not $resolvedGitDir) {
        Write-Warning 'git rev-parse --git-dir failed - cannot install filter.'
        return $false
    }

    $srcFilterDir = Join-Path $ScriptDir 'mcp-filter'
    foreach ($name in @('smudge.sh', 'clean.sh')) {
        if (-not (Test-Path (Join-Path $srcFilterDir $name))) {
            Write-Warning ("Filter source missing: {0}" -f (Join-Path $srcFilterDir $name))
            return $false
        }
    }

    $filterDir = Join-Path $resolvedGitDir 'xray-mcp'
    if (-not (Test-Path $filterDir)) {
        New-Item -ItemType Directory -Path $filterDir -Force | Out-Null
    }

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    # Copy filter scripts, normalizing to LF (bash on Windows tolerates CRLF
    # in scripts but it can break heredocs and trailing-newline-sensitive
    # constructs).
    foreach ($name in @('smudge.sh', 'clean.sh')) {
        $src = Join-Path $srcFilterDir $name
        $dst = Join-Path $filterDir $name
        $body = ([IO.File]::ReadAllText($src, $utf8NoBom)) -replace "`r`n", "`n"
        [IO.File]::WriteAllText($dst, $body, $utf8NoBom)
    }

    # Snapshot.
    [IO.File]::WriteAllText((Join-Path $filterDir 'snapshot.txt'), $SnapshotLine, $utf8NoBom)

    # Wire into .git/config.
    #
    # Important #1: configure the filter as `bash <path>` rather than the
    # bare script path. On macOS/Linux, git invokes the filter command via
    # `sh -c '<command>'`, which requires the script to either have the
    # executable bit set OR to be invoked through an explicit interpreter.
    # We can't rely on the executable bit because:
    #   * Windows filesystems don't carry it,
    #   * `IO.File.WriteAllText` doesn't set it,
    #   * `git update-index --chmod=+x` only works on tracked paths (filter
    #     scripts are inside .git/, never tracked).
    # Using `bash <path>` works on every platform git supports (Git for
    # Windows ships bash; macOS/Linux have it natively).
    #
    # Important #2: resolve the script path at filter-invocation time via
    # `$(git rev-parse --git-common-dir)`. We must use --git-COMMON-dir, not
    # plain --git-dir, because LINKED WORKTREES (`git worktree add`) have a
    # per-worktree gitdir at `.git/worktrees/<name>/` — `--git-dir` returns
    # THAT, where the filter scripts do NOT live (they live in the main
    # gitdir). `--git-common-dir` returns the SHARED main gitdir for both
    # the primary worktree and any linked worktree, so the filter command
    # resolves to the correct script in both cases. The `$(...)` is single-
    # quoted into git config so PowerShell does NOT expand it; git stores it
    # verbatim, and `sh -c <command>` expands it at filter time.
    Push-Location $RepoRoot
    try {
        $smudgeCmd = 'bash "$(git rev-parse --git-common-dir)/xray-mcp/smudge.sh"'
        $cleanCmd  = 'bash "$(git rev-parse --git-common-dir)/xray-mcp/clean.sh"'

        & git config --local 'filter.xray-mcp.smudge' $smudgeCmd 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.xray-mcp.smudge failed (exit {0})" -f $LASTEXITCODE)
            return $false
        }

        & git config --local 'filter.xray-mcp.clean' $cleanCmd 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.xray-mcp.clean failed (exit {0})" -f $LASTEXITCODE)
            return $false
        }

        & git config --local --bool 'filter.xray-mcp.required' false 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.xray-mcp.required failed (exit {0})" -f $LASTEXITCODE)
            return $false
        }
    }
    finally {
        Pop-Location
    }

    # Add attributes line (idempotent).
    $attrFile = Join-Path $resolvedGitDir 'info\attributes'
    $attrDir = Split-Path -Parent $attrFile
    if (-not (Test-Path $attrDir)) {
        New-Item -ItemType Directory -Path $attrDir -Force | Out-Null
    }
    $attrLine = '.mcp.json filter=xray-mcp'
    $existingAttrs = @()
    if (Test-Path $attrFile) {
        $existingAttrs = @(Get-Content $attrFile -ErrorAction SilentlyContinue)
    }
    $existingAttrs = @($existingAttrs | Where-Object { $_ -notmatch '^\s*\.mcp\.json\s' })
    $existingAttrs += $attrLine
    Set-Content -Path $attrFile -Value $existingAttrs -Encoding UTF8

    return $true
}

function Uninstall-McpFilter {
    <#
        Tears down the smudge/clean filter installed by Install-McpFilter:
          - Strips snapshot line from .mcp.json (so the working tree matches
            the canonical / upstream form).
          - Removes [filter "xray-mcp"] section from .git/config.
          - Removes the .mcp.json line from .git/info/attributes.
          - Deletes .git/xray-mcp/ directory.
        Idempotent: calling on a clean state is a no-op.
        Returns one of: 'removed', 'not-installed', 'error'.
    #>
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [switch]$DryRun
    )

    $resolvedGitDir = Get-ResolvedGitDir -RepoRoot $RepoRoot
    if (-not $resolvedGitDir) {
        return 'not-installed'
    }

    $filterDir = Join-Path $resolvedGitDir 'xray-mcp'
    $attrFile = Join-Path $resolvedGitDir 'info\attributes'
    $mcpFile = Join-Path $RepoRoot '.mcp.json'

    $hadAnything = $false

    # 1. Strip snapshot from .mcp.json (pure text op; doesn't depend on git or bash).
    if (Test-Path $mcpFile) {
        $utf8NoBom = [Text.UTF8Encoding]::new($false)
        $raw = [IO.File]::ReadAllText($mcpFile, $utf8NoBom)
        if ($raw -match '_xrayMcpMarker') {
            $hadAnything = $true
            if ($DryRun) {
                Write-Host ("  [DryRun] Would strip xray snapshot line from {0}" -f $mcpFile) -ForegroundColor DarkYellow
            }
            else {
                $sep = if ($raw -match "`r`n") { "`r`n" } else { "`n" }
                $kept = @(($raw -split "`r?`n") | Where-Object { $_ -notmatch '_xrayMcpMarker' })
                [IO.File]::WriteAllText($mcpFile, ($kept -join $sep), $utf8NoBom)
            }
        }
    }

    # 2. Remove .git/config filter section.
    Push-Location $RepoRoot
    try {
        $hasSection = $false
        & git config --local --get-regexp '^filter\.xray-mcp\.' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $hasSection = $true }

        if ($hasSection) {
            $hadAnything = $true
            if ($DryRun) {
                Write-Host '  [DryRun] Would remove [filter "xray-mcp"] from .git/config' -ForegroundColor DarkYellow
            }
            else {
                # Capture exit code: a failed `--remove-section` (e.g.,
                # locked .git/config, permission error) must NOT be silently
                # swallowed — leaving the filter section in place after the
                # scripts are deleted would cause every future git checkout
                # to invoke a missing command and either fail loudly or, with
                # required=false, silently passthrough (bad either way).
                & git config --local --remove-section 'filter.xray-mcp' 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    Write-Warning ("git config --remove-section filter.xray-mcp failed (exit {0}); .git/config may still reference the filter" -f $LASTEXITCODE)
                    $script:_McpFilterRollbackError = $true
                }
            }
        }
    }
    finally {
        Pop-Location
    }

    if ($script:_McpFilterRollbackError) {
        $script:_McpFilterRollbackError = $false
        return 'error'
    }

    # 3. Remove the .mcp.json line from .git/info/attributes.
    if (Test-Path $attrFile) {
        $existingAttrs = @(Get-Content $attrFile -ErrorAction SilentlyContinue)
        $filteredAttrs = @($existingAttrs | Where-Object { $_ -notmatch '^\s*\.mcp\.json\s.*filter=xray-mcp' })
        if ($existingAttrs.Count -ne $filteredAttrs.Count) {
            $hadAnything = $true
            if ($DryRun) {
                Write-Host ("  [DryRun] Would remove .mcp.json line from {0}" -f $attrFile) -ForegroundColor DarkYellow
            }
            else {
                Set-Content -Path $attrFile -Value $filteredAttrs -Encoding UTF8
            }
        }
    }

    # 4. Delete .git/xray-mcp/.
    if (Test-Path $filterDir) {
        $hadAnything = $true
        if ($DryRun) {
            Write-Host ("  [DryRun] Would delete {0}" -f $filterDir) -ForegroundColor DarkYellow
        }
        else {
            Remove-Item -Path $filterDir -Recurse -Force
        }
    }

    if (-not $hadAnything) { return 'not-installed' }
    return 'removed'
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

if ($Force -and -not ($EnableVSCode -or $EnableRoo -or $EnableCopilotCli) -and -not $Restore) {
    Write-Error '-Force requires at least one of -EnableVSCode, -EnableRoo, -EnableCopilotCli (no MCP client would be configured otherwise).'
    exit 1
}

if ($Restore) {
    Write-Host "`n=== Restoring MCP configs in: $RepoPath ===" -ForegroundColor Cyan

    $vscodeMcpPath = Join-Path $RepoPath '.vscode\mcp.json'
    # Roo support disabled: $rooMcpPath = Join-Path $RepoPath '.roo\mcp.json'
    $copilotCliMcpPath = Join-Path $RepoPath '.mcp.json'

    $restoredAny = $false
    if (Restore-McpJson -Path $vscodeMcpPath) { $restoredAny = $true }
    # Roo support disabled: if (Restore-McpJson -Path $rooMcpPath) { $restoredAny = $true }
    if (Restore-McpJson -Path $copilotCliMcpPath) { $restoredAny = $true }

    if (-not $restoredAny) {
        Write-Error 'No .bak files found. Nothing to restore.'
        exit 1
    }

    Write-Host "`n=== Restore complete ===" -ForegroundColor Cyan
    exit 0
}

if ($Uninstall) {
    $action = if ($DryRun) { 'Dry-run uninstall plan for' } else { 'Uninstalling xray from' }
    Write-Host "`n=== $action`: $RepoPath ===" -ForegroundColor Cyan

    $isGitRepo = Test-Path (Join-Path $RepoPath '.git')
    $excludePath = $null
    if ($isGitRepo) {
        Push-Location $RepoPath
        try {
            $excludePath = & git rev-parse --git-path info/exclude 2>$null
            if ($LASTEXITCODE -ne 0) { $excludePath = $null }
        }
        finally {
            Pop-Location
        }
    }

    # 1. Tear down the smudge/clean filter for .mcp.json (if installed).
    # When the filter is in use, the snapshot-line strip already removes our
    # entry without disturbing upstream's formatting. We then skip the
    # JSON-parse-and-rewrite step for .mcp.json (which would reformat
    # args arrays) and only handle .vscode/mcp.json and .roo/mcp.json that way.
    $filterHandled = $false
    if ($isGitRepo) {
        $filterStatus = Uninstall-McpFilter -RepoRoot $RepoPath -DryRun:$DryRun
        switch ($filterStatus) {
            'removed'        { Write-Host '  Removed .mcp.json smudge/clean filter.' -ForegroundColor Green; $filterHandled = $true }
            'not-installed'  { Write-Host '  No .mcp.json filter installed (skip).' -ForegroundColor DarkGray }
            default          { Write-Warning "Filter teardown returned: $filterStatus" }
        }
    }

    # 2. Strip 'xray' entries from each known mcp config (preserving other servers).
    # Skip .mcp.json if the filter teardown already handled it (avoids
    # ConvertTo-Json reformatting upstream's args arrays).
    # Roo support disabled: .roo/mcp.json entry kept commented out so legacy installs
    # can still be cleaned up via -Restore (which uses .bak files), but the new
    # uninstall flow no longer touches Roo.
    $configs = @(
        @{ Path = (Join-Path $RepoPath '.vscode\mcp.json'); Container = 'servers';    Label = 'VS Code (.vscode/mcp.json)'; SkipIfFilterHandled = $false },
        # @{ Path = (Join-Path $RepoPath '.roo\mcp.json');    Container = 'mcpServers'; Label = 'Roo (.roo/mcp.json)';        SkipIfFilterHandled = $false },
        @{ Path = (Join-Path $RepoPath '.mcp.json');        Container = 'mcpServers'; Label = 'Copilot CLI (.mcp.json)';    SkipIfFilterHandled = $true  }
    )
    foreach ($cfg in $configs) {
        if ($cfg.SkipIfFilterHandled -and $filterHandled) {
            Write-Host ("  Skipping JSON rewrite for {0} (filter teardown already removed xray)" -f $cfg.Label) -ForegroundColor DarkGray
            continue
        }
        $status = Remove-XrayServerEntry -Path $cfg.Path -ContainerKey $cfg.Container -DryRun:$DryRun
        switch ($status) {
            'removed'                          { Write-Host ("  Removed xray from {0}" -f $cfg.Label) -ForegroundColor Green }
            'removed-and-deleted-empty-file'   { Write-Host ("  Removed xray and deleted now-empty {0}" -f $cfg.Label) -ForegroundColor Green }
            'no-xray-entry'                    { Write-Host ("  No xray entry in {0} (skip)" -f $cfg.Label) -ForegroundColor DarkGray }
            'absent'                           { Write-Host ("  {0} absent (skip)" -f $cfg.Label) -ForegroundColor DarkGray }
            'error'                            { } # Already warned by helper.
        }
    }

    # 3. Lift legacy git protection (skip-worktree / .git/info/exclude entries).
    # Roo support disabled: '.roo/mcp.json' is no longer touched by uninstall.
    if ($isGitRepo) {
        Push-Location $RepoPath
        try {
            foreach ($rel in @('.vscode/mcp.json', '.mcp.json')) {
                $protStatus = Remove-GitProtection -RelativePath $rel -ExcludePath $excludePath -DryRun:$DryRun
                switch ($protStatus) {
                    'lifted-skip-worktree'   { Write-Host ("  Lifted skip-worktree on {0}" -f $rel) -ForegroundColor Green }
                    'removed-from-exclude'   { Write-Host ("  Removed {0} from .git/info/exclude" -f $rel) -ForegroundColor Green }
                    'not-protected'          { Write-Host ("  {0} not protected (skip)" -f $rel) -ForegroundColor DarkGray }
                    'error'                  { } # Already warned by helper.
                }
            }
        }
        finally {
            Pop-Location
        }
    }

    # 4. Refresh git's stat-cache so the file no longer appears modified.
    # We just rewrote .mcp.json (stripping the snapshot line); without this
    # refresh, git status would show 'M' until the next git operation that
    # naturally updates stat info.
    if ($isGitRepo -and -not $DryRun -and (Test-Path (Join-Path $RepoPath '.mcp.json'))) {
        Push-Location $RepoPath
        try {
            & git update-index --refresh -- '.mcp.json' 2>$null | Out-Null
        }
        finally {
            Pop-Location
        }
    }

    # 5. Delete .bak files unless -KeepBackups.
    if (-not $KeepBackups) {
        foreach ($cfg in $configs) {
            $bak = "$($cfg.Path).bak"
            if (Test-Path $bak) {
                if ($DryRun) {
                    Write-Host ("  [DryRun] Would delete {0}" -f $bak) -ForegroundColor DarkYellow
                }
                else {
                    Remove-Item -Path $bak -Force -ErrorAction SilentlyContinue
                    Write-Host ("  Deleted {0}" -f $bak) -ForegroundColor Green
                }
            }
        }
    }
    else {
        Write-Host '  -KeepBackups: leaving .bak files in place.' -ForegroundColor DarkGray
    }

    # 6. Delete xray.exe unless -KeepBinary.
    $xrayBinPath = Join-Path $InstallDir 'xray.exe'
    if (-not $KeepBinary -and (Test-Path $xrayBinPath)) {
        if ($DryRun) {
            Write-Host ("  [DryRun] Would delete {0}" -f $xrayBinPath) -ForegroundColor DarkYellow
        }
        else {
            try {
                Remove-Item -Path $xrayBinPath -Force -ErrorAction Stop
                Write-Host ("  Deleted {0}" -f $xrayBinPath) -ForegroundColor Green
            }
            catch {
                Write-Warning ("Could not delete {0}: {1} (process may be running)" -f $xrayBinPath, $_.Exception.Message)
            }
        }
    }
    elseif ($KeepBinary) {
        Write-Host '  -KeepBinary: leaving xray.exe in place.' -ForegroundColor DarkGray
    }

    if ($DryRun) {
        Write-Host "`n=== Dry-run complete (no changes made) ===" -ForegroundColor Cyan
    }
    else {
        Write-Host "`n=== Uninstall complete ===" -ForegroundColor Cyan
    }
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

        # Windows refuses to overwrite a running .exe. If xray.exe is in use
        # (typically an MCP server spawned by another VS Code / Roo instance),
        # terminate the running processes first - either silently when the user
        # opts in via -KillRunning / -Force, or after an interactive prompt.
        try {
            Move-Item $downloaded $xrayPath -Force -ErrorAction Stop
        }
        catch [System.IO.IOException] {
            $running = @(Get-Process -Name 'xray' -ErrorAction SilentlyContinue)
            if ($running.Count -eq 0) {
                Remove-Item $tempDir -Recurse -Force
                Write-Error "Cannot replace $xrayPath but no xray.exe process is running. Original error: $($_.Exception.Message)"
                exit 1
            }

            Write-Host "$xrayPath is in use by $($running.Count) running xray.exe process(es):" -ForegroundColor Yellow
            $running | ForEach-Object { Write-Host ("  PID $($_.Id)  $($_.Path)") -ForegroundColor Yellow }

            $kill = $KillRunning -or (Read-YesNo -Prompt 'Kill these processes and continue?' -Default $false -ForceYes:$Force)
            if (-not $kill) {
                Remove-Item $tempDir -Recurse -Force
                Write-Error "Aborted: xray.exe is in use. Re-run with -KillRunning to terminate it automatically, or close the MCP hosts manually."
                exit 1
            }

            foreach ($proc in $running) {
                try {
                    Stop-Process -Id $proc.Id -Force -ErrorAction Stop
                    Write-Host "Stopped PID $($proc.Id)." -ForegroundColor DarkYellow
                }
                catch {
                    Remove-Item $tempDir -Recurse -Force
                    Write-Error "Failed to stop PID $($proc.Id): $($_.Exception.Message)"
                    exit 1
                }
            }

            # Give Windows a moment to release the file handle.
            Start-Sleep -Milliseconds 500
            Move-Item $downloaded $xrayPath -Force
            Write-Host 'Note: MCP hosts that were using xray will need to restart it.' -ForegroundColor Yellow
        }
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
$writeVscode = $false
$vscodeAction = 'create'

if ($EnableVSCode) {
    $writeVscode = $true
}
elseif (-not $Force) {
    $writeVscode = Read-YesNo -Prompt 'Configure for VS Code (GitHub Copilot)?' -Default $false
}

if ($writeVscode -and (Test-Path $vscodeMcpPath)) {
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
        Warn-McpMergeLossyFields -Path $vscodeMcpPath -Client 'VS Code' -ExistingXrayEntry $existingVscode.servers.xray -ScriptManagedFields @('type','command','args')
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
# Roo support disabled. The whole Roo install block below is commented out;
# $writeRoo stays $false so the downstream git-protect loop is a no-op for Roo,
# and so the -EnableRoo switch (kept for backward compat in caller scripts) is
# silently ignored. Re-enable by uncommenting both this block and the
# .roo/mcp.json entries in the -Restore and -Uninstall blocks above.
$writeRoo = $false

<#
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
        Warn-McpMergeLossyFields -Path $rooMcpPath -Client 'Roo' -ExistingXrayEntry $existingRoo.mcpServers.xray -ScriptManagedFields @('command','args','alwaysAllow','disabled','timeout')
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
#>

# Copilot CLI canonical project-level path is .mcp.json in the repo root.
# .github/copilot/mcp.json and .github/mcp.json are NOT read by Copilot CLI
# as of this writing (verified empirically against the official CLI).
$copilotCliMcpPath = Join-Path $RepoPath '.mcp.json'
$writeCopilotCli = $false
$copilotCliAction = 'create'

if ($EnableCopilotCli) {
    $writeCopilotCli = $true
}
elseif (-not $Force) {
    $writeCopilotCli = Read-YesNo -Prompt 'Also configure for GitHub Copilot CLI?' -Default $false
}

if ($writeCopilotCli -and (Test-Path $copilotCliMcpPath)) {
    try {
        $existingCopilotCli = Get-Content -Path $copilotCliMcpPath -Raw | ConvertFrom-Json
    }
    catch {
        $existingCopilotCli = $null
    }

    if ($existingCopilotCli -and $existingCopilotCli.mcpServers -and $existingCopilotCli.mcpServers.xray) {
        if (-not $Force) {
            Write-Warning "$copilotCliMcpPath already has an 'xray' server entry"
            $writeCopilotCli = Read-YesNo -Prompt 'Update it?' -Default $false
        }
        $copilotCliAction = 'merge'
    }
    elseif ($existingCopilotCli -and $existingCopilotCli.mcpServers) {
        $copilotCliAction = 'merge'
    }
    else {
        if (-not $Force) {
            Write-Warning "$copilotCliMcpPath exists but has no 'mcpServers' key"
            $writeCopilotCli = Read-YesNo -Prompt 'Overwrite?' -Default $false
        }
    }
}

if ($writeCopilotCli) {
    # .mcp.json lives in the repo root, which always exists - no mkdir needed.
    Backup-McpJson -Path $copilotCliMcpPath

    # Decide between two strategies:
    #   - Filter strategy (.mcp.json is tracked OR exists with upstream content
    #     in a git repo): install smudge/clean filter so 'git pull' merges
    #     upstream changes silently and 'git status' stays clean.
    #   - Plain JSON strategy (no git repo, OR file is untracked AND we're
    #     creating it fresh): merge xray entry via JSON parse+rewrite, fall
    #     back on .git/info/exclude or skip-worktree (handled later in the
    #     git-protect block).
    $useFilterStrategy = $false
    $copilotCliFilterInstalled = $false
    if ($isGitRepo) {
        $isTracked = Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath '.mcp.json'
        $useFilterStrategy = [bool]$isTracked
    }

    if ($useFilterStrategy) {
        $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
        $snapshotLine = New-XraySnapshotLine -XrayPath $xrayPath -XrayArgs $xrayArgs

        # Lift any pre-existing skip-worktree from a legacy install BEFORE we
        # touch the file - otherwise our write would silently no-op in the index.
        Push-Location $RepoPath
        try {
            & git update-index --no-skip-worktree '.mcp.json' 2>$null | Out-Null
        }
        finally {
            Pop-Location
        }

        if (Install-McpFilter -RepoRoot $RepoPath -ScriptDir $scriptDir -SnapshotLine $snapshotLine) {
            $copilotCliFilterInstalled = $true
            $injected = Set-McpFileWithSnapshot -Path $copilotCliMcpPath -SnapshotLine $snapshotLine
            if ($injected) {
                # Renormalize so the index reflects the canonical (clean) form.
                # Without this, the just-written enriched file would appear as
                # 'modified' in git status until the next checkout.
                Push-Location $RepoPath
                try {
                    & git add --renormalize '.mcp.json' 2>&1 | Out-Null
                    if ($LASTEXITCODE -ne 0) {
                        Write-Warning ("git add --renormalize .mcp.json failed (exit {0}). The xray line may show as a diff until the next clean filter pass." -f $LASTEXITCODE)
                    }
                }
                finally {
                    Pop-Location
                }
                Write-Host "Configured xray in $copilotCliMcpPath via smudge/clean filter (git status stays clean across upstream pulls)." -ForegroundColor Green
            }
        }
        else {
            # FAIL-CLOSED: the upstream-tracked .mcp.json must NOT be rewritten
            # via ConvertTo-Json (which loses single-element-array structure,
            # rewrites whitespace, and reintroduces the very pull-abort/diff
            # damage class that the filter migration replaced). The legacy
            # skip-worktree fallback is also unsafe here because any future
            # `git stash`/`reset --hard`/`checkout` would silently lift the
            # bit and expose the xray entry as a real diff.
            #
            # Roll back any partial filter artifacts and abort the install.
            Write-Warning 'Filter installation failed for the tracked upstream .mcp.json.'
            Write-Warning 'Refusing to fall back to JSON merge + skip-worktree (would corrupt upstream formatting and create silent diff hazards on future git operations).'
            Write-Warning 'Rolling back any partial filter artifacts and aborting the Copilot CLI install for this repo.'
            $rollbackResult = Uninstall-McpFilter -RepoRoot $RepoPath
            Write-Warning ("Rollback result: {0}. Investigate the Install-McpFilter warning above (likely missing bash, missing filter source files, or .git/info not writable) and re-run." -f $rollbackResult)
            $writeCopilotCli = $false
        }
    }

    if (-not $useFilterStrategy -and $writeCopilotCli) {
        $copilotCliXrayEntry = [ordered]@{
            command = $xrayPath
            args    = $xrayArgs
            env     = [ordered]@{}
        }

        if ($copilotCliAction -eq 'merge' -and $existingCopilotCli) {
            Warn-McpMergeLossyFields -Path $copilotCliMcpPath -Client 'Copilot CLI' -ExistingXrayEntry $existingCopilotCli.mcpServers.xray -ScriptManagedFields @('command','args','env')
            $existingCopilotCli.mcpServers | Add-Member -NotePropertyName 'xray' -NotePropertyValue $copilotCliXrayEntry -Force
            $existingCopilotCli | ConvertTo-Json -Depth 10 | Set-Content -Path $copilotCliMcpPath -Encoding UTF8
            Write-Host "Updated xray entry in $copilotCliMcpPath (other servers preserved)" -ForegroundColor Green
        }
        else {
            $copilotCliConfig = [ordered]@{
                mcpServers = [ordered]@{
                    xray = $copilotCliXrayEntry
                }
            }
            $copilotCliConfig | ConvertTo-Json -Depth 5 | Set-Content -Path $copilotCliMcpPath -Encoding UTF8
            Write-Host "Created $copilotCliMcpPath" -ForegroundColor Green
        }
    }
}

if ($isGitRepo) {
    Push-Location $RepoPath
    try {
        $skipWorktreeFiles = @()
        $excludedFiles = @()

        # `git rev-parse` exits non-zero when .git exists but is not a real
        # repo (e.g. submodule placeholder or partially initialized dir).
        # $ErrorActionPreference='Stop' would otherwise abort the whole script
        # AFTER mcp.json files are already written. Catch and degrade to a
        # warning instead.
        try {
            $excludePath = & git rev-parse --git-path info/exclude 2>$null
            if ($LASTEXITCODE -ne 0) {
                throw "git rev-parse exited with $LASTEXITCODE"
            }
        }
        catch {
            Write-Warning ("Skipping git protection: {0}" -f $_.Exception.Message)
            $excludePath = $null
        }

        if ($excludePath -and -not (Test-Path (Split-Path $excludePath))) {
            $excludePath = $null
        }

        $existingExcludes = @()
        if ($excludePath -and (Test-Path $excludePath)) {
            $existingExcludes = Get-Content $excludePath -ErrorAction SilentlyContinue
        }

        foreach ($mcpFile in @(
                @{ Path = '.vscode/mcp.json'; Protect = ($writeVscode -and (Test-Path (Join-Path $RepoPath '.vscode/mcp.json'))) },
                @{ Path = '.roo/mcp.json'; Protect = ($writeRoo -and (Test-Path (Join-Path $RepoPath '.roo/mcp.json'))) },
                @{ Path = '.mcp.json'; Protect = ($writeCopilotCli -and -not $copilotCliFilterInstalled -and (Test-Path (Join-Path $RepoPath '.mcp.json'))) }
            )) {
            if (-not $mcpFile.Protect) {
                continue
            }

            $rel = $mcpFile.Path
            try {
                $tracked = & git ls-files $rel 2>$null
                if ($LASTEXITCODE -ne 0) { $tracked = $null }
            }
            catch {
                $tracked = $null
            }

            if ($tracked) {
                & git update-index --skip-worktree $rel 2>$null | Out-Null
                if ($LASTEXITCODE -eq 0) {
                    $skipWorktreeFiles += $rel
                }
                else {
                    Write-Warning ("git update-index --skip-worktree {0} failed (exit {1})" -f $rel, $LASTEXITCODE)
                }
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
if ($writeVscode) {
    Write-Host "VS Code (Copilot) config: $vscodeMcpPath"
}
if ($writeRoo) {
    Write-Host "Roo config:         $rooMcpPath"
}
if ($writeCopilotCli) {
    Write-Host "Copilot CLI config: $copilotCliMcpPath"
}
if (-not ($writeVscode -or $writeRoo -or $writeCopilotCli)) {
    Write-Warning 'No MCP client was configured. Re-run with -EnableVSCode / -EnableRoo / -EnableCopilotCli (or answer "y" interactively).'
}
else {
    Write-Host "`nReopen the repo / restart the MCP host to activate xray." -ForegroundColor Yellow
}

exit 0
