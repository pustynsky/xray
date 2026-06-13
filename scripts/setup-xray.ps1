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
    Path to the LOCAL project repository you want to use xray with (the folder
    xray will index and serve over MCP) - e.g. C:\Repos\MyProject. This is NOT
    where xray.exe is installed (see -InstallDir). If not specified, the script
    prompts for it interactively.

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

.PARAMETER GitVisibility
    Controls how the xray entry written into mcp.json relates to git. One of:
      - Visible: write the xray entry directly into mcp.json (preserving existing
        formatting and other servers) with a "//" warning field. NO smudge/clean
        filter, NO skip-worktree, NO .git/info/exclude. If the file is git-tracked
        the change appears in 'git status' and YOU decide whether to commit/push it.
      - Hidden: the previous behavior. Installs a smudge/clean filter on tracked
        files (or .git/info/exclude / skip-worktree fallback) so the xray entry
        never shows in 'git status' and cannot leak upstream.
    When omitted: interactive runs prompt (default Visible); -Force defaults to
    Hidden to preserve existing automation behavior; outside a git repo the mode
    is Visible (there is nothing to hide).

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
    [ValidateSet('Visible', 'Hidden')]
    [string]$GitVisibility,
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

# Human-facing warning embedded as a "//" field inside the xray entry in
# VISIBLE git mode. Unlike the _xrayMcpMarker sentinel (which drives the
# smudge/clean round-trip in hidden mode), this is meant to be SEEN by anyone
# reviewing a git diff of mcp.json.
$Script:XrayVisibleWarning = 'xray MCP entry added by setup-xray.ps1 (embeds a per-machine local path). If this file is git-tracked, MAKE SURE you want to commit/push this change.'

# -----------------------------------------------------------------------------
# Embedded MCP filter scripts (smudge / clean).
# -----------------------------------------------------------------------------
#
# These are byte-identical copies of scripts/mcp-filter/{smudge,clean}.sh
# from the xray repo. They are embedded so this script is fully
# self-contained: when invoked via the bootstrap one-liner
# (`iex (irm .../setup-xray.ps1)`) or downloaded standalone (Option A2),
# there is no `mcp-filter/` directory next to the script on disk, but
# Install-McpFilter still needs the filter scripts to wire git
# smudge/clean.
#
# Source-of-truth: scripts/mcp-filter/{smudge,clean}.sh in the repo.
# These embedded copies are kept in sync by:
#   * scripts/mcp-filter/test-embedded-sync.ps1 (CI-style byte-equality
#     check; AST-extracts these constants and diff-vs-disk).
#   * Install-McpFilter prefers the on-disk source when present, so
#     local edits to scripts/mcp-filter/*.sh take effect immediately
#     without re-embedding here. The embedded copy is a fallback only.
#
# DO NOT EDIT BY HAND. Run `scripts/mcp-filter/test-embedded-sync.ps1`
# to verify in-sync; if it fails, copy the canonical .sh body into the
# corresponding here-string below verbatim.
#
# Encoding: single-quoted here-strings (`@'...'@`) take content literally
# (no $ interpolation, no backtick escapes, single-quote chars allowed).
# Both files have been verified to contain neither CRLF nor the
# terminator sequence `'@` at column 1.

$Script:EmbeddedSmudgeSh = @'
#!/usr/bin/env bash
# smudge filter for xray-managed MCP config files (.mcp.json,
# .vscode/mcp.json, ...). Reads the canonical (upstream-only) form from
# stdin, writes the enriched form (with the local xray entry injected) to
# stdout.
#
# CONTAINER KEY (first positional argument):
#   The JSON object key whose opening brace marks the injection point.
#   - .mcp.json (Copilot CLI):     "mcpServers"  (default)
#   - .vscode/mcp.json (VS Code): "servers"
#   Defaults to "mcpServers" when no argument is passed, preserving
#   backward compatibility with installs from the previous version of
#   setup-xray.ps1 that wired the filter as `bash <path>` with no args.
#
# Strategy: insert the snapshot line as the FIRST entry inside the
# container object, immediately after its opening brace. This avoids
# matching the closing brace, which would require brace-counting that is
# not safe against `{` and `}` characters appearing inside JSON string
# values (common in args).
#
# Behavior:
#   * snapshot.txt missing             -> passthrough (filter no-op)
#   * input already contains marker    -> passthrough (defensive)
#   * mcpServers opens at end of line  -> inject after that line
#       - empty mcpServers (next non-blank line is `}` or `},`) -> inject without trailing comma
#       - non-empty mcpServers                                  -> inject with trailing comma
#   * any other shape (inline `{}`, single-line full mcpServers, missing key) -> passthrough
#
# Round-trip property (verified by test-roundtrip.ps1):
#   clean(smudge(canonical)) == canonical for every fixture in fixtures/.
#
# Implementation note: uses perl (bundled with Git for Windows) instead of
# awk because awk on Git for Windows normalizes CRLF -> LF even with
# BINMODE=3, breaking byte-exact round-trip on Windows clones with
# CRLF-stored .mcp.json. perl with `:raw` binmode preserves bytes verbatim
# on all platforms.
#
# `filter.required = false` is set so that any failure here (including
# missing bash on PATH) degrades to passthrough rather than aborting git.
#
# Installed as: .git/xray-mcp/smudge.sh
# Wired via:    [filter "xray-mcp"] smudge = .git/xray-mcp/smudge.sh
#
# DO NOT EDIT BY HAND. setup-xray.ps1 manages this file.

set -eu

# First positional argument: container key (defaults to "mcpServers" for
# backward compat with pre-vscode-extension installs that wired the filter
# command as `bash <path>` with no args). Validate it's a non-empty
# alphanumeric token so the value can be safely interpolated into the perl
# regex below without escaping.
container_key="${1:-mcpServers}"
if ! printf '%s' "$container_key" | grep -Eq '^[A-Za-z_][A-Za-z0-9_]*$'; then
    # Invalid container key -> degrade to passthrough rather than risk a
    # regex injection or a perl die that would block git checkout.
    exec cat
fi

snapshot_path="$(dirname "$0")/snapshot.txt"

# Snapshot missing -> passthrough.
if [ ! -r "$snapshot_path" ]; then
    exec cat
fi

exec perl -e '
    use strict;
    use warnings;
    binmode STDIN,  ":raw";
    binmode STDOUT, ":raw";

    # Read snapshot (single line; strip any trailing CR/LF).
    my $snapshot_path = $ARGV[0];
    my $container_key = $ARGV[1];
    open my $sf, "<:raw", $snapshot_path or do { print while <STDIN>; exit 0 };
    my $snap = do { local $/; <$sf> };
    close $sf;
    $snap =~ s/[\r\n]+\z//;

    # Slurp entire input.
    my $all = do { local $/; <STDIN> };
    $all = "" unless defined $all;

    # Defensive: if the marker is already present, passthrough byte-exact.
    if ($all =~ /_xrayMcpMarker/) {
        print $all;
        exit 0;
    }

    # Detect dominant line separator (\r\n vs \n).
    # If we see at least one \r\n in the input, use it; else \n; else \n.
    my $sep = ($all =~ /\r\n/) ? "\r\n" : "\n";

    # Split into lines preserving separators. Use split with limit=-1 so
    # trailing empty strings are kept; structure is text,sep,text,sep,...
    my @parts = split /(\r?\n)/, $all, -1;

    # Build the container-opening regex: "<key>"\s*:\s*\{\s*$
    # The key is validated by the bash wrapper to be /^[A-Za-z_][A-Za-z0-9_]*$/
    # so it is safe to splice into the regex without escaping.
    my $open_re = qr/"\Q$container_key\E"\s*:\s*\{\s*$/;

    my @out;
    my $injected = 0;
    my $i = 0;
    my $n = scalar @parts;
    while ($i < $n) {
        my $text = $parts[$i];
        my $line_sep = ($i + 1 < $n) ? $parts[$i + 1] : "";
        push @out, $text;
        push @out, $line_sep if $line_sep ne "";

        if (!$injected && $text =~ $open_re) {
            # Peek next text segment (skip blank lines).
            my $j = $i + 2;  # next text index after sep
            while ($j < $n && $parts[$j] =~ /^\s*$/ && (($j + 1 < $n) ? $parts[$j + 1] ne "" : 0)) {
                # Push the blank line through.
                push @out, $parts[$j];
                push @out, $parts[$j + 1];
                $j += 2;
            }
            my $next = ($j < $n) ? $parts[$j] : "";
            my $trimmed = $next;
            $trimmed =~ s/^\s+//;
            $trimmed =~ s/\s+$//;
            my $is_empty_servers = ($trimmed eq "}" || $trimmed eq "}," || $trimmed eq "} ,");

            my $line_to_emit = $is_empty_servers ? $snap : $snap . ",";
            push @out, $line_to_emit;
            push @out, $line_sep;  # match same line separator as opening line
            $injected = 1;

            # Continue from $j (we already pushed blank lines).
            $i = $j;
            next;
        }

        $i += 2;  # advance past text + sep
    }

    print join("", @out);
' "$snapshot_path" "$container_key"
'@

$Script:EmbeddedCleanSh = @'
#!/usr/bin/env bash
# clean filter for .mcp.json (xray MCP per-clone setup).
# Reads enriched .mcp.json from stdin, writes canonical (upstream-only) form
# to stdout by deleting any line carrying the _xrayMcpMarker sentinel.
#
# Idempotent: an input with no marker passes through byte-identical.
# Designed to be the exact inverse of smudge.sh on smudge-produced content.
#
# Implementation note: uses perl (bundled with Git for Windows) instead of
# sed because sed on Git for Windows normalizes CRLF -> LF, breaking
# byte-exact round-trip on Windows clones with CRLF-stored .mcp.json.
# perl with `:raw` binmode preserves bytes verbatim on all platforms.
#
# Installed as: .git/xray-mcp/clean.sh
# Wired via:    [filter "xray-mcp"] clean = .git/xray-mcp/clean.sh
#
# DO NOT EDIT BY HAND. setup-xray.ps1 manages this file.

set -eu
exec perl -e '
    binmode STDIN, ":raw";
    binmode STDOUT, ":raw";
    while (<STDIN>) {
        print unless /_xrayMcpMarker/;
    }
'
'@

function New-XraySnapshotLine {
    <#
        Builds the single-line JSON entry that the smudge filter injects
        into a tracked MCP config file. Indented with 4 spaces to match
        common formatting (server entries nested two levels deep inside
        the container object).

        SHAPE controls which schema the entry follows:
          * 'CopilotCli' (default): {"command":...,"args":[...],"env":{},"_xrayMcpMarker":...}
            -> matches the .mcp.json schema (mcpServers container, no
               'type' key, env always present).
          * 'VsCode':              {"type":"stdio","command":...,"args":[...],"_xrayMcpMarker":...}
            -> matches the .vscode/mcp.json schema (servers container,
               'type' required, no 'env' by convention).
    #>
    param(
        [Parameter(Mandatory)] [string]$XrayPath,
        [Parameter(Mandatory)] [string[]]$XrayArgs,
        [ValidateSet('CopilotCli', 'VsCode')]
        [string]$Shape = 'CopilotCli'
    )

    $argsJson = (@($XrayArgs) | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress }) -join ','
    $cmdJson  = ConvertTo-Json -InputObject $XrayPath -Compress
    $markerJson = ConvertTo-Json -InputObject $Script:XrayMcpMarker -Compress
    if ($Shape -eq 'VsCode') {
        return ('    "xray":{"type":"stdio","command":' + $cmdJson + ',"args":[' + $argsJson + '],"_xrayMcpMarker":' + $markerJson + '}')
    }
    return ('    "xray":{"command":' + $cmdJson + ',"args":[' + $argsJson + '],"env":{},"_xrayMcpMarker":' + $markerJson + '}')
}

function New-XrayVisibleEntryLine {
    <#
        Builds the single-line JSON xray entry for VISIBLE git mode. Same shape
        as New-XraySnapshotLine, but:
          - carries a human-facing "//" warning field FIRST (so it is the most
            prominent token in a git diff) instead of the _xrayMcpMarker sentinel;
          - is a normal entry the user is expected to see in 'git status'.
        Indented with 4 spaces to match server entries nested two levels deep.
    #>
    param(
        [Parameter(Mandatory)] [string]$XrayPath,
        [Parameter(Mandatory)] [string[]]$XrayArgs,
        [ValidateSet('CopilotCli', 'VsCode')]
        [string]$Shape = 'CopilotCli'
    )

    $argsJson = (@($XrayArgs) | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress }) -join ','
    $cmdJson  = ConvertTo-Json -InputObject $XrayPath -Compress
    $warnJson = ConvertTo-Json -InputObject $Script:XrayVisibleWarning -Compress
    if ($Shape -eq 'VsCode') {
        return ('    "xray": { "//": ' + $warnJson + ', "type": "stdio", "command": ' + $cmdJson + ', "args": [' + $argsJson + '] }')
    }
    return ('    "xray": { "//": ' + $warnJson + ', "command": ' + $cmdJson + ', "args": [' + $argsJson + '], "env": {} }')
}

function Set-McpFileVisibleViaJson {
    <#
        Fallback writer for VISIBLE git mode. Used only when the formatting-
        preserving line-injection path in Set-McpFileVisibleEntry cannot apply
        cleanly (container key absent, inline `{}` container, or a pre-existing
        MULTI-LINE xray block from a legacy plain-merge install). Parses the
        file, sets/replaces the xray entry (with the "//" warning field first),
        and reserializes. This reformats the whole file, so it is intentionally
        the slow path; the common first-install case keeps upstream formatting.
        Returns $true on success.
    #>
    param(
        [Parameter(Mandatory)] [string]$Path,
        [ValidateSet('mcpServers', 'servers')] [string]$ContainerKey,
        [Parameter(Mandatory)] [string]$XrayPath,
        [Parameter(Mandatory)] [string[]]$XrayArgs,
        [ValidateSet('CopilotCli', 'VsCode')] [string]$Shape,
        [string]$Sep = "`n"
    )

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    $obj = $null
    if (Test-Path $Path) {
        try { $obj = (Get-Content -Path $Path -Raw) | ConvertFrom-Json -ErrorAction Stop } catch { $obj = $null }
    }
    if (-not $obj) { $obj = [pscustomobject]@{} }

    if (-not ($obj.PSObject.Properties.Name -contains $ContainerKey)) {
        $obj | Add-Member -NotePropertyName $ContainerKey -NotePropertyValue ([pscustomobject]@{}) -Force
    }

    if ($Shape -eq 'VsCode') {
        $entry = [ordered]@{ '//' = $Script:XrayVisibleWarning; type = 'stdio'; command = $XrayPath; args = $XrayArgs }
    }
    else {
        $entry = [ordered]@{ '//' = $Script:XrayVisibleWarning; command = $XrayPath; args = $XrayArgs; env = [ordered]@{} }
    }
    $obj.$ContainerKey | Add-Member -NotePropertyName 'xray' -NotePropertyValue ([pscustomobject]$entry) -Force

    $json = $obj | ConvertTo-Json -Depth 10
    $json = $json -replace "`r`n", "`n"
    if ($Sep -eq "`r`n") { $json = $json -replace "`n", "`r`n" }
    [IO.File]::WriteAllText($Path, $json + $Sep, $utf8NoBom)
    return $true
}

function Set-McpFileVisibleEntry {
    <#
        Writes the xray entry into an MCP config file in VISIBLE git mode.
        Preserves existing formatting and other servers by injecting the
        single-line $EntryLine as the FIRST entry of the container object
        (same line-surgery approach as Set-McpFileWithSnapshot, minus the
        smudge/clean marker). Idempotent across re-runs and mode switches:
          - strips any prior hidden snapshot line (_xrayMcpMarker), so switching
            from hidden -> visible re-injects a clean visible entry;
          - replaces a prior single-line visible xray entry in place;
          - for any other pre-existing xray shape (multi-line block) or unusual
            container shape, falls back to Set-McpFileVisibleViaJson.
        Output preserves the file's dominant line separator (CRLF vs LF).
        Returns $true on success, $false if the file exists but is not parseable
        as JSON (in which case it is left untouched).
    #>
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$EntryLine,
        [ValidateSet('mcpServers', 'servers')] [string]$ContainerKey = 'mcpServers',
        [Parameter(Mandatory)] [string]$XrayPath,
        [Parameter(Mandatory)] [string[]]$XrayArgs,
        [ValidateSet('CopilotCli', 'VsCode')] [string]$Shape = 'CopilotCli'
    )

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    if (-not (Test-Path $Path)) {
        $body = "{`n  ""$ContainerKey"": {`n$EntryLine`n  }`n}`n"
        [IO.File]::WriteAllText($Path, $body, $utf8NoBom)
        return $true
    }

    $raw = [IO.File]::ReadAllText($Path, $utf8NoBom)
    $sep = if ($raw -match "`r`n") { "`r`n" } else { "`n" }

    # Validate JSON; never corrupt an unparseable file.
    try { $null = $raw | ConvertFrom-Json -ErrorAction Stop }
    catch {
        Write-Warning ("Cannot parse {0} as JSON; leaving it untouched. Add the xray entry manually." -f $Path)
        return $false
    }

    $normalized = $raw -replace "`r`n", "`n"

    # 1. Strip any prior hidden snapshot line(s). After this, a former hidden
    #    install has no xray line in the text, so step 3 injects a clean one.
    if ($normalized -match '_xrayMcpMarker') {
        $kept = @(($normalized -split "`n") | Where-Object { $_ -notmatch '_xrayMcpMarker' })
        $normalized = ($kept -join "`n")
    }

    $lines = $normalized -split "`n"

    # Locate the container-open line and its first non-blank member.
    $openPattern = '"' + $ContainerKey + '"\s*:\s*\{\s*$'
    $openIdx = -1
    for ($i = 0; $i -lt $lines.Length; $i++) {
        if ($lines[$i] -match $openPattern) { $openIdx = $i; break }
    }
    $firstMemberIdx = -1
    if ($openIdx -ge 0) {
        $k = $openIdx + 1
        while ($k -lt $lines.Length -and $lines[$k] -match '^\s*$') { $k++ }
        if ($k -lt $lines.Length) { $firstMemberIdx = $k }
    }

    # 2. Remove a prior single-line visible xray entry in place ONLY when it is
    #    the FIRST container member. Removing a non-first single-line entry would
    #    orphan the preceding entry's trailing comma and yield strict-JSON-invalid
    #    output (ConvertFrom-Json tolerates a trailing comma, but Copilot CLI's
    #    strict parser rejects it). A multi-line/legacy xray block, OR a
    #    single-line xray that is not first, defers to the JSON reserialize
    #    fallback which removes/repositions xray safely.
    $singleLineXrayPattern = '^\s*"xray"\s*:\s*\{.*\}\s*,?\s*$'
    $multiLineXrayOpen = '^\s*"xray"\s*:\s*\{\s*$'
    $singleIdx = -1
    $multiOpenIdx = -1
    for ($i = 0; $i -lt $lines.Length; $i++) {
        if ($singleIdx -lt 0 -and $lines[$i] -match $singleLineXrayPattern) { $singleIdx = $i }
        if ($multiOpenIdx -lt 0 -and $lines[$i] -match $multiLineXrayOpen) { $multiOpenIdx = $i }
    }

    if ($singleIdx -ge 0) {
        if ($singleIdx -ne $firstMemberIdx) {
            # Not the first member: line-removal would leave a dangling comma.
            # Reserialize via JSON, which removes/repositions xray safely.
            return (Set-McpFileVisibleViaJson -Path $Path -ContainerKey $ContainerKey -XrayPath $XrayPath -XrayArgs $XrayArgs -Shape $Shape -Sep $sep)
        }
        # Safe: removing the first member never orphans a preceding entry's comma.
        # The fresh entry is re-injected at the top in step 3.
        $before = if ($singleIdx -gt 0) { $lines[0..($singleIdx - 1)] } else { @() }
        $after = if ($singleIdx -lt $lines.Length - 1) { $lines[($singleIdx + 1)..($lines.Length - 1)] } else { @() }
        $lines = @($before) + @($after)
    }
    elseif ($multiOpenIdx -ge 0) {
        return (Set-McpFileVisibleViaJson -Path $Path -ContainerKey $ContainerKey -XrayPath $XrayPath -XrayArgs $XrayArgs -Shape $Shape -Sep $sep)
    }

    # 3. Inject $EntryLine as the first entry after the container opening brace.
    $newLines = New-Object System.Collections.Generic.List[string]
    $injected = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $newLines.Add($lines[$i])
        if (-not $injected -and $lines[$i] -match $openPattern) {
            $j = $i + 1
            while ($j -lt $lines.Length -and $lines[$j] -match '^\s*$') { $j++ }
            $emptyServers = ($j -lt $lines.Length -and $lines[$j] -match '^\s*\}\s*,?\s*$')
            if ($emptyServers) { $newLines.Add($EntryLine) }
            else { $newLines.Add($EntryLine + ',') }
            $injected = $true
        }
    }

    if (-not $injected) {
        # Container is inline ({}), absent, or not at end of line -> JSON fallback.
        return (Set-McpFileVisibleViaJson -Path $Path -ContainerKey $ContainerKey -XrayPath $XrayPath -XrayArgs $XrayArgs -Shape $Shape -Sep $sep)
    }

    [IO.File]::WriteAllText($Path, ($newLines -join $sep), $utf8NoBom)
    return $true
}

function Get-GitWorkTreeRoot {
    param([Parameter(Mandatory)] [string]$RepoRoot)

    $ErrorActionPreference = 'Continue'

    try {
        $top = & git -C $RepoRoot rev-parse --show-toplevel 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $top) { return $null }
        return (Resolve-Path ($top | Select-Object -First 1)).Path
    }
    catch {
        return $null
    }
}


function Get-ResolvedGitDir {
    param([Parameter(Mandatory)] [string]$RepoRoot)

    # Local EAP override: see Test-IsTrackedFile for the full rationale.
    # `git rev-parse --git-dir` writes "fatal: not a git repository" to
    # stderr when called outside a repo; on Windows PowerShell 5.1 with
    # $ErrorActionPreference='Stop' inherited from script scope, that
    # stderr line is converted to a terminating ErrorRecord and aborts the
    # entire script. Function scope confines the override.
    $ErrorActionPreference = 'Continue'

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

function Get-ResolvedGitCommonDir {
    <#
        Returns the absolute path to the SHARED git common dir for the repo
        at $RepoRoot. In a primary clone this equals --git-dir; in a linked
        worktree (created via `git worktree add`), --git-dir points at
        `<main>/.git/worktrees/<name>/` while --git-common-dir points at
        `<main>/.git/`.

        Use this for any per-clone artifact that must be SHARED across all
        worktrees:
          - the smudge/clean filter scripts (the filter command is stored
            as `bash "$(git rev-parse --git-common-dir)/<name>/smudge.sh" ...`
            and git evaluates that substitution at filter-invocation time
            using --git-common-dir, so the scripts MUST physically live
            under the common dir).
          - `info/attributes` (per gitrepository-layout(5), the `info/`
            directory is shared across worktrees; git's own
            `git rev-parse --git-path info/attributes` resolves to the
            common dir, and our manual `Join-Path $resolvedGitCommonDir
            'info\attributes'` matches that).

        For path lookups under `.git/info/` from elsewhere in the script
        (e.g., `Remove-GitProtection` for `info/exclude`), prefer
        `git rev-parse --git-path info/<name>` and let git pick the
        correct directory rather than hand-building a path here. That
        delegation is why `Get-ResolvedGitDir` is still used for
        per-worktree HEAD/index lookups (and why `info/exclude` cleanup
        does NOT call this function — it goes through git rev-parse).
    #>
    param([Parameter(Mandatory)] [string]$RepoRoot)

    # Local EAP override: see Test-IsTrackedFile for the full rationale.
    $ErrorActionPreference = 'Continue'

    Push-Location $RepoRoot
    try {
        $gcd = & git rev-parse --git-common-dir 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $gcd) { return $null }
        if (-not [IO.Path]::IsPathRooted($gcd)) { $gcd = Join-Path $RepoRoot $gcd }
        return (Resolve-Path $gcd).Path
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

    # `git ls-files --error-unmatch` is the standard probe for "is this path
    # tracked?". It exits 0 for tracked, 1 (with stderr "error: pathspec ...
    # did not match") for untracked. The stderr message is the *expected*
    # answer, not a real error.
    #
    # On Windows PowerShell 5.1 — the runtime the bootstrap one-liner
    # invokes by default — native-command stderr is converted to a
    # NativeCommandError ErrorRecord. With $ErrorActionPreference='Stop'
    # inherited from script scope (set near the top of this script for
    # safety), that ErrorRecord becomes a TERMINATING error and aborts
    # the entire setup script BEFORE the script can branch on $LASTEXITCODE.
    # `2>$null`, `2>&1 | Out-Null`, and `$null = ... 2>&1` do NOT change
    # this behavior; the abort happens before the redirect can swallow
    # the stream. PowerShell 7's $PSNativeCommandUseErrorActionPreference
    # toggle does not exist in 5.1, so the fix has to be local-scope EAP
    # relaxation. Function scope confines the override; nothing else needs
    # the relaxed setting.
    #
    # See target/tmp/leak-probe2.ps1 for the cross-runtime test that
    # established this. (Bug surfaced in production: setup-xray.ps1 line 551
    # leaked `git.exe : error: pathspec '.vscode/mcp.json' did not match...`
    # to the user's console and aborted setup before the VS Code MCP file
    # could be created.)
    $ErrorActionPreference = 'Continue'

    Push-Location $RepoRoot
    try {
        $null = & git ls-files --error-unmatch -- $RelativePath 2>$null
        return $LASTEXITCODE -eq 0
    }
    finally {
        Pop-Location
    }
}

function Add-TextLineNoBom {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Line
    )

    $resolvedPath = if ([IO.Path]::IsPathRooted($Path)) { $Path } else { Join-Path (Get-Location).Path $Path }
    $utf8NoBom = [Text.UTF8Encoding]::new($false)
    $parent = Split-Path -Parent $resolvedPath
    if ($parent -and -not (Test-Path $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }

    if (-not (Test-Path $resolvedPath)) {
        [IO.File]::WriteAllText($resolvedPath, ($Line + "`n"), $utf8NoBom)
        return $true
    }

    $raw = [IO.File]::ReadAllText($resolvedPath)
    $lines = @($raw -split "`r?`n") | Where-Object { $_ -ne '' }
    if ($lines -contains $Line) {
        return $false
    }

    $sep = if ($raw -match "`r`n") { "`r`n" } else { "`n" }
    $prefix = if ($raw.Length -gt 0 -and -not $raw.EndsWith("`n")) { $sep } else { '' }
    [IO.File]::AppendAllText($resolvedPath, ($prefix + $Line + $sep), $utf8NoBom)
    return $true
}

function Remove-TextLineNoBom {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Line
    )

    $resolvedPath = if ([IO.Path]::IsPathRooted($Path)) { $Path } else { Join-Path (Get-Location).Path $Path }
    if (-not (Test-Path $resolvedPath)) {
        return $false
    }

    $utf8NoBom = [Text.UTF8Encoding]::new($false)
    $raw = [IO.File]::ReadAllText($resolvedPath)
    $sep = if ($raw -match "`r`n") { "`r`n" } else { "`n" }
    $parts = @($raw -split "`r?`n")
    $kept = @($parts | Where-Object { $_ -ne $Line })
    if ($kept.Count -eq $parts.Count) {
        return $false
    }

    while ($kept.Count -gt 0 -and $kept[-1] -eq '') {
        if ($kept.Count -eq 1) {
            $kept = @()
        }
        else {
            $kept = @($kept[0..($kept.Count - 2)])
        }
    }

    $body = if ($kept.Count -gt 0) { ($kept -join $sep) + $sep } else { '' }
    [IO.File]::WriteAllText($resolvedPath, $body, $utf8NoBom)
    return $true
}


function Test-GitIgnoresPath {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$RelativePath
    )

    $ErrorActionPreference = 'Continue'

    Push-Location $RepoRoot
    try {
        & git check-ignore -q -- $RelativePath 2>$null
        return $LASTEXITCODE -eq 0
    }
    finally {
        Pop-Location
    }
}

function Add-DirectoryGitignoreOverride {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$RelativePath,
        [string]$ExcludePath
    )

    $dirRel = (Split-Path -Parent $RelativePath) -replace '\\', '/'
    if ([string]::IsNullOrWhiteSpace($dirRel)) {
        return $false
    }

    $leaf = Split-Path -Leaf $RelativePath
    $localGitignoreRel = "$dirRel/.gitignore"
    if (Test-IsTrackedFile -RepoRoot $RepoRoot -RelativePath $localGitignoreRel) {
        Write-Warning "Cannot create local ignore override for $RelativePath because $localGitignoreRel is tracked. The repository .gitignore may keep this file visible."
        return $false
    }

    $localGitignorePath = Join-Path $RepoRoot $localGitignoreRel
    $helperExistedBefore = Test-Path $localGitignorePath
    $addedHelperLine = Add-TextLineNoBom -Path $localGitignorePath -Line $leaf

    $addedExcludeLine = $false
    if ($ExcludePath) {
        $addedExcludeLine = Add-TextLineNoBom -Path $ExcludePath -Line $localGitignoreRel
    }

    if (-not (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $localGitignoreRel)) {
        Write-Warning "Local ignore helper $localGitignoreRel may be visible in git status."
        if (-not $helperExistedBefore) {
            Remove-Item -Path $localGitignorePath -Force -ErrorAction SilentlyContinue
        }
        elseif ($addedHelperLine) {
            [void](Remove-TextLineNoBom -Path $localGitignorePath -Line $leaf)
        }
        if ($ExcludePath -and $addedExcludeLine) {
            [void](Remove-TextLineNoBom -Path $ExcludePath -Line $localGitignoreRel)
        }
        return $false
    }

    return (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $RelativePath)
}


function Set-McpFileWithSnapshot {
    <#
        Writes an MCP config file so that it contains the snapshot line as
        the FIRST entry of the container object, preserving any existing
        entries and any other top-level keys verbatim.

        ContainerKey selects the container object key:
          * 'mcpServers' (default): .mcp.json schema (Copilot CLI).
          * 'servers':              .vscode/mcp.json schema (VS Code).

        Behavior:
          - File missing: create a fresh minimal file containing only the
            xray entry, wrapped in the requested container key.
          - Marker already present: replace existing snapshot line in place.
          - Container present (multi-line, opens at end of line): inject
            snapshot line after the line that opens the object.
          - Container inline ({} on one line) or missing: warn and bail.
            The caller must fail closed for tracked filter installs; otherwise
            setup would report success without registering xray.

        Output preserves the file's existing dominant line separator (CRLF
        if any CRLF is present in the input, otherwise LF). This matches
        the smudge filter's separator-detection behavior, so the round-trip
        property `clean(smudge(canonical)) == canonical` is preserved
        byte-exact even on repos whose tracked file uses CRLF.
        New files (no existing input) default to LF.
    #>
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$SnapshotLine,
        [ValidateSet('mcpServers', 'servers')]
        [string]$ContainerKey = 'mcpServers'
    )

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    if (-not (Test-Path $Path)) {
        $body = "{`n  ""$ContainerKey"": {`n$SnapshotLine`n  }`n}`n"
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


    # Build the open-pattern. ContainerKey is constrained by ValidateSet to
    # 'mcpServers' or 'servers' (both safe regex literals); no escaping
    # required.
    $openPattern = '"' + $ContainerKey + '"\s*:\s*\{\s*$'

    $lines = $normalized -split "`n"
    $newLines = New-Object System.Collections.Generic.List[string]
    $injected = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $newLines.Add($lines[$i])
        if (-not $injected -and $lines[$i] -match $openPattern) {
            # Determine whether the container is empty by peeking ahead.
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
        Write-Warning ("Could not find an injection point for xray in {0} ({1} may be inline `{{}}` or absent). Filter will passthrough; xray entry NOT registered." -f $Path, $ContainerKey)
        return $false
    }

    [IO.File]::WriteAllText($Path, ($newLines -join $sep), $utf8NoBom)
    return $true
}

function Install-McpFilter {
    <#
        Installs the smudge/clean filter for an MCP config file in the
        given repo. Supports multiple parallel filter setups distinguished
        by FilterName (e.g. one for .mcp.json, one for .vscode/mcp.json).
        Effects:
          - Creates <git-common-dir>/<FilterName>/ with smudge.sh, clean.sh,
            snapshot.txt (LF-normalized; bash heredocs / set -e are
            sensitive to CRLF in script bodies).
          - Sets .git/config [filter "<FilterName>"] smudge/clean/required.
            The smudge command is stored as
            `bash "$(git rev-parse --git-common-dir)/<FilterName>/smudge.sh" <ContainerKey>`
            (NOT bare paths and NOT --git-dir): explicit `bash` avoids the
            need for the executable bit on macOS/Linux, --git-common-dir
            resolves correctly inside linked worktrees where the
            per-worktree --git-dir does NOT contain the scripts, and the
            ContainerKey arg lets a single smudge.sh source serve both
            the 'mcpServers' (Copilot CLI) and 'servers' (VS Code) shapes.
          - Adds '<AttributePath> filter=<FilterName>' to
            <git-common-dir>/info/attributes (idempotent; replaces any
            prior line for that attribute path). NO `text eol=lf`
            attribute: the perl-based filter preserves CRLF byte-exact.
        Returns $true on success, $false on failure (with a warning). Each
        git config write checks $LASTEXITCODE; a failure short-circuits and
        triggers the caller's fail-closed rollback path.

        Source scripts are copied from $ScriptDir/mcp-filter/{smudge,clean}.sh
        so both production install and dev iteration use the same files.
        BOTH filter setups (xray-mcp and xray-vscode-mcp) get their OWN copy
        of the same source scripts; refreshing one filter does not affect
        the other.
    #>
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        # Allow empty/null because the in-memory invocation form
        # (`& ([scriptblock]::Create((iwr ...).Content))`) leaves the
        # caller without a script path on disk. Install-McpFilter detects
        # this below and short-circuits to the embedded fallback.
        [Parameter()] [AllowEmptyString()] [AllowNull()] [string]$ScriptDir,
        [Parameter(Mandatory)] [string]$SnapshotLine,
        [Parameter(Mandatory)]
        [ValidateSet('xray-mcp', 'xray-vscode-mcp')]
        [string]$FilterName,
        [Parameter(Mandatory)]
        [ValidateSet('mcpServers', 'servers')]
        [string]$ContainerKey,
        [Parameter(Mandatory)]
        [string]$AttributePath
    )

    $resolvedGitDir = Get-ResolvedGitDir -RepoRoot $RepoRoot
    if (-not $resolvedGitDir) {
        Write-Warning 'git rev-parse --git-dir failed - cannot install filter.'
        return $false
    }

    # Filter scripts and snapshot.txt are referenced at runtime by
    # `bash "$(git rev-parse --git-common-dir)/<FilterName>/..."`, so they
    # MUST live under the SHARED common dir, not the per-worktree --git-dir.
    # Otherwise: setup-xray.ps1 invoked from inside a linked worktree would
    # write the scripts to `<main>/.git/worktrees/<wt>/<FilterName>/`, but
    # git would look for them under `<main>/.git/<FilterName>/` and fail
    # open (with required=false the result is a silent passthrough that
    # both drops the xray entry on checkout AND, on the next `git add
    # --renormalize`, can stage the marker-bearing working-tree text).
    # `info/attributes` is also a shared-across-worktrees file per
    # gitrepository-layout(5), so it goes here too.
    $resolvedGitCommonDir = Get-ResolvedGitCommonDir -RepoRoot $RepoRoot
    if (-not $resolvedGitCommonDir) {
        Write-Warning 'git rev-parse --git-common-dir failed - cannot install filter.'
        return $false
    }

    # Filter script sources: prefer on-disk (clone case — local edits to
    # scripts/mcp-filter/*.sh take effect immediately), fall back to the
    # embedded constants ($Script:EmbeddedSmudgeSh / $Script:EmbeddedCleanSh)
    # when no `mcp-filter/` directory exists next to this script. The
    # fallback path is hit by:
    #   * the bootstrap one-liner (`iex (irm .../setup-xray.ps1)`) — the
    #     script never lands on disk, so $ScriptDir is irrelevant.
    #   * Option A2 (`iwr ... -OutFile $tmp; & $tmp`) — the script lands
    #     in %TEMP% with no sibling directory.
    # The on-disk-vs-embedded byte-equality is enforced by
    # scripts/mcp-filter/test-embedded-sync.ps1.
    # When invoked in-memory (One-liner B / `iex (irm ...)`) the caller
    # passes $ScriptDir as $null because there is no file on disk;
    # `Join-Path $null ...` would abort with a null-binding error, so
    # short-circuit straight into the embedded branch in that case.
    $srcFilterDir = if ([string]::IsNullOrEmpty($ScriptDir)) { $null } else { Join-Path $ScriptDir 'mcp-filter' }
    $filterSources = @{}
    foreach ($name in @('smudge.sh', 'clean.sh')) {
        $diskPath = if ($srcFilterDir) { Join-Path $srcFilterDir $name } else { $null }
        if ($diskPath -and (Test-Path $diskPath)) {
            $filterSources[$name] = @{ Source = 'disk'; DiskPath = $diskPath }
        }
        else {
            $embedded = if ($name -eq 'smudge.sh') { $Script:EmbeddedSmudgeSh } else { $Script:EmbeddedCleanSh }
            if (-not $embedded) {
                # Defensive: if a future refactor accidentally drops the
                # embedded constants AND the disk source is missing, we
                # cannot proceed. The previous behavior was a hard failure
                # here too. Use a PS 5.1-compatible null check (no `??`).
                $where = if ($diskPath) { $diskPath } else { '(in-memory invocation, no $ScriptDir)' }
                Write-Warning ("Filter source missing: {0} (and no embedded fallback)" -f $where)
                return $false
            }
            $filterSources[$name] = @{ Source = 'embedded'; Body = $embedded }
        }
    }

    $filterDir = Join-Path $resolvedGitCommonDir $FilterName
    if (-not (Test-Path $filterDir)) {
        New-Item -ItemType Directory -Path $filterDir -Force | Out-Null
    }

    $utf8NoBom = [Text.UTF8Encoding]::new($false)

    # Copy filter scripts, normalizing to LF (bash on Windows tolerates CRLF
    # in scripts but it can break heredocs and trailing-newline-sensitive
    # constructs). Also enforce a trailing LF so the embedded-fallback path
    # produces a file byte-identical to the on-disk source path: PowerShell
    # here-strings (`@'...'@`) do NOT include the bounding newline before
    # `'@` in the value, so the embedded constants end without `\n`; on-disk
    # source files conventionally do. Ensuring trailing-LF here makes both
    # branches converge.
    foreach ($name in @('smudge.sh', 'clean.sh')) {
        $info = $filterSources[$name]
        if ($info.Source -eq 'disk') {
            $body = [IO.File]::ReadAllText($info.DiskPath, $utf8NoBom)
        }
        else {
            $body = $info.Body
        }
        $body = $body -replace "`r`n", "`n"
        if (-not $body.EndsWith("`n")) { $body += "`n" }
        $dst = Join-Path $filterDir $name
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
        # The container key is interpolated into the filter command verbatim
        # (single-quoted string above prevents PowerShell expansion). The
        # smudge.sh wrapper validates the key against
        # /^[A-Za-z_][A-Za-z0-9_]*$/ before splicing it into the perl regex,
        # but ValidateSet on the parameter already restricts the value to
        # 'mcpServers' / 'servers', so this is defense-in-depth.
        $smudgeCmd = 'bash "$(git rev-parse --git-common-dir)/' + $FilterName + '/smudge.sh" ' + $ContainerKey
        $cleanCmd  = 'bash "$(git rev-parse --git-common-dir)/' + $FilterName + '/clean.sh"'

        # Windows PowerShell 5.1 native-command argument-passing bug:
        # when an argument value contains literal `"` characters, 5.1's
        # CommandLineToArgvW-style serialization STRIPS the embedded
        # quotes, then git word-splits the result. e.g. on 5.1,
        # `& git config --local key 'bash "$(...)/smudge.sh" arg'` is
        # delivered to git.exe as multiple args (`bash`, `$(...)/smudge.sh`,
        # `arg`) and stored as just `bash` — or, when the broken split
        # produces flag-shaped tokens, git aborts with "error: no action
        # specified". PS 7.4+ defaults `$PSNativeCommandArgumentPassing`
        # to `Standard` which escapes the embedded quotes correctly, so
        # this hazard is 5.1-specific. Workaround: pre-escape every `"`
        # in the value to `\"` ONLY on 5.1; PS 7+ would double-escape if
        # we did the same.
        # Was caught by target/tmp/test-prod-embedded.ps1; the existing
        # test-e2e.ps1 misses it because it always invokes setup-xray.ps1
        # via `pwsh -File ...` (PS 7) regardless of the runtime running
        # the test itself.
        $smudgeArg = $smudgeCmd
        $cleanArg  = $cleanCmd
        if ($PSVersionTable.PSVersion.Major -lt 6) {
            $smudgeArg = $smudgeCmd -replace '"', '\"'
            $cleanArg  = $cleanCmd  -replace '"', '\"'
        }

        & git config --local ("filter.{0}.smudge" -f $FilterName) $smudgeArg 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.{0}.smudge failed (exit {1})" -f $FilterName, $LASTEXITCODE)
            return $false
        }

        & git config --local ("filter.{0}.clean" -f $FilterName) $cleanArg 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.{0}.clean failed (exit {1})" -f $FilterName, $LASTEXITCODE)
            return $false
        }

        & git config --local --bool ("filter.{0}.required" -f $FilterName) false 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning ("git config filter.{0}.required failed (exit {1})" -f $FilterName, $LASTEXITCODE)
            return $false
        }
    }
    finally {
        Pop-Location
    }

    # Add attributes line (idempotent). Each FilterName has its own
    # AttributePath; we strip any prior line matching that AttributePath
    # (including legacy ones for the SAME path that pointed at a different
    # filter, e.g. an old install of the same script that used a different
    # filter name) before appending the canonical line. The match anchor
    # is start-of-line + the literal attribute path + a separator (space
    # or tab), so a line attaching attributes to a DIFFERENT path that
    # happens to begin with this path's basename does NOT get stripped.
    $attrFile = Join-Path $resolvedGitCommonDir 'info\attributes'
    $attrDir = Split-Path -Parent $attrFile
    if (-not (Test-Path $attrDir)) {
        New-Item -ItemType Directory -Path $attrDir -Force | Out-Null
    }
    $attrLine = ("{0} filter={1}" -f $AttributePath, $FilterName)
    $existingAttrs = @()
    if (Test-Path $attrFile) {
        $existingAttrs = @(Get-Content $attrFile -ErrorAction SilentlyContinue)
    }
    $escapedPath = [Regex]::Escape($AttributePath)
    $stripPattern = '^\s*' + $escapedPath + '[ \t]'
    $existingAttrs = @($existingAttrs | Where-Object { $_ -notmatch $stripPattern })
    $existingAttrs += $attrLine
    Set-Content -Path $attrFile -Value $existingAttrs -Encoding UTF8

    return $true
}

function Uninstall-McpFilter {
    <#
        Tears down the smudge/clean filter installed by Install-McpFilter:
          - Strips snapshot line from McpRelPath (so the working tree
            matches the canonical / upstream form).
          - Removes [filter "<FilterName>"] section from .git/config.
          - Removes the AttributePath line from .git/info/attributes.
          - Deletes <git-common-dir>/<FilterName>/ directory.
        Idempotent: calling on a clean state is a no-op.
        Returns one of: 'removed', 'not-installed', 'error'.
    #>
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)]
        [ValidateSet('xray-mcp', 'xray-vscode-mcp')]
        [string]$FilterName,
        [Parameter(Mandatory)]
        [string]$AttributePath,
        [Parameter(Mandatory)]
        [string]$McpRelPath,
        [switch]$DryRun
    )

    $resolvedGitDir = Get-ResolvedGitDir -RepoRoot $RepoRoot
    if (-not $resolvedGitDir) {
        return 'not-installed'
    }

    # See Install-McpFilter for the rationale: filter scripts and the
    # info/attributes file live under --git-common-dir (shared across
    # worktrees), not --git-dir (per-worktree). Reading from --git-dir
    # here would cause uninstall invoked from a linked worktree to remove
    # only the per-worktree config section while leaving the actual
    # filter scripts and attributes line intact under the common dir,
    # producing a half-uninstalled state where checkouts in OTHER
    # worktrees still see the filter and try to invoke now-orphaned
    # config that no longer exists.
    $resolvedGitCommonDir = Get-ResolvedGitCommonDir -RepoRoot $RepoRoot
    if (-not $resolvedGitCommonDir) {
        return 'not-installed'
    }

    $filterDir = Join-Path $resolvedGitCommonDir $FilterName
    $attrFile = Join-Path $resolvedGitCommonDir 'info\attributes'
    $mcpFile = Join-Path $RepoRoot $McpRelPath

    $hadAnything = $false

    # 1. Strip snapshot from McpRelPath (pure text op; doesn't depend on git or bash).
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
        & git config --local --get-regexp ('^filter\.' + [Regex]::Escape($FilterName) + '\.') 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $hasSection = $true }

        if ($hasSection) {
            $hadAnything = $true
            if ($DryRun) {
                Write-Host ('  [DryRun] Would remove [filter "{0}"] from .git/config' -f $FilterName) -ForegroundColor DarkYellow
            }
            else {
                # Capture exit code: a failed `--remove-section` (e.g.,
                # locked .git/config, permission error) must NOT be silently
                # swallowed -- leaving the filter section in place after the
                # scripts are deleted would cause every future git checkout
                # to invoke a missing command and either fail loudly or, with
                # required=false, silently passthrough (bad either way).
                & git config --local --remove-section ("filter.{0}" -f $FilterName) 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    Write-Warning ("git config --remove-section filter.{0} failed (exit {1}); .git/config may still reference the filter" -f $FilterName, $LASTEXITCODE)
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

    # 3. Remove the AttributePath line from .git/info/attributes.
    if (Test-Path $attrFile) {
        $existingAttrs = @(Get-Content $attrFile -ErrorAction SilentlyContinue)
        $stripPattern = '^\s*' + [Regex]::Escape($AttributePath) + '[ \t].*filter=' + [Regex]::Escape($FilterName)
        $filteredAttrs = @($existingAttrs | Where-Object { $_ -notmatch $stripPattern })
        if ($existingAttrs.Count -ne $filteredAttrs.Count) {
            $hadAnything = $true
            if ($DryRun) {
                Write-Host ("  [DryRun] Would remove {0} line from {1}" -f $AttributePath, $attrFile) -ForegroundColor DarkYellow
            }
            else {
                Set-Content -Path $attrFile -Value $filteredAttrs -Encoding UTF8
            }
        }
    }

    # 4. Delete <git-common-dir>/<FilterName>/.
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
    <#
        Walks the directory tree under $RootPath and tallies file
        extensions whose key is present in $KnownExtensions, skipping any
        directory whose immediate name is in $SkipDirectoryNames.

        IMPORTANT: pruning is applied at the DIRECTORY BOUNDARY, BEFORE
        descending. The previous implementation used
            Get-ChildItem -Recurse -File | Where-Object { ... not in skip ... }
        which still had to enumerate every file inside `node_modules`,
        `target`, `bin`, `obj`, `dist`, etc. and only THEN dropped them
        in PowerShell. On the xray repo itself (warm cache) that
        recursive enumeration walks ~67k files to keep ~380 — a ~178x
        over-walk that takes ~3.4s. On any larger repo with a populated
        `node_modules` (frontend monorepos, Electron apps) the same
        pattern blows up to tens of seconds.

        The new implementation uses an explicit stack +
        [IO.Directory]::EnumerateFiles / EnumerateDirectories so we never
        look inside a skipped directory at all. Same behavior on
        permission-denied / IO failure / long-path / security errors
        (silent — matches the previous `-ErrorAction SilentlyContinue`
        semantics, which swallowed everything); same case-insensitive
        skip-set semantics; same `-Force` equivalent (hidden dirs are
        visited unless explicitly listed in $SkipDirectoryNames, which
        is where `.git` / `.vs` / `.idea` / `.vscode` / etc. live).

        Returns a hashtable of extension -> count. Extensions are
        normalized lowercase, leading dot stripped.
    #>
    param(
        [string]$RootPath,
        [hashtable]$KnownExtensions,
        [string[]]$SkipDirectoryNames
    )

    $extCounts = @{}
    $skipSet = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $SkipDirectoryNames | ForEach-Object { $null = $skipSet.Add($_) }

    # Stack-based DFS. Using a Stack[string] (not List or recursion):
    # avoids PS function-call overhead per directory; bounds memory at
    # max-depth-of-tree rather than total-dir-count.
    $stack = [System.Collections.Generic.Stack[string]]::new()
    $stack.Push($RootPath)

    while ($stack.Count -gt 0) {
        $dir = $stack.Pop()

        # Files in this directory.
        #
        # Catch contract MUST match the previous Get-ChildItem -Recurse
        # -File -Force -ErrorAction SilentlyContinue behavior, which
        # swallowed EVERY error during enumeration (auth, IO, security,
        # long-path, etc.) and continued the walk. Catching only specific
        # exceptions here would narrow that contract and could abort
        # setup partway through scan on edge cases like deep node_modules
        # paths > MAX_PATH (PathTooLongException is an IOException) or
        # antivirus-locked files (IOException). UnauthorizedAccessException,
        # IOException, and SecurityException together cover the entire set
        # of exceptions the .NET enumerate APIs document as throwing.
        try {
            foreach ($file in [IO.Directory]::EnumerateFiles($dir)) {
                # Path.GetExtension returns ".cs" / "" — strip the dot
                # and lowercase. ToLowerInvariant matches the previous
                # behavior; OrdinalIgnoreCase on the hashtable key would
                # be a separate change.
                $ext = [IO.Path]::GetExtension($file)
                if ($ext.Length -gt 1) {
                    $ext = $ext.Substring(1).ToLowerInvariant()
                    if ($KnownExtensions.ContainsKey($ext)) {
                        if ($extCounts.ContainsKey($ext)) {
                            $extCounts[$ext]++
                        }
                        else {
                            $extCounts[$ext] = 1
                        }
                    }
                }
            }
        }
        catch [UnauthorizedAccessException] { } # silent — matches previous -ErrorAction SilentlyContinue
        catch [IO.IOException] { }              # incl. DirectoryNotFound, FileNotFound, PathTooLong
        catch [Security.SecurityException] { }

        # Subdirectories — prune by leaf name AND by reparse-point attr.
        #
        # Reparse points (symlinks / junctions / mount points) are NOT
        # followed. The previous Get-ChildItem -Recurse implementation
        # also did not follow them by default — that's the safe behavior
        # to avoid infinite loops on circular junctions and to avoid
        # accidentally descending into mapped network drives or backup
        # mount points. Switching to [IO.Directory]::EnumerateDirectories
        # changes that default (it DOES follow them), so we must opt out
        # explicitly. This was caught by a benchmark-vs-old comparison
        # on the xray repo itself, where `.github\skills\repo-kb\` is a
        # junction and the new implementation initially walked into it.
        #
        # Catch contract: same as the file-enumerate block — preserve the
        # silent-on-everything semantics of the previous Get-ChildItem
        # -ErrorAction SilentlyContinue.
        try {
            foreach ($sub in [IO.Directory]::EnumerateDirectories($dir)) {
                $leaf = [IO.Path]::GetFileName($sub)
                if ($skipSet.Contains($leaf)) { continue }
                try {
                    $attrs = [IO.File]::GetAttributes($sub)
                    if ($attrs -band [IO.FileAttributes]::ReparsePoint) { continue }
                }
                catch [UnauthorizedAccessException] { continue }
                catch [IO.IOException] { continue }              # incl. FileNotFound on race-removed dir
                catch [Security.SecurityException] { continue }
                $stack.Push($sub)
            }
        }
        catch [UnauthorizedAccessException] { }
        catch [IO.IOException] { }              # incl. DirectoryNotFound
        catch [Security.SecurityException] { }
    }

    return $extCounts
}

if (-not $RepoPath) {
    Write-Host 'Target repository = the LOCAL project folder you want to use xray with' -ForegroundColor Cyan
    Write-Host '(the repo xray will index and serve over MCP - NOT where xray.exe is installed).' -ForegroundColor Cyan
    Write-Host 'Example: C:\Repos\MyProject' -ForegroundColor DarkGray
    $RepoPath = Read-Host 'Enter the path to the target repository (e.g. C:\Repos\MyProject)'
}

try {
    $RepoPath = (Resolve-Path $RepoPath).Path
}
catch {
    Write-Error "Repository path not found: $RepoPath"
    exit 1
}

$gitWorkTreeRoot = Get-GitWorkTreeRoot -RepoRoot $RepoPath
$isGitRepo = $null -ne $gitWorkTreeRoot
if ($isGitRepo -and $gitWorkTreeRoot -ne $RepoPath) {
    Write-Host "Resolved git worktree root: $gitWorkTreeRoot" -ForegroundColor DarkGray
    $RepoPath = $gitWorkTreeRoot
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

    # 1. Tear down the smudge/clean filters for .mcp.json AND
    # .vscode/mcp.json (each is independent — one or the other may be
    # installed, both, or neither). When a filter is in use, the
    # snapshot-line strip already removes our entry without disturbing
    # upstream's formatting; we then skip the JSON-parse-and-rewrite step
    # for that file (which would reformat args arrays). $filterHandled is
    # tracked per relative path so the JSON-rewrite loop below can decide
    # per-file.
    $filterHandled = @{}
    if ($isGitRepo) {
        $filterTeardowns = @(
            @{ FilterName = 'xray-mcp';        AttributePath = '.mcp.json';        McpRelPath = '.mcp.json';        Label = '.mcp.json'        },
            @{ FilterName = 'xray-vscode-mcp'; AttributePath = '.vscode/mcp.json'; McpRelPath = '.vscode/mcp.json'; Label = '.vscode/mcp.json' }
        )
        foreach ($td in $filterTeardowns) {
            $filterStatus = Uninstall-McpFilter -RepoRoot $RepoPath -FilterName $td.FilterName -AttributePath $td.AttributePath -McpRelPath $td.McpRelPath -DryRun:$DryRun
            switch ($filterStatus) {
                'removed'        { Write-Host ("  Removed {0} smudge/clean filter ({1})." -f $td.Label, $td.FilterName) -ForegroundColor Green; $filterHandled[$td.McpRelPath] = $true }
                'not-installed'  { Write-Host ("  No {0} filter installed (skip)." -f $td.Label) -ForegroundColor DarkGray }
                default          { Write-Warning ("Filter teardown for {0} returned: {1}" -f $td.Label, $filterStatus) }
            }
        }
    }

    # 2. Strip 'xray' entries from each known mcp config (preserving other servers).
    # Skip .mcp.json / .vscode/mcp.json if the filter teardown already
    # handled them (avoids ConvertTo-Json reformatting upstream's args
    # arrays).
    # Roo support disabled: .roo/mcp.json entry kept commented out so legacy installs
    # can still be cleaned up via -Restore (which uses .bak files), but the new
    # uninstall flow no longer touches Roo.
    $configs = @(
        @{ Path = (Join-Path $RepoPath '.vscode\mcp.json'); RelPath = '.vscode/mcp.json'; Container = 'servers';    Label = 'VS Code (.vscode/mcp.json)'; SkipIfFilterHandled = $true  },
        # @{ Path = (Join-Path $RepoPath '.roo\mcp.json');    RelPath = '.roo/mcp.json';    Container = 'mcpServers'; Label = 'Roo (.roo/mcp.json)';        SkipIfFilterHandled = $false },
        @{ Path = (Join-Path $RepoPath '.mcp.json');        RelPath = '.mcp.json';        Container = 'mcpServers'; Label = 'Copilot CLI (.mcp.json)';    SkipIfFilterHandled = $true  }
    )
    foreach ($cfg in $configs) {
        if ($cfg.SkipIfFilterHandled -and $filterHandled[$cfg.RelPath]) {
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

    # 4. Refresh git's stat-cache so the files no longer appear modified.
    # We just rewrote .mcp.json / .vscode/mcp.json (stripping the snapshot
    # line); without this refresh, git status would show 'M' until the
    # next git operation that naturally updates stat info.
    if ($isGitRepo -and -not $DryRun) {
        Push-Location $RepoPath
        try {
            foreach ($rel in @('.mcp.json', '.vscode/mcp.json')) {
                if (Test-Path (Join-Path $RepoPath $rel)) {
                    & git update-index --refresh -- $rel 2>$null | Out-Null
                }
            }
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

if (-not $isGitRepo) {
    Write-Warning "$RepoPath does not appear to be a git work tree (git rev-parse --show-toplevel failed)"
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
        $userInput = Read-Host "`nAccept suggested extensions, or enter your own (comma-separated) [Enter = accept]"
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

# Resolve the git visibility mode for the xray entry written into mcp.json.
#   Visible: write a normal, user-visible change (with a "//" warning) - no
#     smudge/clean filter, no skip-worktree, no .git/info/exclude. If the file
#     is git-tracked the user sees it in 'git status' and decides about it.
#   Hidden: install the smudge/clean filter (or skip-worktree / .git/info/exclude
#     fallback) so the entry never shows in git.
# Explicit -GitVisibility always wins. Outside a git repo there is nothing to
# hide, so default to Visible without prompting.
if ($GitVisibility) {
    $visibleMode = ($GitVisibility -eq 'Visible')
}
elseif (-not $isGitRepo) {
    $visibleMode = $true
}
elseif ($Force) {
    $visibleMode = $false
}
else {
    $visibleMode = Read-YesNo -Prompt 'Write the xray entry as a normal VISIBLE change to mcp.json (you decide whether to commit it)? Answer No to hide it from git' -Default $true
}
Write-Host ("Git visibility mode: {0}" -f $(if ($visibleMode) { 'Visible (normal git change)' } else { 'Hidden (filter / skip-worktree)' })) -ForegroundColor Cyan

# In visible mode we may need to lift a prior hidden install's protection so
# the file actually shows in git. Resolve .git/info/exclude once for that.
$visibleExcludePath = $null
if ($isGitRepo -and $visibleMode) {
    Push-Location $RepoPath
    try {
        $visibleExcludePath = & git rev-parse --git-path info/exclude 2>$null
        if ($LASTEXITCODE -ne 0) { $visibleExcludePath = $null }
    }
    finally { Pop-Location }
}

$vscodeMcpDir = Join-Path $RepoPath '.vscode'
$vscodeMcpPath = Join-Path $vscodeMcpDir 'mcp.json'
$vscodeMcpExistedBefore = Test-Path $vscodeMcpPath
$writeVscode = $false
$protectExistingUntrackedVscode = $false
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
            if (-not $writeVscode) {
                $protectExistingUntrackedVscode = $true
            }
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

    $vscodeFilterInstalled = $false

    if ($visibleMode) {
        # VISIBLE mode: write a normal, user-visible xray entry into
        # .vscode/mcp.json. Preserve existing formatting and other servers via
        # line-injection, and never hide the change from git. If the file is
        # tracked, the user sees it in 'git status' and owns the commit decision.
        $vscodeEntryLine = New-XrayVisibleEntryLine -XrayPath $xrayPath -XrayArgs $xrayArgs -Shape 'VsCode'

        # Lift any prior hidden protection (skip-worktree on tracked files,
        # .git/info/exclude on untracked) so the visible write shows in git.
        if ($isGitRepo) {
            Push-Location $RepoPath
            try { [void](Remove-GitProtection -RelativePath '.vscode/mcp.json' -ExcludePath $visibleExcludePath) }
            finally { Pop-Location }
        }

        $vscodeVisibleOk = Set-McpFileVisibleEntry -Path $vscodeMcpPath -EntryLine $vscodeEntryLine -ContainerKey 'servers' -XrayPath $xrayPath -XrayArgs $xrayArgs -Shape 'VsCode'
        if ($vscodeVisibleOk) {
            # Tear down any prior hidden smudge/clean filter so the switch to
            # visible is clean (no-op when no filter was installed).
            if ($isGitRepo) {
                [void](Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-vscode-mcp' -AttributePath '.vscode/mcp.json' -McpRelPath '.vscode/mcp.json')
            }
            Write-Host "Configured xray (visible) in $vscodeMcpPath. If this file is git-tracked, the change shows in 'git status' - you decide whether to commit it." -ForegroundColor Green
        }
        else {
            Write-Warning "Could not write a visible xray entry to $vscodeMcpPath (left untouched). See the warning above."
            $writeVscode = $false
        }
    }
    else {
        # Decide between two strategies (mirrors the .mcp.json install logic):
        #   - Filter strategy (.vscode/mcp.json is tracked in a git repo): install
        #     smudge/clean filter so 'git pull' merges upstream changes silently
        #     and 'git status' stays clean.
        #   - Plain JSON strategy (no git repo, OR file is untracked): merge xray
        #     entry via JSON parse+rewrite, fall back on .git/info/exclude or
        #     skip-worktree (handled later in the git-protect block).
        $useVscodeFilterStrategy = $false
        if ($isGitRepo) {
            $isVscodeTracked = Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath '.vscode/mcp.json'
            $useVscodeFilterStrategy = [bool]$isVscodeTracked
        }

    if ($useVscodeFilterStrategy) {
        # In-memory invocation (e.g. `& ([scriptblock]::Create((iwr ...).Content))`
        # or `iex (irm ...)`) leaves $MyInvocation.MyCommand.Path as $null,
        # so a naive `Split-Path -Parent $null` aborts the entire install.
        # Preserve $null here so Install-McpFilter takes the embedded
        # fallback path unconditionally.
        $scriptPath = $MyInvocation.MyCommand.Path
        $scriptDir = if ($scriptPath) { Split-Path -Parent $scriptPath } else { $null }
        $vscodeSnapshotLine = New-XraySnapshotLine -XrayPath $xrayPath -XrayArgs $xrayArgs -Shape 'VsCode'

        # Lift any pre-existing skip-worktree from a legacy install BEFORE we
        # touch the file - otherwise our write would silently no-op in the index.
        Push-Location $RepoPath
        try {
            & git update-index --no-skip-worktree '.vscode/mcp.json' 2>$null | Out-Null
        }
        finally {
            Pop-Location
        }

        if (Install-McpFilter -RepoRoot $RepoPath -ScriptDir $scriptDir -SnapshotLine $vscodeSnapshotLine -FilterName 'xray-vscode-mcp' -ContainerKey 'servers' -AttributePath '.vscode/mcp.json') {
            $vscodeFilterInstalled = $true
            $injected = Set-McpFileWithSnapshot -Path $vscodeMcpPath -SnapshotLine $vscodeSnapshotLine -ContainerKey 'servers'
            if ($injected) {
                # Renormalize so the index reflects the canonical (clean) form.
                Push-Location $RepoPath
                try {
                    & git add --renormalize '.vscode/mcp.json' 2>&1 | Out-Null
                    if ($LASTEXITCODE -ne 0) {
                        Write-Warning ("git add --renormalize .vscode/mcp.json failed (exit {0}). The xray line may show as a diff until the next clean filter pass." -f $LASTEXITCODE)
                    }
                }
                finally {
                    Pop-Location
                }
                Write-Host "Configured xray in $vscodeMcpPath via smudge/clean filter (git status stays clean across upstream pulls)." -ForegroundColor Green
            }
            else {
                Write-Warning 'Filter installation succeeded but the tracked .vscode/mcp.json could not be injected with the xray entry.'
                Write-Warning 'Rolling back filter artifacts and aborting; otherwise setup would finish with no VS Code xray server registered.'
                $rollbackResult = Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-vscode-mcp' -AttributePath '.vscode/mcp.json' -McpRelPath '.vscode/mcp.json'
                Write-Warning ("Rollback result: {0}. Reformat .vscode/mcp.json so the servers object is multiline or empty-inline, then re-run setup." -f $rollbackResult)
                exit 1
            }
        }
        else {
            # FAIL-CLOSED: same rationale as the .mcp.json branch - a
            # tracked upstream .vscode/mcp.json must NOT be rewritten via
            # ConvertTo-Json (loses single-element-array structure, rewrites
            # whitespace, reintroduces the pull-abort/diff damage class the
            # filter migration replaced). The legacy skip-worktree fallback
            # is also unsafe: any future `git stash`/`reset --hard`/`checkout`
            # would silently lift the bit and expose the xray entry as a real
            # diff against upstream.
            #
            # Roll back any partial filter artifacts and abort the VS Code
            # install for this repo.
            Write-Warning 'Filter installation failed for the tracked upstream .vscode/mcp.json.'
            Write-Warning 'Refusing to fall back to JSON merge + skip-worktree (would corrupt upstream formatting and create silent diff hazards on future git operations).'
            Write-Warning 'Rolling back any partial filter artifacts and aborting the VS Code install for this repo.'
            $rollbackResult = Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-vscode-mcp' -AttributePath '.vscode/mcp.json' -McpRelPath '.vscode/mcp.json'
            Write-Warning ("Rollback result: {0}. Investigate the Install-McpFilter warning above (likely missing bash, missing filter source files, or .git/info not writable) and re-run." -f $rollbackResult)
            $writeVscode = $false
        }
    }

    if (-not $useVscodeFilterStrategy -and $writeVscode) {
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
$copilotCliMcpExistedBefore = Test-Path $copilotCliMcpPath
$writeCopilotCli = $false
$protectExistingUntrackedCopilotCli = $false
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
            if (-not $writeCopilotCli) {
                $protectExistingUntrackedCopilotCli = $true
            }
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

    $copilotCliFilterInstalled = $false

    if ($visibleMode) {
        # VISIBLE mode: write a normal, user-visible xray entry into .mcp.json.
        # Preserve existing formatting and other servers via line-injection, and
        # never hide the change from git. If the file is tracked, the user sees
        # it in 'git status' and owns the commit decision.
        $copilotCliEntryLine = New-XrayVisibleEntryLine -XrayPath $xrayPath -XrayArgs $xrayArgs -Shape 'CopilotCli'

        # Lift any prior hidden protection (skip-worktree on tracked files,
        # .git/info/exclude on untracked) so the visible write shows in git.
        if ($isGitRepo) {
            Push-Location $RepoPath
            try { [void](Remove-GitProtection -RelativePath '.mcp.json' -ExcludePath $visibleExcludePath) }
            finally { Pop-Location }
        }

        $copilotCliVisibleOk = Set-McpFileVisibleEntry -Path $copilotCliMcpPath -EntryLine $copilotCliEntryLine -ContainerKey 'mcpServers' -XrayPath $xrayPath -XrayArgs $xrayArgs -Shape 'CopilotCli'
        if ($copilotCliVisibleOk) {
            # Tear down any prior hidden smudge/clean filter so the switch to
            # visible is clean (no-op when no filter was installed).
            if ($isGitRepo) {
                [void](Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-mcp' -AttributePath '.mcp.json' -McpRelPath '.mcp.json')
            }
            Write-Host "Configured xray (visible) in $copilotCliMcpPath. If this file is git-tracked, the change shows in 'git status' - you decide whether to commit it." -ForegroundColor Green
        }
        else {
            Write-Warning "Could not write a visible xray entry to $copilotCliMcpPath (left untouched). See the warning above."
            $writeCopilotCli = $false
        }
    }
    else {
        # Decide between two strategies:
        #   - Filter strategy (.mcp.json is tracked OR exists with upstream content
        #     in a git repo): install smudge/clean filter so 'git pull' merges
        #     upstream changes silently and 'git status' stays clean.
        #   - Plain JSON strategy (no git repo, OR file is untracked AND we're
        #     creating it fresh): merge xray entry via JSON parse+rewrite, fall
        #     back on .git/info/exclude or skip-worktree (handled later in the
        #     git-protect block).
        $useFilterStrategy = $false
        if ($isGitRepo) {
            $isTracked = Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath '.mcp.json'
            $useFilterStrategy = [bool]$isTracked
        }

    if ($useFilterStrategy) {
        # In-memory invocation (e.g. `& ([scriptblock]::Create((iwr ...).Content))`
        # or `iex (irm ...)`) leaves $MyInvocation.MyCommand.Path as $null,
        # so a naive `Split-Path -Parent $null` aborts the entire install.
        # Preserve $null here so Install-McpFilter takes the embedded
        # fallback path unconditionally.
        $scriptPath = $MyInvocation.MyCommand.Path
        $scriptDir = if ($scriptPath) { Split-Path -Parent $scriptPath } else { $null }
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

        if (Install-McpFilter -RepoRoot $RepoPath -ScriptDir $scriptDir -SnapshotLine $snapshotLine -FilterName 'xray-mcp' -ContainerKey 'mcpServers' -AttributePath '.mcp.json') {
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
            else {
                Write-Warning 'Filter installation succeeded but the tracked .mcp.json could not be injected with the xray entry.'
                Write-Warning 'Rolling back filter artifacts and aborting; otherwise setup would finish with no Copilot CLI xray server registered.'
                $rollbackResult = Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-mcp' -AttributePath '.mcp.json' -McpRelPath '.mcp.json'
                Write-Warning ("Rollback result: {0}. Reformat .mcp.json so the mcpServers object is multiline or empty-inline, then re-run setup." -f $rollbackResult)
                exit 1
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
            $rollbackResult = Uninstall-McpFilter -RepoRoot $RepoPath -FilterName 'xray-mcp' -AttributePath '.mcp.json' -McpRelPath '.mcp.json'
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
}

# Hidden mode only: install git protection (skip-worktree / .git/info/exclude)
# so the xray entry never shows in 'git status'. In visible mode we intentionally
# leave the change visible, so this entire block is skipped.
if ($isGitRepo -and -not $visibleMode) {
    Push-Location $RepoPath
    try {
        $skipWorktreeFiles = @()
        $excludedFiles = @()
        $gitProtectionFailed = $false

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
                @{ Path = '.vscode/mcp.json'; Protect = ((($writeVscode -and -not $vscodeFilterInstalled) -or $protectExistingUntrackedVscode) -and (Test-Path (Join-Path $RepoPath '.vscode/mcp.json'))); UntrackedOnly = ($protectExistingUntrackedVscode -and -not ($writeVscode -and -not $vscodeFilterInstalled)); Generated = (-not $vscodeMcpExistedBefore) },
                @{ Path = '.roo/mcp.json'; Protect = ($writeRoo -and (Test-Path (Join-Path $RepoPath '.roo/mcp.json'))); UntrackedOnly = $false; Generated = $false },
                @{ Path = '.mcp.json'; Protect = ((($writeCopilotCli -and -not $copilotCliFilterInstalled) -or $protectExistingUntrackedCopilotCli) -and (Test-Path (Join-Path $RepoPath '.mcp.json'))); UntrackedOnly = ($protectExistingUntrackedCopilotCli -and -not ($writeCopilotCli -and -not $copilotCliFilterInstalled)); Generated = (-not $copilotCliMcpExistedBefore) }
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
                if ($mcpFile.UntrackedOnly) {
                    continue
                }

                & git update-index --skip-worktree $rel 2>$null | Out-Null
                if ($LASTEXITCODE -eq 0) {
                    $skipWorktreeFiles += $rel
                }
                else {
                    Write-Warning ("git update-index --skip-worktree {0} failed (exit {1})" -f $rel, $LASTEXITCODE)
                }
            }
            elseif ($excludePath) {
                $addedExcludeLine = Add-TextLineNoBom -Path $excludePath -Line $rel

                if (-not (Test-GitIgnoresPath -RepoRoot $RepoPath -RelativePath $rel)) {
                    [void](Add-DirectoryGitignoreOverride -RepoRoot $RepoPath -RelativePath $rel -ExcludePath $excludePath)
                }

                if (Test-GitIgnoresPath -RepoRoot $RepoPath -RelativePath $rel) {
                    $excludedFiles += $rel
                }
                else {
                    Write-Warning ("Git protection for {0} was ineffective. A higher-priority .gitignore rule may be re-including it." -f $rel)
                    if ($addedExcludeLine) {
                        [void](Remove-TextLineNoBom -Path $excludePath -Line $rel)
                    }
                    if ($mcpFile.Generated) {
                        Remove-Item -Path (Join-Path $RepoPath $rel) -Force -ErrorAction SilentlyContinue
                        Write-Warning ("Removed generated {0} because it could not be hidden from git status." -f $rel)
                    }
                    $gitProtectionFailed = $true
                }
            }
        }

        if ($gitProtectionFailed) {
            Write-Error 'Git protection failed for one or more generated MCP configs. Setup aborted so local-only MCP files are not accidentally committed.'
            exit 1
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
Write-Host ("Git visibility: {0}" -f $(if ($visibleMode) { 'Visible (normal git change)' } else { 'Hidden from git' }))
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
    if ($visibleMode) {
        Write-Host "Visible mode: the xray entry is a normal change in mcp.json. If the file is git-tracked, review 'git status' / 'git diff' and decide whether to commit it." -ForegroundColor Yellow
    }
}

exit 0
