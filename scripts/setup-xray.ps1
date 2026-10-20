#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Downloads the latest xray release and configures MCP for a target repository.

.DESCRIPTION
    1. Downloads xray.exe from the latest GitHub release (if not already installed)
    2. Detects file extensions in the target repo heuristically
    3. Configures both GitHub Copilot CLI (.github/mcp.json) and VS Code
       (.vscode/mcp.json), unless client switches select only one.
    4. Excludes generated configs and backups from Git locally. Tracked files
       are never changed by ordinary setup; no Git filters are installed.
    5. Offers persistent Copilot CLI folder trust in interactive mode.
    6. Creates a local copilot-xray.cmd launcher when a root .mcp.json would
       take precedence over .github/mcp.json, without changing the root config.
    Legacy root configs and Git protection are cleaned up only with explicit
    migration consent. Roo Code setup remains disabled.

.PARAMETER RepoPath
    Path to the LOCAL project repository you want to use xray with (the folder
    xray will index and serve over MCP) - e.g. C:\Repos\MyProject. This is NOT
    where xray.exe is installed (see -InstallDir). If not specified, the script
    prompts for it interactively.

.PARAMETER InstallDir
    Where to install xray.exe. Defaults to %LOCALAPPDATA%\xray.

.PARAMETER GithubRepo
    GitHub repository that publishes xray.exe releases. Public assets download
    directly over HTTPS; authenticated GitHub CLI is only a fallback for private
    or restricted repositories.

.PARAMETER Extensions
    Optional comma-separated list of extensions to index. When provided,
    skips repository scanning and interactive extension selection.

.PARAMETER SkipDownload
    Skip downloading xray.exe and use the existing installation.

.PARAMETER EnableVSCode
    Select VS Code setup. Without either client switch, both clients are selected.
    With -UserMcp, the CLI/SDK registration is also included for Agent Host.

.PARAMETER EnableRoo
    DISABLED. Kept as a no-op switch for backward compat with caller scripts.
    Roo configuration is not written, restored, or removed.

.PARAMETER EnableCopilotCli
    Select GitHub Copilot CLI setup in .github/mcp.json. Without either client
    switch, both clients are selected.
    With -UserMcp, select personal CLI/SDK setup and omit VS Code unless
    -EnableVSCode is also supplied.

.PARAMETER UserMcp
    Register Xray in personal Copilot CLI/SDK and VS Code MCP configurations.
    Store separate workspace profiles under InstallDir\workspaces. This mode
    does not require a CLI wrapper or changes to tracked workspace files.
    Without this switch, setup keeps the existing workspace-local behavior.
    With -EnableVSCode, CLI/SDK registration is also configured for Agent Host.

.PARAMETER VSCodeMcpPath
    Personal VS Code MCP file for -UserMcp. By default, use the saved folder
    profile association in stable VS Code. Specify this path for other editions,
    custom user-data directories, or multi-root workspace profiles.

.PARAMETER GitVisibility
    Deprecated. Accepted with a warning for existing callers. Both values now
    use local-only setup and never install a filter or modify a tracked config.

.PARAMETER MigrateLegacy
    Explicitly authorize removal of old Xray entries and their Git protection
    after the selected new configurations are ready. Also enables legacy cleanup
    with -Uninstall. Without this switch, interactive setup asks (default No);
    non-interactive setup leaves legacy state untouched.
    Shared legacy filters with multiple worktrees require coordinated cleanup
    in a single-worktree clone; setup refuses to remove their shared metadata.

.PARAMETER Force
    Run non-interactively where possible: overwrite existing xray entry, overwrite
    existing xray.exe and accept suggested extensions. Both clients are selected
    unless client switches select one. Does not authorize legacy cleanup or
    grant Copilot CLI trust.
    Implies -KillRunning when xray.exe is locked by a running process.

.PARAMETER KillRunning
    If xray.exe is in use (e.g. running as an MCP server in another VS Code instance),
    terminate all running xray.exe processes without prompting before overwriting the
    binary. Without this switch, the script asks for confirmation interactively.

.PARAMETER Restore
    Cannot be combined with -Uninstall.
    With -UserMcp: restore only the selected workspace profile backup.
    Restore .vscode/mcp.json and .github/mcp.json from the .bak files created on the
    previous setup run, then exit. Skips download and extension detection.
    Use this to undo the most recent setup-xray run.
    Tracked target files are not restored. Ordinary setup creates a backup only
    when changing an existing configuration; an unchanged rerun makes no writes.
    Removes the managed CLI launcher if the restored CLI config has no Xray entry
    or the root .mcp.json is gone. Launcher cleanup errors do not prevent the
    other client from being restored, but restore exits nonzero.
    If no eligible .bak exists when -Restore is invoked,
    the script exits with an error.

.PARAMETER Uninstall
    Cannot be combined with -Restore.
    With -UserMcp: remove the selected workspace profile. Keep shared client
    registrations while other profiles remain; otherwise remove matching
    managed registrations from the selected personal configs. Keep the binary.
    Fully remove xray from the target repository:
      - Strip the 'xray' server entry from local .vscode/mcp.json and
        .github/mcp.json (other servers preserved; tracked targets skipped).
      - If a config file would become empty AND was created by this script
        (no upstream content), delete it.
      - Legacy root entries, filters and index flags require migration consent.
      - Delete .bak files (unless -KeepBackups).
      - Remove the managed, untracked copilot-xray.cmd launcher.
      - Delete the xray.exe binary in -InstallDir (unless -KeepBinary).
    Idempotent: re-running on a clean state is a no-op.
    Does NOT touch:
      - upstream-managed servers in any mcp.json
      - Copilot CLI trustedFolders or global MCP configuration
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
    [switch]$UserMcp,
    [string]$VSCodeMcpPath,
    [ValidateSet('Visible', 'Hidden')]
    [string]$GitVisibility,
    [switch]$MigrateLegacy,
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

$KnownCodeExtensions = @{
    'cs' = 'C#'; 'csx' = 'C# Script'; 'csproj' = 'C# Project';
    'vb' = 'VB.NET'; 'vbproj' = 'VB Project';
    'fsproj' = 'F# Project'; 'vcxproj' = 'C++ Project';
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
    'xml' = 'XML'; 'resx' = 'Resource XML'; 'json' = 'JSON'; 'jsonc' = 'JSONC';
    'yaml' = 'YAML'; 'yml' = 'YAML';
    'config' = 'Config'; 'ini' = 'INI'; 'env' = 'Env';
    'manifestxml' = 'Manifest XML'; 'appxmanifest' = 'Appx Manifest'; 'vsixmanifest' = 'VSIX Manifest';
    'md' = 'Markdown'; 'txt' = 'Text'; 'rst' = 'reStructuredText';
    'sql' = 'SQL';
    'scala' = 'Scala'; 'clj' = 'Clojure'; 'ex' = 'Elixir'; 'erl' = 'Erlang';
    'dart' = 'Dart';
    'lua' = 'Lua';
    'r' = 'R'; 'rmd' = 'R Markdown';
    'tf' = 'Terraform'; 'hcl' = 'HCL'
}

$AlwaysIncludeIfDetected = @(
    'md',
    'sln',
    'csproj', 'vbproj', 'fsproj', 'vcxproj',
    'props', 'targets',
    'xaml',
    'razor', 'cshtml',
    'xml', 'config', 'resx',
    'manifestxml', 'appxmanifest', 'vsixmanifest'
)

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

function Read-ExtensionSelection {
    param([string]$Suggested)

    $maxAttempts = 10
    $useCustom = $false
    for ($attempt = 0; $attempt -lt $maxAttempts; $attempt++) {
        $answer = Read-Host 'Use the suggested extensions? [Y/n]'
        if ([string]::IsNullOrWhiteSpace($answer) -or $answer.Trim() -match '^(y|yes)$') {
            return $Suggested
        }
        if ($answer.Trim() -match '^(n|no)$') {
            $useCustom = $true
            break
        }
        Write-Host 'Answer y/yes, n/no, or press Enter to accept.' -ForegroundColor Yellow
    }
    if (-not $useCustom) {
        throw 'No valid confirmation received. Re-run setup or supply -Extensions explicitly.'
    }

    for ($attempt = 0; $attempt -lt $maxAttempts; $attempt++) {
        $userInput = Read-Host 'Enter extensions separated by commas (e.g. cs,sql,md)'
        if ([string]::IsNullOrWhiteSpace($userInput) -or $userInput.Trim() -match '^(y|yes|n|no)$') {
            Write-Host 'Enter an extension list, not a yes/no answer. For .y or .n files, include the dot.' -ForegroundColor Yellow
            continue
        }
        $selected = Normalize-ExtensionList -Value $userInput
        if ($selected) {
            return $selected
        }
        Write-Host 'Enter at least one extension.' -ForegroundColor Yellow
    }
    throw 'No valid extension list received. Input may be closed. Re-run setup or supply -Extensions explicitly.'
}

function Test-WindowsExecutable {
    param([Parameter(Mandatory)] [string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $false
    }

    try {
        $stream = [IO.File]::OpenRead($Path)
        try {
            if ($stream.Length -lt 64 -or $stream.ReadByte() -ne 0x4D -or $stream.ReadByte() -ne 0x5A) {
                return $false
            }

            $dosHeader = New-Object byte[] 64
            $stream.Position = 0
            if ($stream.Read($dosHeader, 0, $dosHeader.Length) -ne $dosHeader.Length) {
                return $false
            }
            $peOffset = [BitConverter]::ToInt32($dosHeader, 0x3C)
            if ($peOffset -lt 0 -or $peOffset -gt ($stream.Length - 4)) {
                return $false
            }

            $stream.Position = $peOffset
            return $stream.ReadByte() -eq 0x50 -and
                $stream.ReadByte() -eq 0x45 -and
                $stream.ReadByte() -eq 0 -and
                $stream.ReadByte() -eq 0
        }
        finally {
            $stream.Dispose()
        }
    }
    catch {
        return $false
    }
}

function Save-XrayReleaseAsset {
    param(
        [Parameter(Mandatory)] [string]$GithubRepo,
        [Parameter(Mandatory)] [string]$Destination
    )

    if ($GithubRepo -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$' -or $GithubRepo -match '(^|/)\.{1,2}($|/)') {
        throw "Invalid GitHub repository '$GithubRepo'. Expected owner/repo."
    }

    $downloadUri = "https://github.com/$GithubRepo/releases/latest/download/xray.exe"
    $oldProgressPreference = $ProgressPreference
    $oldSecurityProtocol = [Net.ServicePointManager]::SecurityProtocol
    try {
        $ProgressPreference = 'SilentlyContinue'
        if ($PSVersionTable.PSVersion.Major -lt 6) {
            [Net.ServicePointManager]::SecurityProtocol = $oldSecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        }
        Invoke-WebRequest -Uri $downloadUri -OutFile $Destination -UseBasicParsing -ErrorAction Stop
    }
    catch {
        $directDownloadError = $_.Exception.Message
        $ghCommand = Get-Command gh -CommandType Application -ErrorAction SilentlyContinue
        if (-not $ghCommand) {
            throw ("Download failed from {0}: {1}" -f $downloadUri, $directDownloadError)
        }

        Write-Warning 'Direct public download failed; retrying with authenticated GitHub CLI for a private or restricted repository.'
        Remove-Item $Destination -Force -ErrorAction SilentlyContinue
        & $ghCommand.Source release download --repo $GithubRepo --pattern 'xray.exe' --dir (Split-Path -Parent $Destination)
        if ($LASTEXITCODE -ne 0) {
            throw "GitHub CLI download failed for $GithubRepo (exit $LASTEXITCODE)."
        }
    }
    finally {
        $ProgressPreference = $oldProgressPreference
        [Net.ServicePointManager]::SecurityProtocol = $oldSecurityProtocol
    }

    if (-not (Test-WindowsExecutable -Path $Destination)) {
        throw 'Download failed - release asset is not a valid Windows executable'
    }
}

function Get-AutoSelectedExtensions {
    param(
        [Parameter(Mandatory)] [System.Collections.IDictionary]$ExtensionCounts,
        [Parameter(Mandatory)] [int]$Threshold,
        [Parameter(Mandatory)] [string[]]$AlwaysIncludeIfDetected
    )

    $thresholdSelected = @($ExtensionCounts.GetEnumerator() |
            Where-Object { $_.Value -ge $Threshold } |
            Select-Object -ExpandProperty Key)

    $structuralSelected = @($AlwaysIncludeIfDetected |
            Where-Object {
                $ExtensionCounts.Contains($_) -and
                $ExtensionCounts[$_] -lt $Threshold
            })

    $allSelected = @($thresholdSelected) + @($structuralSelected)
    return [PSCustomObject]@{
        ThresholdSelected  = @($thresholdSelected | Sort-Object -Unique)
        StructuralSelected = @($structuralSelected | Sort-Object -Unique)
        All                = @($allSelected | Sort-Object -Unique)
    }
}

function Install-XrayBinaryWithRetry {
    param(
        [Parameter(Mandatory)] [string]$Downloaded,
        [Parameter(Mandatory)] [string]$Destination
    )

    $Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)
    $maxAttempts = 5
    $deploymentError = $null
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        $running = @(Get-Process -Name 'xray' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Destination })
        foreach ($proc in $running) {
            try {
                Stop-Process -Id $proc.Id -Force -ErrorAction Stop
                Write-Host "Stopped PID $($proc.Id)." -ForegroundColor DarkYellow
            }
            catch {
                if (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue) {
                    throw "Failed to stop PID $($proc.Id): $($_.Exception.Message)"
                }
            }
        }
        $running | Wait-Process -Timeout 5 -ErrorAction SilentlyContinue

        try {
            Move-Item -LiteralPath $Downloaded -Destination $Destination -Force -ErrorAction Stop
            return
        }
        catch [System.IO.IOException], [System.UnauthorizedAccessException] {
            $deploymentError = $_
            if ($attempt -lt $maxAttempts) {
                Start-Sleep -Milliseconds (250 * $attempt)
            }
        }
    }

    throw "Cannot replace $Destination after $maxAttempts attempts. Last error: $($deploymentError.Exception.Message)"
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

$Script:XrayMcpMarker = 'managed-by-setup-xray.ps1-do-not-edit'
$Script:XrayVisibleWarning = 'xray MCP entry added by setup-xray.ps1 (embeds a per-machine local path). If this file is git-tracked, MAKE SURE you want to commit/push this change.'

function ConvertFrom-SetupJson {
    param([Parameter(Mandatory)] [string]$Text, [Parameter(Mandatory)] [string]$Path)

    # Copilot CLI prefixes its managed config with // header comments.
    $jsonText = $Text -replace '\A(?:\s*//[^\r\n]*(?:\r\n|\r|\n|\z))*', ''
    $parameters = @{ InputObject = $jsonText; ErrorAction = 'Stop' }
    if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) {
        $parameters.DateKind = 'String'
    }
    $useTokenReader = $PSVersionTable.PSVersion.Major -ge 7 -and -not $parameters.ContainsKey('DateKind')
    try {
        if ($useTokenReader) {
            # PowerShell 7.0-7.4 otherwise converts date-looking strings to dates.
            $reader = [Newtonsoft.Json.JsonTextReader]::new([IO.StringReader]::new($jsonText))
            try {
                $reader.DateParseHandling = [Newtonsoft.Json.DateParseHandling]::None
                # The reader counts the root object; the serializer starts at depth zero.
                $reader.MaxDepth = 101
                $token = [Newtonsoft.Json.Linq.JToken]::ReadFrom($reader)
                if ($reader.Read()) { throw 'Unexpected trailing JSON content.' }
            }
            finally { $reader.Close() }
        }
        else {
            $value = ConvertFrom-Json @parameters
        }
    }
    catch {
        throw "Invalid JSON in $Path. The file was not overwritten."
    }
    if ($useTokenReader) { $value = ConvertFrom-SetupJsonToken -Token $token }
    if ($value -isnot [PSCustomObject]) {
        throw "Expected a JSON object in $Path. The file was not overwritten."
    }
    return $value
}

function ConvertFrom-SetupJsonToken {
    param([Parameter(Mandatory)] $Token)

    switch ($Token.get_Type().ToString()) {
        'Object' {
            $properties = [ordered]@{}
            foreach ($property in $Token.Properties()) {
                $properties[$property.get_Name()] = ConvertFrom-SetupJsonToken -Token $property.get_Value()
            }
            return [PSCustomObject]$properties
        }
        'Array' {
            $items = [Collections.Generic.List[object]]::new()
            foreach ($item in $Token.Children()) { $items.Add((ConvertFrom-SetupJsonToken -Token $item)) }
            return ,$items.ToArray()
        }
        'Null' { return $null }
        default { return $Token.get_Value() }
    }
}

function Read-SetupJson {
    param([Parameter(Mandatory)] [string]$Path)

    $text = $null
    $value = [PSCustomObject]@{}
    if (Test-Path -LiteralPath $Path) {
        $text = [IO.File]::ReadAllText($Path)
        $value = ConvertFrom-SetupJson -Text $text -Path $Path
    }
    return [PSCustomObject]@{ Path = $Path; Text = $text; Value = $value }
}

function Backup-McpJson {
    param([Parameter(Mandatory)] [string]$Path)

    if (Test-Path -LiteralPath $Path) {
        if ((Test-Path -LiteralPath "$Path.bak") -and -not (Test-Path -LiteralPath "$Path.bak" -PathType Leaf)) {
            throw "Backup path is not a file: $Path.bak"
        }
        Copy-Item -LiteralPath $Path -Destination "$Path.bak" -Force -ErrorAction Stop
        Write-Host "Backed up existing config to $Path.bak" -ForegroundColor DarkGray
    }
}

function Write-SetupText {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [AllowEmptyString()] [string]$Text,
        [AllowNull()] $OriginalText,
        [switch]$Backup
    )

    $exists = Test-Path -LiteralPath $Path
    if ($exists -and [IO.File]::ReadAllText($Path) -ceq $Text) { return $false }
    if (($null -eq $OriginalText -and $exists) -or
        ($null -ne $OriginalText -and (-not $exists -or [IO.File]::ReadAllText($Path) -cne $OriginalText))) {
        throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes."
    }
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $temp = Join-Path $parent ([IO.Path]::GetRandomFileName())
    try {
        [IO.File]::WriteAllText($temp, $Text, [Text.UTF8Encoding]::new($false))
        if ($Backup) { Backup-McpJson -Path $Path }
        if ($exists) {
            if ([IO.File]::ReadAllText($Path) -cne $OriginalText) {
                throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes."
            }
            [IO.File]::Replace($temp, $Path, [NullString]::Value)
        }
        else {
            [IO.File]::Move($temp, $Path)
        }
    }
    finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force }
    }
    return $true
}

function ConvertTo-SetupJson {
    param([Parameter(Mandatory)] $Value, [Parameter(Mandatory)] [string]$Path, [switch]$Compress)

    try {
        return ConvertTo-Json -InputObject $Value -Depth 100 -Compress:$Compress -WarningAction Stop -ErrorAction Stop
    }
    catch {
        throw "Cannot safely serialize JSON in $Path. Check the supported depth of 100. The file was not overwritten."
    }
}

function Write-SetupJson {
    param([Parameter(Mandatory)] $Document, [switch]$Backup)

    $after = ConvertTo-SetupJson -Value $Document.Value -Path $Document.Path -Compress
    if ($null -ne $Document.Text) {
        $before = ConvertFrom-SetupJson -Text $Document.Text -Path $Document.Path
        if ((ConvertTo-SetupJson -Value $before -Path $Document.Path -Compress) -ceq $after) { return $false }
    }
    $text = ConvertTo-SetupJson -Value $Document.Value -Path $Document.Path
    $separator = if ($Document.Text -and $Document.Text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $text = ($text -replace "`r?`n", $separator) + $separator
    return Write-SetupText -Path $Document.Path -Text $text -OriginalText $Document.Text -Backup:$Backup
}

function Restore-McpJson {
    param([Parameter(Mandatory)] [string]$Path)

    if (-not (Test-Path -LiteralPath "$Path.bak")) { return $false }
    $backup = Read-SetupJson -Path "$Path.bak"
    $original = if (Test-Path -LiteralPath $Path) { [IO.File]::ReadAllText($Path) } else { $null }
    [void](Write-SetupText -Path $Path -Text $backup.Text -OriginalText $original)
    Remove-Item -LiteralPath "$Path.bak" -Force
    Write-Host "Restored $Path from backup" -ForegroundColor Green
    return $true
}

function Invoke-SetupGit {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string[]]$Arguments,
        [int[]]$AllowedExitCodes = @(0)
    )

    $ErrorActionPreference = 'Continue'
    $output = @(& git -C $RepoRoot @Arguments 2>&1)
    $code = $LASTEXITCODE
    if ($AllowedExitCodes -notcontains $code) {
        throw "git $($Arguments -join ' ') failed (exit $code): $(($output | Out-String).Trim())"
    }
    return [PSCustomObject]@{ ExitCode = $code; Lines = @($output | ForEach-Object { $_.ToString() }) }
}

function Get-GitWorkTreeRoot {
    param([Parameter(Mandatory)] [string]$RepoRoot)

    $ErrorActionPreference = 'Continue'
    $top = & git -C $RepoRoot rev-parse --show-toplevel 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $top) { return $null }
    return (Resolve-Path -LiteralPath ($top | Select-Object -First 1)).Path
}

function Find-GitMetadataRoot {
    param([Parameter(Mandatory)] [string]$StartPath)

    $current = Get-Item -LiteralPath $StartPath
    while ($current) {
        if (Test-Path -LiteralPath (Join-Path $current.FullName '.git')) { return $current.FullName }
        $current = $current.Parent
    }
    return $null
}

function Get-ResolvedGitPath {
    param([Parameter(Mandatory)] [string]$RepoRoot, [Parameter(Mandatory)] [string]$Name)

    $result = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('rev-parse', '--git-path', $Name)
    $path = $result.Lines[0]
    if (-not [IO.Path]::IsPathRooted($path)) { $path = Join-Path $RepoRoot $path }
    return [IO.Path]::GetFullPath($path)
}

function Test-IsTrackedFile {
    param([Parameter(Mandatory)] [string]$RepoRoot, [Parameter(Mandatory)] [string]$RelativePath)

    $result = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('ls-files', '--', $RelativePath)
    return $result.Lines.Count -gt 0
}

function Test-GitIgnoresPath {
    param([Parameter(Mandatory)] [string]$RepoRoot, [Parameter(Mandatory)] [string]$RelativePath)

    $result = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('check-ignore', '-q', '--', $RelativePath) -AllowedExitCodes @(0, 1)
    return $result.ExitCode -eq 0
}

function Add-TextLineNoBom {
    param([Parameter(Mandatory)] [string]$Path, [Parameter(Mandatory)] [string]$Line)

    $raw = if (Test-Path -LiteralPath $Path) { [IO.File]::ReadAllText($Path) } else { $null }
    if ($null -ne $raw -and @($raw -split "`r?`n") -ccontains $Line) { return $false }
    $separator = if ($raw -and $raw.Contains("`r`n")) { "`r`n" } else { "`n" }
    $prefix = if ($raw -and -not $raw.EndsWith("`n")) { $separator } else { '' }
    return Write-SetupText -Path $Path -Text ($raw + $prefix + $Line + $separator) -OriginalText $raw
}

function Remove-TextLineNoBom {
    param([Parameter(Mandatory)] [string]$Path, [Parameter(Mandatory)] [string]$Line)

    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $raw = [IO.File]::ReadAllText($Path)
    $separator = if ($raw.Contains("`r`n")) { "`r`n" } else { "`n" }
    $parts = @($raw -split "`r?`n")
    $kept = @($parts | Where-Object { $_ -cne $Line })
    if ($parts.Count -eq $kept.Count) { return $false }
    return Write-SetupText -Path $Path -Text ($kept -join $separator) -OriginalText $raw
}

function Add-DirectoryGitignoreOverride {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$RelativePath,
        [Parameter(Mandatory)] [string]$ExcludePath
    )

    $directory = (Split-Path -Parent $RelativePath) -replace '\\', '/'
    if ([string]::IsNullOrWhiteSpace($directory)) {
        throw "Repository rules prevent excluding $RelativePath. Setup will not change the root .gitignore."
    }
    $helperRelative = "$directory/.gitignore"
    if (Test-IsTrackedFile -RepoRoot $RepoRoot -RelativePath $helperRelative) {
        throw "Cannot override repository ignore rules: $helperRelative is tracked. No MCP config was written."
    }
    $helperPath = Join-Path $RepoRoot $helperRelative
    $original = if (Test-Path -LiteralPath $helperPath) { [IO.File]::ReadAllText($helperPath) } else { $null }
    $excludeRule = "/$helperRelative"
    $addedExclude = Add-TextLineNoBom -Path $ExcludePath -Line $excludeRule
    try {
        if (-not (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $helperRelative)) {
            throw "Local ignore helper $helperRelative would be visible to Git."
        }
        [void](Add-TextLineNoBom -Path $helperPath -Line ('/' + (Split-Path -Leaf $RelativePath)))
        if (-not (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $RelativePath)) {
            throw "Repository rules still prevent ignoring $RelativePath."
        }
    }
    catch {
        if ($null -eq $original) {
            if (Test-Path -LiteralPath $helperPath) { Remove-Item -LiteralPath $helperPath -Force }
        }
        else {
            $current = [IO.File]::ReadAllText($helperPath)
            [void](Write-SetupText -Path $helperPath -Text $original -OriginalText $current)
        }
        if ($addedExclude) { [void](Remove-TextLineNoBom -Path $ExcludePath -Line $excludeRule) }
        throw
    }
}

function Protect-McpPath {
    param(
        [Parameter(Mandatory)] [string]$RepoRoot,
        [Parameter(Mandatory)] [string]$RelativePath,
        [Parameter(Mandatory)] [string]$ExcludePath
    )

    if (Test-IsTrackedFile -RepoRoot $RepoRoot -RelativePath $RelativePath) {
        throw "$RelativePath is tracked by Git. Setup will not modify it or change the index."
    }
    [void](Add-TextLineNoBom -Path $ExcludePath -Line "/$RelativePath")
    if (-not (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $RelativePath)) {
        Add-DirectoryGitignoreOverride -RepoRoot $RepoRoot -RelativePath $RelativePath -ExcludePath $ExcludePath
    }
}

function Get-CopilotLauncherText {
    return (@(
        '@echo off'
        ('@rem ' + $Script:XrayMcpMarker)
        'setlocal DisableDelayedExpansion'
        'if not exist "%~dp0.github\mcp.json" ('
        '    echo Xray MCP configuration was not found. Run setup-xray.ps1 again. 1>&2'
        '    exit /b 1'
        ')'
        'copilot -C "%~dp0." --additional-mcp-config "@%~dp0.github\mcp.json" %*'
        'exit /b %errorlevel%'
    ) -join "`r`n") + "`r`n"
}

function Set-CopilotLauncher {
    param([string]$RepoRoot, [bool]$IsGitRepo, [string]$ExcludePath)

    $relative = 'copilot-xray.cmd'
    $path = Join-Path $RepoRoot $relative
    if ($IsGitRepo -and (Test-IsTrackedFile -RepoRoot $RepoRoot -RelativePath $relative)) {
        throw "$relative is tracked by Git. Setup will not modify it or change the index."
    }
    $original = if (Test-Path -LiteralPath $path) { [IO.File]::ReadAllText($path) } else { $null }
    if ($null -ne $original -and $original -notmatch ('(?m)^@rem ' + [Regex]::Escape($Script:XrayMcpMarker) + '\r?$')) {
        throw "Not overwriting an unrecognized launcher: $path."
    }
    if ($IsGitRepo) {
        Protect-McpPath -RepoRoot $RepoRoot -RelativePath $relative -ExcludePath $ExcludePath
    }
    $changed = Write-SetupText -Path $path -Text (Get-CopilotLauncherText) -OriginalText $original
    if ($IsGitRepo -and -not (Test-GitIgnoresPath -RepoRoot $RepoRoot -RelativePath $relative)) {
        throw "Git exclusion verification failed for $relative."
    }
    return $(if ($changed) { 'Configured' } else { 'Up to date' })
}

function Remove-CopilotLauncherExclude {
    param([string]$RepoRoot, [string]$ExcludePath)

    if (-not (Test-Path -LiteralPath $ExcludePath)) { return }
    $excludeText = [IO.File]::ReadAllText($ExcludePath)
    if (@($excludeText -split "`r?`n") -cnotcontains '/copilot-xray.cmd') { return }

    $worktrees = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('worktree', 'list', '--porcelain', '-z')
    foreach ($record in (($worktrees.Lines -join "`n") -split "`0")) {
        if ($record.StartsWith('worktree ')) {
            $worktreeRoot = $record.Substring(9)
            if (Test-Path -LiteralPath (Join-Path $worktreeRoot 'copilot-xray.cmd')) {
                Write-Verbose "Kept the shared launcher exclude rule for worktree: $worktreeRoot"
                return
            }
        }
    }
    [void](Remove-TextLineNoBom -Path $ExcludePath -Line '/copilot-xray.cmd')
}

function Remove-CopilotLauncher {
    param([string]$RepoRoot, [bool]$IsGitRepo, [string]$ExcludePath, [switch]$DryRun)

    $relative = 'copilot-xray.cmd'
    $path = Join-Path $RepoRoot $relative
    if ($IsGitRepo -and (Test-IsTrackedFile -RepoRoot $RepoRoot -RelativePath $relative)) {
        throw "Not removing a tracked launcher: $path."
    }
    if (-not (Test-Path -LiteralPath $path)) {
        if ($IsGitRepo -and -not $DryRun) {
            Remove-CopilotLauncherExclude -RepoRoot $RepoRoot -ExcludePath $ExcludePath
        }
        return 'Not present'
    }
    $original = [IO.File]::ReadAllText($path)
    if ($original -notmatch ('(?m)^@rem ' + [Regex]::Escape($Script:XrayMcpMarker) + '\r?$')) {
        Write-Warning "Not removing an unrecognized launcher: $path."
        return 'Not managed'
    }
    if ($DryRun) { return 'Would remove' }
    if ([IO.File]::ReadAllText($path) -cne $original) {
        throw "File changed during setup: $path. Retry rather than deleting concurrent changes."
    }
    Remove-Item -LiteralPath $path -Force -ErrorAction Stop
    if ($IsGitRepo) {
        Remove-CopilotLauncherExclude -RepoRoot $RepoRoot -ExcludePath $ExcludePath
    }
    return 'Removed'
}

function Get-LegacyMcpState {
    param([Parameter(Mandatory)] [string]$RepoRoot, [bool]$IsGitRepo)

    foreach ($definition in @(
        @{ Relative = '.mcp.json'; Container = 'mcpServers'; Filter = 'xray-mcp'; Client = 'Copilot CLI' },
        @{ Relative = '.vscode/mcp.json'; Container = 'servers'; Filter = 'xray-vscode-mcp'; Client = 'VS Code' }
    )) {
        $path = Join-Path $RepoRoot $definition.Relative
        $entry = $null
        $entryProperty = $null
        $parseError = $false
        if (Test-Path -LiteralPath $path) {
            try {
                $document = Read-SetupJson -Path $path
                $containerProperty = $document.Value.PSObject.Properties[$definition.Container]
                $container = if ($containerProperty) { $containerProperty.Value } elseif ($definition.Relative -eq '.mcp.json') { $document.Value } else { $null }
                if ($containerProperty -and $container -isnot [PSCustomObject]) {
                    throw "Expected a server object in legacy config: $path."
                }
                if ($container -is [PSCustomObject]) {
                    $entryProperty = $container.PSObject.Properties['xray']
                    if ($entryProperty) { $entry = $entryProperty.Value }
                }
            }
            catch {
                Write-Warning $_.Exception.Message
                $parseError = $true
            }
        }
        $filterPresent = $false
        $filterDirectory = $null
        $attributesPath = $null
        $skipWorktree = $false
        $assumeUnchanged = $false
        if ($IsGitRepo) {
            $common = (Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('rev-parse', '--git-common-dir')).Lines[0]
            if (-not [IO.Path]::IsPathRooted($common)) { $common = Join-Path $RepoRoot $common }
            $filterDirectory = Join-Path ([IO.Path]::GetFullPath($common)) $definition.Filter
            $attributesPath = Get-ResolvedGitPath -RepoRoot $RepoRoot -Name 'info/attributes'
            $configuration = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('config', '--local', '--get-regexp', ('^filter\.' + [Regex]::Escape($definition.Filter) + '\.')) -AllowedExitCodes @(0, 1)
            $filterPresent = $configuration.ExitCode -eq 0 -or (Test-Path -LiteralPath $filterDirectory)
            if (Test-Path -LiteralPath $attributesPath) {
                $attributePattern = '^\s*' + [Regex]::Escape($definition.Relative) + '\s+.*(?<!\S)filter=' + [Regex]::Escape($definition.Filter) + '(?=\s|$)'
                $filterPresent = $filterPresent -or [Regex]::IsMatch([IO.File]::ReadAllText($attributesPath), $attributePattern, [Text.RegularExpressions.RegexOptions]::Multiline)
            }
            $flags = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('ls-files', '-v', '--', $definition.Relative)
            if ($flags.Lines.Count -gt 0) {
                $skipWorktree = $flags.Lines[0] -cmatch '^[Ss] '
                $assumeUnchanged = $flags.Lines[0] -cmatch '^[a-z] '
            }
        }
        $marker = if ($entry -is [PSCustomObject]) { $entry.PSObject.Properties['_xrayMcpMarker'] } else { $null }
        $marked = $null -ne $marker -and $marker.Value -ceq $Script:XrayMcpMarker
        $found = $filterPresent -or $marked -or $skipWorktree -or $assumeUnchanged
        if ($definition.Relative -eq '.mcp.json') { $found = $found -or $null -ne $entryProperty -or $parseError }
        [PSCustomObject]@{
            Relative = $definition.Relative; Path = $path; Container = $definition.Container
            Filter = $definition.Filter; Client = $definition.Client; Found = $found
            ParseError = $parseError; EntryPresent = $null -ne $entryProperty; Marked = $marked
            FilterPresent = $filterPresent; FilterDirectory = $filterDirectory; AttributesPath = $attributesPath
            SkipWorktree = $skipWorktree; AssumeUnchanged = $assumeUnchanged
        }
    }
}

function Get-XrayStrippedText {
    param([string]$Text, [string]$ExpectedJson, [string]$Path, [switch]$AllowUnmarked)
    $marker = if ($AllowUnmarked) { '' } else { '"_xrayMcpMarker"\s*:\s*"managed-by-setup-xray\.ps1-do-not-edit"[^\r\n]*' }
    $pattern = '(?m)^[ \t]*"xray"\s*:\s*\{[^\r\n]*' + $marker + '\},?[ \t]*\r?\n'
    $stripped = [Regex]::Replace($Text, $pattern, '')
    if ($stripped -ceq $Text) { return $null }
    try { $parsed = ConvertFrom-SetupJson -Text $stripped -Path $Path }
    catch { return $null }
    if ((ConvertTo-SetupJson -Value $parsed -Path $Path -Compress) -ceq $ExpectedJson) { return $stripped }
    return $null
}

function Remove-XrayServerEntry {
    param([Parameter(Mandatory)] [string]$Path, [Parameter(Mandatory)] [string]$ContainerKey, [switch]$DryRun, [switch]$KeepEmptyFile,
        [string]$ExpectedText)

    if (-not (Test-Path -LiteralPath $Path)) {
        if ($PSBoundParameters.ContainsKey('ExpectedText')) { throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes." }
        return 'absent'
    }
    $document = Read-SetupJson -Path $Path
    if ($PSBoundParameters.ContainsKey('ExpectedText') -and $document.Text -cne $ExpectedText) {
        throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes."
    }
    $containerProperty = $document.Value.PSObject.Properties[$ContainerKey]
    $container = if ($containerProperty) { $containerProperty.Value } elseif ($ContainerKey -eq 'mcpServers') { $document.Value } else { $null }
    if ($null -eq $container) { return 'no-xray-entry' }
    if ($container -isnot [PSCustomObject]) { throw "Expected an object for $ContainerKey in $Path." }
    $entry = $container.PSObject.Properties['xray']
    if ($null -eq $entry) { return 'no-xray-entry' }
    if ($DryRun) { Write-Host "  [DryRun] Would remove xray from $Path"; return 'removed' }
    $container.PSObject.Properties.Remove('xray')
    if (-not $KeepEmptyFile -and @($container.PSObject.Properties).Count -eq 0 -and
        ($container -eq $document.Value -or @($document.Value.PSObject.Properties).Count -eq 1)) {
        if (-not (Test-Path -LiteralPath $Path) -or [IO.File]::ReadAllText($Path) -cne $document.Text) {
            throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes."
        }
        Backup-McpJson -Path $Path
        if (-not (Test-Path -LiteralPath $Path) -or [IO.File]::ReadAllText($Path) -cne $document.Text) {
            throw "File changed during setup: $Path. Retry rather than overwriting concurrent changes."
        }
        Remove-Item -LiteralPath $Path -Force
        return 'removed-and-deleted-empty-file'
    }

    $expected = ConvertTo-SetupJson -Value $document.Value -Path $Path -Compress
    $stripped = Get-XrayStrippedText -Text $document.Text -ExpectedJson $expected -Path $Path
    if ($null -ne $stripped) {
        [void](Write-SetupText -Path $Path -Text $stripped -OriginalText $document.Text -Backup)
        return 'removed'
    }
    [void](Write-SetupJson -Document $document -Backup)
    return 'removed'
}

function Assert-LegacyMcpMigration {
    param([string]$RepoRoot, $State)
    if ($State.ParseError) { throw "Cannot migrate unreadable legacy config: $($State.Path)." }
    if ($State.FilterPresent) {
        $worktrees = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('worktree', 'list', '--porcelain')
        if (@($worktrees.Lines | Where-Object { $_ -match '^worktree ' }).Count -gt 1) {
            throw "Legacy filter $($State.Filter) is shared by multiple worktrees. Coordinate cleanup in a single-worktree clone before retrying; shared metadata was not removed."
        }
        if ((Test-Path -LiteralPath $State.FilterDirectory) -and
            ((Get-Item -LiteralPath $State.FilterDirectory -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing to recursively remove a linked filter directory: $($State.FilterDirectory)."
        }
    }
}

function Remove-LegacyMcpState {
    param([Parameter(Mandatory)] [string]$RepoRoot, [Parameter(Mandatory)] $State, [switch]$DryRun, [switch]$KeepEntry)

    Assert-LegacyMcpMigration -RepoRoot $RepoRoot -State $State
    if (-not $KeepEntry) {
        [void](Remove-XrayServerEntry -Path $State.Path -ContainerKey $State.Container -KeepEmptyFile -DryRun:$DryRun)
    }
    if ($State.SkipWorktree -or $State.AssumeUnchanged) {
        if ($DryRun) { Write-Host "  [DryRun] Would clear legacy index flags on $($State.Relative)" }
        else {
            if ($State.SkipWorktree) { [void](Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('update-index', '--no-skip-worktree', '--', $State.Relative)) }
            if ($State.AssumeUnchanged) { [void](Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('update-index', '--no-assume-unchanged', '--', $State.Relative)) }
        }
    }
    if ($State.FilterPresent) {
        if ($DryRun) { Write-Host "  [DryRun] Would remove shared legacy filter $($State.Filter)"; return }
        $configuration = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('config', '--local', '--get-regexp', ('^filter\.' + [Regex]::Escape($State.Filter) + '\.')) -AllowedExitCodes @(0, 1)
        if ($configuration.ExitCode -eq 0) {
            [void](Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('config', '--local', '--remove-section', "filter.$($State.Filter)"))
        }
        if (Test-Path -LiteralPath $State.AttributesPath) {
            $raw = [IO.File]::ReadAllText($State.AttributesPath)
            $separator = if ($raw.Contains("`r`n")) { "`r`n" } else { "`n" }
            $kept = foreach ($line in ($raw -split "`r?`n")) {
                if ($line -match ('^\s*' + [Regex]::Escape($State.Relative) + '\s+')) {
                    $line = [Regex]::Replace($line, '(?<!\S)filter=' + [Regex]::Escape($State.Filter) + '(?=\s|$)', '')
                    if (@($line.Trim() -split '\s+').Count -eq 1) { continue }
                }
                $line
            }
            [void](Write-SetupText -Path $State.AttributesPath -Text ($kept -join $separator) -OriginalText $raw)
        }
        if (Test-Path -LiteralPath $State.FilterDirectory) {
            Remove-Item -LiteralPath $State.FilterDirectory -Recurse -Force
        }
    }
    Write-Host "Removed legacy Xray setup for $($State.Client)." -ForegroundColor Green
}

function Request-LegacyMigration {
    param([object[]]$States, [bool]$Interactive, [switch]$ExplicitConsent)

    if ($States.Count -eq 0) { return $false }
    foreach ($state in $States) {
        Write-Warning ("Legacy Xray state: {0}; filter={1}, skip-worktree={2}, assume-unchanged={3}. Cleanup may modify this config and shared Git metadata." -f
            $state.Path, $state.FilterPresent, $state.SkipWorktree, $state.AssumeUnchanged)
    }
    if ($ExplicitConsent) { return $true }
    if ($Interactive) {
        return Read-YesNo -Prompt 'Migrate the legacy Xray setup after configuring the new local files? Remove only the old Xray entry and its Git protection' -Default $false
    }
    Write-Warning 'Legacy cleanup was not authorized. Re-run with -MigrateLegacy to explicitly approve it; -Force does not grant this permission.'
    return $false
}

function Get-CopilotHome {
    $directory = if ([string]::IsNullOrWhiteSpace($env:COPILOT_HOME)) { Join-Path $HOME '.copilot' } else { $env:COPILOT_HOME }
    if (-not [IO.Path]::IsPathRooted($directory)) { $directory = Join-Path (Get-Location).ProviderPath $directory }
    return [IO.Path]::GetFullPath($directory)
}

function Get-UserVSCodeMcpPath {
    param([string]$RepoRoot, [string]$Override)
    if ($Override) {
        if (-not [IO.Path]::IsPathRooted($Override)) { $Override = Join-Path (Get-Location).ProviderPath $Override }
        return [IO.Path]::GetFullPath($Override)
    }
    $user = if ($env:VSCODE_PORTABLE) { Join-Path $env:VSCODE_PORTABLE 'user-data\User' } else { Join-Path $env:APPDATA 'Code\User' }
    $state = Read-SetupJson -Path (Join-Path $user 'globalStorage\storage.json')
    $profileId = $null
    if ($state.Value.profileAssociations.workspaces) {
        $workspaceUri = [Uri]($RepoRoot.TrimEnd('\', '/') + '\')
        $workspacePath = $workspaceUri.GetComponents([UriComponents]::Path, [UriFormat]::Unescaped).TrimEnd('/')
        foreach ($association in $state.Value.profileAssociations.workspaces.PSObject.Properties) {
            $uri = [Uri]$association.Name
            if ($uri.IsFile -and $uri.Host -ieq $workspaceUri.Host -and
                $uri.GetComponents([UriComponents]::Path, [UriFormat]::Unescaped).TrimEnd('/') -ieq $workspacePath) {
                $profileId = $association.Value
                break
            }
        }
    }
    if ($profileId -and $profileId -ne '__default__profile__') {
        $profile = @($state.Value.userDataProfiles | Where-Object { $_.location -eq $profileId })
        if ($profile.Count -ne 1) { throw "Cannot resolve VS Code profile $profileId. Supply -VSCodeMcpPath from 'MCP: Open User Configuration'." }
        if (-not $profile[0].useDefaultFlags.mcp) {
            $profiles = [IO.Path]::GetFullPath((Join-Path $user 'profiles')) + [IO.Path]::DirectorySeparatorChar
            $directory = [IO.Path]::GetFullPath((Join-Path $profiles $profileId))
            if (-not $directory.StartsWith($profiles, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid VS Code profile location.' }
            return Join-Path $directory 'mcp.json'
        }
    }
    return Join-Path $user 'mcp.json'
}

function Get-UserMcpTargets {
    param([string]$RepoRoot, [string]$VSCodeMcpPath, [bool]$IncludeVSCode)
    [PSCustomObject]@{ Relative = ''; Container = 'mcpServers'; Client = 'Copilot CLI/SDK'; Selected = $true; Path = (Join-Path (Get-CopilotHome) 'mcp-config.json'); Status = 'Not selected'; Document = $null }
    [PSCustomObject]@{ Relative = ''; Container = 'servers'; Client = 'VS Code'; Selected = $IncludeVSCode; Path = $(if ($IncludeVSCode) { Get-UserVSCodeMcpPath $RepoRoot $VSCodeMcpPath }); Status = 'Not selected'; Document = $null }
}

function Get-WorkspaceProfilePath {
    param([string]$Profiles, [string]$RepoRoot)
    $hash = [Security.Cryptography.SHA256]::Create()
    try {
        $key = [IO.Path]::GetFullPath($RepoRoot).TrimEnd('\', '/').ToLowerInvariant()
        $id = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($key)))).Replace('-', '').ToLowerInvariant()
        return Join-Path (Join-Path $Profiles $id) 'config.json'
    }
    finally { $hash.Dispose() }
}

function Test-ManagedUserMcpEntry {
    param($Entry, [string]$Binary, [string]$Profiles)
    return $Entry -and $Entry._xrayUserMcp -eq 1 -and $Entry.command -ieq $Binary -and
        @($Entry.args).Count -eq 3 -and $Entry.args[0] -ceq 'serve-workspace' -and
        $Entry.args[1] -ceq '--profiles' -and $Entry.args[2] -ieq $Profiles
}

function Get-WorkspaceProfileFiles {
    param([string]$Profiles)
    if (-not (Test-Path -LiteralPath $Profiles)) { return }
    Get-ChildItem -LiteralPath $Profiles -Directory | ForEach-Object {
        $file = Join-Path $_.FullName 'config.json'
        if (Test-Path -LiteralPath $file -PathType Leaf) { $file }
    }
}

function Get-UserMcpMigrationConfigs {
    param([string]$RepoRoot, [string]$Binary, [bool]$IsGitRepo, [bool]$Consent)
    $legacyStates = @(Get-LegacyMcpState -RepoRoot $RepoRoot -IsGitRepo $IsGitRepo)
    foreach ($definition in @(
        @{ Relative = '.mcp.json'; Container = 'mcpServers' },
        @{ Relative = '.github/mcp.json'; Container = 'mcpServers' },
        @{ Relative = '.vscode/mcp.json'; Container = 'servers' }
    )) {
        $path = Join-Path $RepoRoot $definition.Relative
        $document = Read-SetupJson -Path $path
        $container = $document.Value.PSObject.Properties[$definition.Container]
        $entry = if ($container -and $container.Value -is [PSCustomObject]) { $container.Value.PSObject.Properties['xray'] } else { $null }
        if (-not $container -and $definition.Relative -eq '.mcp.json') { $entry = $document.Value.PSObject.Properties['xray'] }
        $legacy = $legacyStates | Where-Object { $_.Relative -eq $definition.Relative -and $_.FilterPresent }
        if (-not $entry -and -not $legacy) { continue }
        if (-not $Consent) { throw "Workspace Xray entry or legacy filter in $path can override personal MCP. Re-run -UserMcp -MigrateLegacy to remove eligible local entries and filter metadata." }
        if ($legacy) {
            Assert-LegacyMcpMigration -RepoRoot $RepoRoot -State $legacy
            if ($legacy.SkipWorktree -or $legacy.AssumeUnchanged) {
                throw "Legacy index flags require separate cleanup before -UserMcp: $path. Setup will not change them."
            }
        }
        $index = $null
        if ($legacy -and $IsGitRepo -and (Test-IsTrackedFile $RepoRoot $definition.Relative)) {
            $blob = Invoke-SetupGit -RepoRoot $RepoRoot -Arguments @('show', ":$($definition.Relative)")
            $index = ConvertFrom-SetupJson -Text ($blob.Lines -join "`n") -Path $path
            $indexContainer = $index.PSObject.Properties[$definition.Container]
            $indexServers = if ($indexContainer) { $indexContainer.Value } else { $index }
            if ($indexServers -isnot [PSCustomObject] -or $indexServers.PSObject.Properties['xray']) {
                throw "Not migrating an Xray entry present in the Git index: $path."
            }
        }
        if (-not $entry) {
            [PSCustomObject]@{
                Path = $path; Relative = $definition.Relative; Container = $definition.Container
                LegacyState = $legacy; OriginalText = $document.Text; CleanText = $null; Binary = $Binary; MetadataOnly = $true
            }
            continue
        }
        if ($IsGitRepo -and (Test-IsTrackedFile $RepoRoot "$($definition.Relative).bak")) {
            throw "Not migrating a tracked workspace config or backup: $path."
        }
        if ($IsGitRepo -and (Test-IsTrackedFile $RepoRoot $definition.Relative)) {
            if ($definition.Relative -ne '.mcp.json' -or -not $legacy -or
                (-not $legacy.Marked -and $entry.Value.command -ine $Binary)) {
                throw "Not migrating a tracked workspace config or backup: $path."
            }
            $attributePattern = '(?m)^\s*\.mcp\.json\s+.*(?<!\S)filter=xray-mcp(?=\s|$)'
            if (-not (Test-Path -LiteralPath $legacy.AttributesPath) -or
                -not [Regex]::IsMatch([IO.File]::ReadAllText($legacy.AttributesPath), $attributePattern)) {
                throw "Not migrating a legacy filter without an owned local info/attributes rule: $path."
            }
            $expected = ConvertTo-SetupJson -Value $index -Path $path -Compress
            $clean = Get-XrayStrippedText -Text $document.Text -ExpectedJson $expected -Path $path -AllowUnmarked:($entry.Value.command -ieq $Binary)
            if ($null -eq $clean) {
                throw "Not migrating $path because removing the local Xray line does not reproduce the Git index JSON. Other local changes were preserved."
            }
            [PSCustomObject]@{
                Path = $path; Relative = $definition.Relative; Container = $definition.Container
                LegacyState = $legacy; OriginalText = $document.Text; CleanText = $clean; Binary = $Binary; MetadataOnly = $false
            }
            continue
        }
        if ($entry.Value.command -ine $Binary) { throw "Not migrating an Xray entry owned by another executable: $path." }
        [PSCustomObject]@{
            Path = $path; Relative = $definition.Relative; Container = $definition.Container
            LegacyState = $legacy; OriginalText = $document.Text; CleanText = $null; Binary = $Binary; MetadataOnly = $false
        }
    }
}

function Remove-UserMcpWorkspaceEntry {
    param([string]$RepoRoot, [bool]$IsGitRepo, [string]$ExcludePath, $Migration)
    $current = @(Get-UserMcpMigrationConfigs -RepoRoot $RepoRoot -Binary $Migration.Binary -IsGitRepo $IsGitRepo -Consent $true |
        Where-Object Path -eq $Migration.Path)
    if ($current.Count -ne 1 -or $current[0].OriginalText -cne $Migration.OriginalText -or
        $current[0].CleanText -cne $Migration.CleanText -or $current[0].MetadataOnly -ne $Migration.MetadataOnly) {
        throw "Workspace config changed during setup: $($Migration.Path). Retry rather than overwriting it."
    }
    if ($Migration.MetadataOnly) {
        Remove-LegacyMcpState -RepoRoot $RepoRoot -State $current[0].LegacyState -KeepEntry
        return
    }
    if ($null -ne $Migration.CleanText) {
        Protect-McpPath -RepoRoot $RepoRoot -RelativePath "$($Migration.Relative).bak" -ExcludePath $ExcludePath
        [void](Write-SetupText -Path $Migration.Path -Text $Migration.CleanText -OriginalText $Migration.OriginalText -Backup)
        Remove-LegacyMcpState -RepoRoot $RepoRoot -State $current[0].LegacyState -KeepEntry
        return
    }
    if ($IsGitRepo) {
        Protect-McpPath -RepoRoot $RepoRoot -RelativePath $Migration.Relative -ExcludePath $ExcludePath
        Protect-McpPath -RepoRoot $RepoRoot -RelativePath "$($Migration.Relative).bak" -ExcludePath $ExcludePath
    }
    [void](Remove-XrayServerEntry -Path $Migration.Path -ContainerKey $Migration.Container -ExpectedText $Migration.OriginalText)
    if ($current[0].LegacyState) {
        Remove-LegacyMcpState -RepoRoot $RepoRoot -State $current[0].LegacyState -KeepEntry
    }
}

function Remove-UserMcpWorkspace {
    param([string]$ProfilePath, [string]$Profiles, [string]$Binary, [object[]]$Targets,
        [switch]$DryRun, [switch]$KeepBinary, [switch]$KeepBackups)
    foreach ($target in $Targets) {
        $document = Read-SetupJson -Path $target.Path
        $container = $document.Value.PSObject.Properties[$target.Container]
        if ($container -and $container.Value -isnot [PSCustomObject]) {
            throw "Expected an object for $($target.Container) in $($target.Path)."
        }
    }
    $hadProfile = Test-Path -LiteralPath $ProfilePath -PathType Leaf
    if ($hadProfile) {
        if ($DryRun) { Write-Host "  [DryRun] Would remove workspace profile $ProfilePath" }
        else { Remove-Item -LiteralPath $ProfilePath -Force }
    }
    if (-not $KeepBackups -and (Test-Path -LiteralPath "$ProfilePath.bak")) {
        if ($DryRun) { Write-Host "  [DryRun] Would remove $ProfilePath.bak" }
        else { Remove-Item -LiteralPath "$ProfilePath.bak" -Force }
    }
    $remaining = @(Get-WorkspaceProfileFiles $Profiles | Where-Object { $_ -ine $ProfilePath })
    if ($remaining.Count -gt 0) {
        Write-Host 'Other workspace profiles remain; keeping shared registrations and binary.'
        return
    }
    foreach ($target in $Targets) {
        $document = Read-SetupJson -Path $target.Path
        $container = $document.Value.PSObject.Properties[$target.Container]
        $entry = if ($container -and $container.Value -is [PSCustomObject]) { $container.Value.PSObject.Properties['xray'] } else { $null }
        if ($entry) {
            if (Test-ManagedUserMcpEntry $entry.Value $Binary $Profiles) {
                $status = Remove-XrayServerEntry -Path $target.Path -ContainerKey $target.Container -ExpectedText $document.Text -KeepEmptyFile -DryRun:$DryRun
                Write-Host "$($target.Client): $status"
            }
            else { Write-Warning "Preserving an unmanaged personal Xray entry: $($target.Path)" }
        }
    }
    Write-Host 'Keeping the shared executable; other VS Code profiles or clients may still reference it.'
}

function Set-CopilotWorkspaceTrust {
    param([Parameter(Mandatory)] [string]$RepoRoot, [bool]$Interactive)

    try {
        $copilotHome = Get-CopilotHome
        $document = Read-SetupJson -Path (Join-Path ([IO.Path]::GetFullPath($copilotHome)) 'config.json')
        $property = $document.Value.PSObject.Properties['trustedFolders']
        $folders = @()
        if ($property) {
            if ($property.Value -isnot [array]) { throw 'Copilot trustedFolders must be an array. Configuration was not overwritten.' }
            $folders = @($property.Value)
        }
        $normalizedRepo = [IO.Path]::GetFullPath($RepoRoot).TrimEnd('\', '/')
        foreach ($folder in $folders) {
            if ($folder -isnot [string] -or [string]::IsNullOrWhiteSpace($folder)) {
                throw 'Copilot trustedFolders contains an invalid path. Configuration was not overwritten.'
            }
            $normalized = [IO.Path]::GetFullPath($folder).TrimEnd('\', '/')
            if ([string]::Equals($normalizedRepo, $normalized, [StringComparison]::OrdinalIgnoreCase) -or
                $normalizedRepo.StartsWith($normalized + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
                return 'Yes (already trusted)'
            }
        }
        if (-not $Interactive) {
            Write-Warning 'Copilot CLI trust was not granted in non-interactive mode. Trust the repository in Copilot CLI to enable workspace MCP discovery.'
            return 'No (non-interactive)'
        }
        Write-Host "`nRepository: $RepoRoot"
        if (-not (Read-YesNo -Prompt 'Add repository to GitHub Copilot CLI trusted folders?' -Default $true)) {
            return 'No (declined)'
        }
        $document.Value | Add-Member -NotePropertyName 'trustedFolders' -NotePropertyValue @($folders + $RepoRoot) -Force
        [void](Write-SetupJson -Document $document -Backup)
        return 'Yes'
    }
    catch {
        Write-Warning ("Could not update Copilot CLI trust: {0}" -f $_.Exception.Message)
        return 'No (error)'
    }
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
    $RepoPath = Read-Host 'Enter the path to the target repository (e.g. C:\Repos\MyProject)'
}
try {
    $RepoPath = (Resolve-Path -LiteralPath $RepoPath -ErrorAction Stop).Path
    if (-not [IO.Path]::IsPathRooted($InstallDir)) { $InstallDir = Join-Path (Get-Location).ProviderPath $InstallDir }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
}
catch {
    Write-Error "Cannot resolve repository or installation path: $($_.Exception.Message)"
    exit 1
}

$gitCommand = Get-Command git -CommandType Application -ErrorAction SilentlyContinue
$gitMetadataRoot = Find-GitMetadataRoot -StartPath $RepoPath
$gitWorkTreeRoot = if ($gitCommand) { Get-GitWorkTreeRoot -RepoRoot $RepoPath } else { $null }
$isGitRepo = $null -ne $gitWorkTreeRoot
if ($gitMetadataRoot -and -not $isGitRepo) {
    Write-Error "Git metadata exists at $gitMetadataRoot, but Git could not inspect it. Check Git availability and safe.directory. No files were changed."
    exit 1
}
if ($isGitRepo) { $RepoPath = $gitWorkTreeRoot }
if ($GitVisibility) { Write-Warning '-GitVisibility is deprecated. All generated configs are now local; neither Visible nor Hidden installs a Git filter.' }
if ($EnableRoo) { Write-Warning '-EnableRoo is disabled. Use -EnableVSCode or -EnableCopilotCli instead.' }
if ($Restore -and $Uninstall) { Write-Error '-Restore and -Uninstall cannot be used together.'; exit 1 }
if ($DryRun -and -not $Uninstall) { Write-Error '-DryRun is supported only with -Uninstall.'; exit 1 }
$nonInteractiveHost = @([Environment]::GetCommandLineArgs() | Where-Object { $_ -match '^-NonI' }).Count -gt 0
$interactive = -not $Force -and -not $nonInteractiveHost -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
$selectAll = -not ($EnableVSCode -or $EnableCopilotCli)
$targets = @(
    [PSCustomObject]@{ Relative = '.github/mcp.json'; Container = 'mcpServers'; Client = 'Copilot CLI'; Selected = $selectAll -or $EnableCopilotCli; Path = (Join-Path $RepoPath '.github\mcp.json'); Status = 'Not selected'; Document = $null },
    [PSCustomObject]@{ Relative = '.vscode/mcp.json'; Container = 'servers'; Client = 'VS Code'; Selected = $selectAll -or $EnableVSCode; Path = (Join-Path $RepoPath '.vscode\mcp.json'); Status = 'Not selected'; Document = $null }
)
if ($VSCodeMcpPath -and -not $UserMcp) { Write-Error '-VSCodeMcpPath requires -UserMcp.'; exit 1 }
if (-not $UserMcp -and -not $Uninstall) {
    foreach ($target in @(Get-UserMcpTargets -RepoRoot $RepoPath -IncludeVSCode ($selectAll -or $EnableVSCode) | Where-Object Selected)) {
        $document = Read-SetupJson -Path $target.Path
        $container = $document.Value.PSObject.Properties[$target.Container]
        if ($container -and $container.Value -isnot [PSCustomObject]) {
            throw "Expected an object for $($target.Container) in $($target.Path)."
        }
        if ($container -and $container.Value.PSObject.Properties['xray']) {
            throw "Personal Xray registration already exists in $($target.Path). Refusing workspace setup or restore because it can create an overriding duplicate. Use -UserMcp; add -MigrateLegacy to explicitly migrate eligible workspace entries. No workspace files were changed."
        }
    }
}
if ($UserMcp) {
    if (-not [IO.Path]::IsPathRooted($InstallDir)) { $InstallDir = Join-Path (Get-Location).ProviderPath $InstallDir }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
    $profilesPath = Join-Path $InstallDir 'workspaces'
    $profilePath = Get-WorkspaceProfilePath $profilesPath $RepoPath
    $targets = @(Get-UserMcpTargets -RepoRoot $RepoPath -VSCodeMcpPath $VSCodeMcpPath -IncludeVSCode ($selectAll -or $EnableVSCode))
    $repoPrefix = $RepoPath.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    foreach ($path in @($profilePath) + @($targets | Where-Object { $_.Selected } | ForEach-Object { $_.Path })) {
        if ([IO.Path]::GetFullPath($path).StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            Write-Error "-UserMcp files must be outside the repository: $path"; exit 1
        }
    }
    if ($Restore) {
        if (-not (Restore-McpJson -Path $profilePath)) { Write-Error "No workspace profile backup found: $profilePath.bak"; exit 1 }
        exit 0
    }
    if ($Uninstall) {
        Remove-UserMcpWorkspace -ProfilePath $profilePath -Profiles $profilesPath -Binary (Join-Path $InstallDir 'xray.exe') -Targets @($targets | Where-Object { $_.Selected }) -DryRun:$DryRun -KeepBinary:$KeepBinary -KeepBackups:$KeepBackups
        Write-Host $(if ($DryRun) { 'Dry-run complete. No files were changed.' } else { 'User MCP uninstall complete.' })
        exit 0
    }
    $userMigrations = @(Get-UserMcpMigrationConfigs -RepoRoot $RepoPath -Binary (Join-Path $InstallDir 'xray.exe') -IsGitRepo $isGitRepo -Consent $MigrateLegacy)
}
$excludePath = if ($isGitRepo) { Get-ResolvedGitPath -RepoRoot $RepoPath -Name 'info/exclude' } else { $null }

if ($Restore) {
    $restored = $false
    $restoreFailed = $false
    foreach ($target in $targets) {
        if ($isGitRepo -and ((Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath $target.Relative) -or
            (Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath "$($target.Relative).bak"))) {
            Write-Warning "Not restoring a tracked config or backup: $($target.Path)"
            continue
        }
        if (Test-Path -LiteralPath "$($target.Path).bak") {
            if ($isGitRepo) { Protect-McpPath -RepoRoot $RepoPath -RelativePath $target.Relative -ExcludePath $excludePath }
            if (Restore-McpJson -Path $target.Path) {
                $restored = $true
                if ($target.Client -eq 'Copilot CLI') {
                    $document = Read-SetupJson -Path $target.Path
                    $container = $document.Value.PSObject.Properties['mcpServers']
                    $hasXray = $container -and $container.Value -is [PSCustomObject] -and $container.Value.PSObject.Properties['xray']
                    if (-not $hasXray -or -not (Test-Path -LiteralPath (Join-Path $RepoPath '.mcp.json') -PathType Leaf)) {
                        try {
                            $status = Remove-CopilotLauncher -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath
                            Write-Host "CLI launcher: $status"
                        }
                        catch {
                            Write-Warning ("Could not clean up Copilot CLI launcher during restore: {0}" -f $_.Exception.Message)
                            $restoreFailed = $true
                        }
                    }
                }
            }
        }
    }
    if (-not $restored) { Write-Error 'No eligible .bak files found. Nothing was restored.'; exit 1 }
    if ($restoreFailed) { Write-Warning 'Restore incomplete. Eligible client configs were restored; see launcher cleanup warnings above.'; exit 1 }
    exit 0
}

$legacyStates = if ($UserMcp) { @() } else { @(Get-LegacyMcpState -RepoRoot $RepoPath -IsGitRepo $isGitRepo | Where-Object {
    $_.Found -and ($Uninstall -or ($_.Client -eq 'VS Code' -and $targets[1].Selected) -or ($_.Client -eq 'Copilot CLI' -and $targets[0].Selected))
}) }
$migrate = Request-LegacyMigration -States $legacyStates -Interactive $interactive -ExplicitConsent:$MigrateLegacy

if ($Uninstall) {
    $failed = $false
    $legacyFailed = @()
    foreach ($state in $legacyStates) {
        if (-not $migrate) { $failed = $true; continue }
        try {
            if ($isGitRepo -and -not $DryRun) {
                Protect-McpPath -RepoRoot $RepoPath -RelativePath "$($state.Relative).bak" -ExcludePath $excludePath
            }
            Remove-LegacyMcpState -RepoRoot $RepoPath -State $state -DryRun:$DryRun
        }
        catch { Write-Warning $_.Exception.Message; $failed = $true; $legacyFailed += $state.Relative }
    }
    foreach ($target in $targets) {
        try {
            if ($isGitRepo -and -not $UserMcp) {
                foreach ($relative in @($target.Relative, "$($target.Relative).bak")) {
                    if (Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath $relative) {
                        throw "Not uninstalling tracked config or backup: $relative."
                    }
                }
                if (-not $DryRun -and (Test-Path -LiteralPath $target.Path)) {
                    Protect-McpPath -RepoRoot $RepoPath -RelativePath "$($target.Relative).bak" -ExcludePath $excludePath
                }
            }
            if (@($legacyStates | Where-Object { $_.Relative -eq $target.Relative }).Count -gt 0 -and -not $migrate) {
                throw "Legacy config left untouched: $($target.Path)."
            }
            if ($legacyFailed -contains $target.Relative) {
                throw "Legacy cleanup failed; leaving config untouched: $($target.Path)."
            }
            $status = Remove-XrayServerEntry -Path $target.Path -ContainerKey $target.Container -DryRun:$DryRun
            Write-Host "$($target.Client): $status"
            if ($target.Client -eq 'Copilot CLI') {
                $status = Remove-CopilotLauncher -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath -DryRun:$DryRun
                Write-Host "CLI launcher: $status"
            }
            if (-not $KeepBackups -and (Test-Path -LiteralPath "$($target.Path).bak")) {
                if ($DryRun) { Write-Host "  [DryRun] Would delete $($target.Path).bak" }
                else { Remove-Item -LiteralPath "$($target.Path).bak" -Force }
            }
            if ($isGitRepo -and -not $DryRun -and -not (Test-Path -LiteralPath $target.Path)) {
                [void](Remove-TextLineNoBom -Path $excludePath -Line "/$($target.Relative)")
            }
        }
        catch { Write-Warning $_.Exception.Message; $failed = $true }
    }
    $binary = Join-Path $InstallDir 'xray.exe'
    if (-not $KeepBinary -and (Test-Path -LiteralPath $binary)) {
        if (@(Get-WorkspaceProfileFiles (Join-Path $InstallDir 'workspaces')).Count -gt 0) {
            Write-Host 'Keeping the shared executable because personal workspace profiles still reference it.'
        }
        elseif ($DryRun) { Write-Host "  [DryRun] Would delete $binary" }
        elseif (-not $failed) {
            try { Remove-Item -LiteralPath $binary -Force -ErrorAction Stop }
            catch { Write-Warning "Could not delete $binary (it may be running)."; $failed = $true }
        }
    }
    Write-Host $(if ($DryRun) { 'Dry-run complete. No files were changed.' } elseif ($failed) { 'Uninstall incomplete. See warnings above.' } else { 'Uninstall complete.' })
    if ($failed) { exit 1 }
    exit 0
}

foreach ($target in ($targets | Where-Object { $_.Selected })) {
    try {
        if (-not $migrate -and @($legacyStates | Where-Object { $_.Relative -eq $target.Relative }).Count -gt 0) {
            throw "Legacy config left untouched without migration consent: $($target.Path)."
        }
        if ($isGitRepo -and -not $UserMcp) {
            foreach ($relative in @($target.Relative, "$($target.Relative).bak")) {
                if (Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath $relative) {
                    throw "$relative is tracked by Git. Setup will not modify it or change the index."
                }
            }
        }
        $target.Document = Read-SetupJson -Path $target.Path
        $container = $target.Document.Value.PSObject.Properties[$target.Container]
        if ($container -and $container.Value -isnot [PSCustomObject]) {
            throw "Expected an object for $($target.Container) in $($target.Path)."
        }
        if (-not $container) {
            $target.Document.Value | Add-Member -NotePropertyName $target.Container -NotePropertyValue ([PSCustomObject]@{})
        }
        if ($UserMcp) {
            $entry = $target.Document.Value.($target.Container).PSObject.Properties['xray']
            if ($entry -and -not (Test-ManagedUserMcpEntry $entry.Value (Join-Path $InstallDir 'xray.exe') $profilesPath) -and -not $Force) {
                if (-not $interactive -or -not (Read-YesNo -Prompt "Replace the existing personal Xray entry in $($target.Path)?" -Default $false)) {
                    throw "Personal Xray entry was not replaced: $($target.Path). Use -Force to approve replacement."
                }
            }
        }
        if ($isGitRepo -and -not $UserMcp) {
            Protect-McpPath -RepoRoot $RepoPath -RelativePath $target.Relative -ExcludePath $excludePath
            Protect-McpPath -RepoRoot $RepoPath -RelativePath "$($target.Relative).bak" -ExcludePath $excludePath
        }
        $target.Status = 'Ready'
    }
    catch { Write-Warning $_.Exception.Message; $target.Status = 'Skipped (error)' }
}
if ($UserMcp -and @($targets | Where-Object { $_.Selected -and $_.Status -ne 'Ready' }).Count -gt 0) {
    Write-Error 'Personal MCP preflight failed. No workspace profile or personal MCP config was written.'; exit 1
}
if (@($targets | Where-Object { $_.Status -eq 'Ready' }).Count -eq 0) {
    Write-Error 'No eligible MCP configuration could be prepared. No MCP config was written.'
    exit 1
}
if (-not $isGitRepo) { Write-Warning 'This folder is not a Git worktree. MCP files will be written without Git exclude protection.' }
Write-Host "`n=== Setting up xray MCP for: $RepoPath ===" -ForegroundColor Cyan

$xrayPath = Join-Path $InstallDir 'xray.exe'
if (-not $SkipDownload) {
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $needsDownload = $true
    if (Test-Path $xrayPath) {
        Write-Host "xray.exe already exists at $xrayPath" -ForegroundColor Yellow
        $needsDownload = Read-YesNo -Prompt 'Download the latest release and overwrite?' -Default $false -ForceYes:$Force
        if ($needsDownload -and $Force) {
            Write-Host 'Force enabled; overwriting existing xray.exe.' -ForegroundColor Yellow
        }
    }

    if ($needsDownload) {
        Write-Host 'Downloading the latest xray release...' -ForegroundColor Cyan
        $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("xray-download-{0}" -f [Guid]::NewGuid().ToString('N'))
        if (Test-Path $tempDir) {
            Remove-Item $tempDir -Recurse -Force
        }
        New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

        $downloaded = Join-Path $tempDir 'xray.exe'
        try {
            Save-XrayReleaseAsset -GithubRepo $GithubRepo -Destination $downloaded
        }
        catch {
            $message = $_.Exception.Message
            Remove-Item $tempDir -Recurse -Force -ErrorAction SilentlyContinue
            Write-Error $message
            exit 1
        }

        # Windows refuses to overwrite a running .exe. If xray.exe is in use
        # (typically an MCP server spawned by another VS Code / Roo instance),
        # terminate the running processes first - either silently when the user
        # opts in via -KillRunning / -Force, or after an interactive prompt.
        try {
            Move-Item -LiteralPath $downloaded -Destination $xrayPath -Force -ErrorAction Stop
        }
        catch [System.IO.IOException], [System.UnauthorizedAccessException] {
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

            try {
                Install-XrayBinaryWithRetry -Downloaded $downloaded -Destination $xrayPath
            }
            catch {
                Remove-Item $tempDir -Recurse -Force -ErrorAction SilentlyContinue
                Write-Error $_.Exception.Message
                exit 1
            }
            Write-Host 'Note: MCP hosts that were using xray will need to restart it.' -ForegroundColor Yellow
        }
        Remove-Item $tempDir -Recurse -Force
        Write-Host "Installed the latest xray release to $xrayPath" -ForegroundColor Green
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

    $autoSelection = Get-AutoSelectedExtensions `
        -ExtensionCounts $extCounts `
        -Threshold $threshold `
        -AlwaysIncludeIfDetected $AlwaysIncludeIfDetected

    $thresholdSelected = Normalize-ExtensionList -Value ($autoSelection.ThresholdSelected -join ',')
    $structuralSelected = Normalize-ExtensionList -Value ($autoSelection.StructuralSelected -join ',')
    $suggested = Normalize-ExtensionList -Value ($autoSelection.All -join ',')
    $thresholdDisplay = if ($thresholdSelected) { $thresholdSelected } else { '(none)' }
    $structuralDisplay = if ($structuralSelected) { $structuralSelected } else { '(none)' }

    Write-Host "`nThreshold-selected extensions (>= $([int]$threshold) files): " -NoNewline
    Write-Host $thresholdDisplay -ForegroundColor Green
    Write-Host 'Structural exceptions (detected below threshold): ' -NoNewline
    Write-Host $structuralDisplay -ForegroundColor Green
    Write-Host 'Suggested extensions: ' -NoNewline
    Write-Host $suggested -ForegroundColor Green

    if ($Force) {
        $selectedExts = $suggested
        Write-Host 'Force enabled; accepting suggested extensions.' -ForegroundColor Yellow
    }
    else {
        $selectedExts = Read-ExtensionSelection -Suggested $suggested
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
    '--debug-log'
)
if ($UserMcp) {
    $capability = @(& $xrayPath serve-workspace --help 2>&1)
    if ($LASTEXITCODE -ne 0) { Write-Error "The installed Xray does not support serve-workspace. Upgrade the executable before using -UserMcp. $($capability -join ' ')"; exit 1 }
    $profile = Read-SetupJson -Path $profilePath
    $profile.Value = [PSCustomObject][ordered]@{
        version = 1; workspaceRoot = $RepoPath
        args = @('--ext', $selectedExts, '--watch', '--definitions', '--debug-log')
    }
    [void](Write-SetupJson -Document $profile -Backup)
    $xrayArgs = @('serve-workspace', '--profiles', $profilesPath)
}

$definition = @{ type = 'stdio'; command = $xrayPath; args = $xrayArgs }
foreach ($target in ($targets | Where-Object { $_.Status -eq 'Ready' })) {
    try {
        $entry = if ($target.Container -eq 'servers') {
            [PSCustomObject][ordered]@{ '//' = $Script:XrayVisibleWarning; type = $definition.type; command = $definition.command; args = $definition.args }
        }
        else {
            [PSCustomObject][ordered]@{ type = $definition.type; command = $definition.command; args = $definition.args; tools = @('*') }
        }
        if ($UserMcp) {
            $entry | Add-Member -NotePropertyName '_xrayUserMcp' -NotePropertyValue 1
        }
        $target.Document.Value.($target.Container) | Add-Member -NotePropertyName 'xray' -NotePropertyValue $entry -Force
        $changed = Write-SetupJson -Document $target.Document -Backup
        $target.Status = if ($changed) { 'Configured' } else { 'Up to date' }
        if ($isGitRepo -and -not $UserMcp -and -not (Test-GitIgnoresPath -RepoRoot $RepoPath -RelativePath $target.Relative)) {
            throw "Git exclusion verification failed for $($target.Relative)."
        }
    }
    catch { Write-Warning $_.Exception.Message; $target.Status = 'Failed' }
}
if ($UserMcp) {
    $incomplete = @($targets | Where-Object { $_.Selected -and $_.Status -notin @('Configured', 'Up to date') }).Count -gt 0
    if (-not $incomplete) {
        foreach ($migration in $userMigrations) {
            Remove-UserMcpWorkspaceEntry -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath -Migration $migration
        }
        $launcherStatus = Remove-CopilotLauncher -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath
        Write-Host "Legacy CLI launcher: $launcherStatus"
    }
    Write-Host "`nUser MCP setup $(if ($incomplete) { 'incomplete' } else { 'completed' })."
    Write-Host "Workspace profile: $profilePath"
    foreach ($target in $targets) { Write-Host "$($target.Client): $($target.Path) ($($target.Status))" }
    Write-Host 'Restart MCP connections in VS Code and CLI. No CLI wrapper is required.'
    Write-Host 'Verify actual xray_info and xray_definitions calls in each client.'
    if ($incomplete) { exit 1 }
    exit 0
}

$legacyRemaining = $false
foreach ($state in $legacyStates) {
    $target = $targets | Where-Object { $_.Client -eq $state.Client } | Select-Object -First 1
    if (-not $migrate -or $target.Status -notin @('Configured', 'Up to date')) {
        $legacyRemaining = $true
        continue
    }
    try {
        if ($isGitRepo -and $state.Relative -eq '.mcp.json') {
            Protect-McpPath -RepoRoot $RepoPath -RelativePath '.mcp.json.bak' -ExcludePath $excludePath
        }
        Remove-LegacyMcpState -RepoRoot $RepoPath -State $state -KeepEntry:($state.Relative -eq '.vscode/mcp.json')
    }
    catch { Write-Warning $_.Exception.Message; $legacyRemaining = $true }
}

$launcherPath = Join-Path $RepoPath 'copilot-xray.cmd'
$launcherStatus = 'Not requested'
$rootMcpPresent = Test-Path -LiteralPath (Join-Path $RepoPath '.mcp.json') -PathType Leaf
$trust = 'Not requested'
if ($targets[0].Status -in @('Configured', 'Up to date')) {
    try {
        if ($rootMcpPresent) {
            $launcherStatus = Set-CopilotLauncher -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath
            Write-Warning 'A root .mcp.json takes precedence over .github/mcp.json. Use .\copilot-xray.cmd to load Xray alongside the root servers. The root config was not changed.'
        }
        else {
            $launcherStatus = Remove-CopilotLauncher -RepoRoot $RepoPath -IsGitRepo $isGitRepo -ExcludePath $excludePath
            if ($launcherStatus -eq 'Not present') { $launcherStatus = 'Not needed' }
        }
    }
    catch { Write-Warning $_.Exception.Message; $launcherStatus = 'Failed' }
    $trust = Set-CopilotWorkspaceTrust -RepoRoot $RepoPath -Interactive $interactive
}
$incomplete = $legacyRemaining -or $launcherStatus -eq 'Failed' -or @($targets | Where-Object { $_.Selected -and $_.Status -notin @('Configured', 'Up to date') }).Count -gt 0
Write-Host "`nXray MCP setup $(if ($incomplete) { 'incomplete' } else { 'completed' })." -ForegroundColor Cyan
Write-Host "Repository:    $RepoPath"
Write-Host "Xray binary:   $xrayPath"
foreach ($target in $targets) { Write-Host ("{0}: {1} ({2})" -f $target.Client, $target.Path, $target.Status) }
Write-Host ("Git excluded:  {0}" -f $(if (-not $isGitRepo) { 'No (not a Git repository)' } elseif ($incomplete) { 'Only successfully configured files; see warnings' } else { 'Yes' }))
Write-Host "CLI trusted:   $trust"
if ($targets[0].Selected) { Write-Host "CLI launcher:  $launcherPath ($launcherStatus)" }
if ($legacyRemaining) { Write-Warning 'Legacy Xray state remains. A root definition or legacy filter may override the new CLI config. No successful migration is claimed.' }
if ($targets[0].Status -in @('Configured', 'Up to date')) {
    if ($rootMcpPresent) {
        if ($launcherStatus -in @('Configured', 'Up to date')) {
            Write-Host "`nStart from this repository: .\copilot-xray.cmd"
            Write-Host 'Verify inside that Copilot session: /mcp list; /mcp show xray'
        }
        else {
            Write-Host "`nLoad Xray explicitly from this repository: copilot --additional-mcp-config '@.\.github\mcp.json'"
        }
    }
    else { Write-Host "`nVerify from this repository: copilot mcp list; copilot mcp get xray" }
}
if ($incomplete) { exit 1 }
exit 0
