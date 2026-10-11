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

.PARAMETER EnableRoo
    DISABLED. Kept as a no-op switch for backward compat with caller scripts.
    Roo configuration is not written, restored, or removed.

.PARAMETER EnableCopilotCli
    Select GitHub Copilot CLI setup in .github/mcp.json. Without either client
    switch, both clients are selected.

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
    Restore .vscode/mcp.json and .github/mcp.json from the .bak files created on the
    previous setup run, then exit. Skips download and extension detection.
    Use this to undo the most recent setup-xray run.
    Tracked target files are not restored. Ordinary setup creates a backup only
    when changing an existing configuration; an unchanged rerun makes no writes.
    If no eligible .bak exists when -Restore is invoked,
    the script exits with an error.

.PARAMETER Uninstall
    Fully remove xray from the target repository:
      - Strip the 'xray' server entry from local .vscode/mcp.json and
        .github/mcp.json (other servers preserved; tracked targets skipped).
      - If a config file would become empty AND was created by this script
        (no upstream content), delete it.
      - Legacy root entries, filters and index flags require migration consent.
      - Delete .bak files (unless -KeepBackups).
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

    $parameters = @{ InputObject = $Text; ErrorAction = 'Stop' }
    if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) {
        $parameters.DateKind = 'String'
    }
    $useTokenReader = $PSVersionTable.PSVersion.Major -ge 7 -and -not $parameters.ContainsKey('DateKind')
    try {
        if ($useTokenReader) {
            # PowerShell 7.0-7.4 otherwise converts date-looking strings to dates.
            $reader = [Newtonsoft.Json.JsonTextReader]::new([IO.StringReader]::new($Text))
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

function Remove-XrayServerEntry {
    param([Parameter(Mandatory)] [string]$Path, [Parameter(Mandatory)] [string]$ContainerKey, [switch]$DryRun, [switch]$KeepEmptyFile)

    if (-not (Test-Path -LiteralPath $Path)) { return 'absent' }
    $document = Read-SetupJson -Path $Path
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

    # Old filters injected a complete first member on one line. Removing that
    # line preserves the upstream bytes; validate it before using the shortcut.
    $pattern = '(?m)^[ \t]*"xray"\s*:\s*\{[^\r\n]*"_xrayMcpMarker"\s*:\s*"managed-by-setup-xray\.ps1-do-not-edit"[^\r\n]*\},?[ \t]*\r?\n'
    $stripped = [Regex]::Replace($document.Text, $pattern, '')
    $expected = ConvertTo-SetupJson -Value $document.Value -Path $Path -Compress
    if ($stripped -cne $document.Text) {
        try { $parsed = ConvertFrom-SetupJson -Text $stripped -Path $Path }
        catch { $parsed = $null }
        if ($null -ne $parsed -and (ConvertTo-SetupJson -Value $parsed -Path $Path -Compress) -ceq $expected) {
            [void](Write-SetupText -Path $Path -Text $stripped -OriginalText $document.Text -Backup)
            return 'removed'
        }
    }
    [void](Write-SetupJson -Document $document -Backup)
    return 'removed'
}

function Remove-LegacyMcpState {
    param([Parameter(Mandatory)] [string]$RepoRoot, [Parameter(Mandatory)] $State, [switch]$DryRun, [switch]$KeepEntry)

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

function Set-CopilotWorkspaceTrust {
    param([Parameter(Mandatory)] [string]$RepoRoot, [bool]$Interactive)

    try {
        $copilotHome = if ([string]::IsNullOrWhiteSpace($env:COPILOT_HOME)) { Join-Path $HOME '.copilot' } else { $env:COPILOT_HOME }
        if (-not [IO.Path]::IsPathRooted($copilotHome)) { $copilotHome = Join-Path (Get-Location).ProviderPath $copilotHome }
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
if ($DryRun -and -not $Uninstall) { Write-Error '-DryRun is supported only with -Uninstall.'; exit 1 }
$nonInteractiveHost = @([Environment]::GetCommandLineArgs() | Where-Object { $_ -match '^-NonI' }).Count -gt 0
$interactive = -not $Force -and -not $nonInteractiveHost -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
$selectAll = -not ($EnableVSCode -or $EnableCopilotCli)
$targets = @(
    [PSCustomObject]@{ Relative = '.github/mcp.json'; Container = 'mcpServers'; Client = 'Copilot CLI'; Selected = $selectAll -or $EnableCopilotCli; Path = (Join-Path $RepoPath '.github\mcp.json'); Status = 'Not selected'; Document = $null },
    [PSCustomObject]@{ Relative = '.vscode/mcp.json'; Container = 'servers'; Client = 'VS Code'; Selected = $selectAll -or $EnableVSCode; Path = (Join-Path $RepoPath '.vscode\mcp.json'); Status = 'Not selected'; Document = $null }
)
$excludePath = if ($isGitRepo) { Get-ResolvedGitPath -RepoRoot $RepoPath -Name 'info/exclude' } else { $null }

if ($Restore) {
    $restored = $false
    foreach ($target in $targets) {
        if ($isGitRepo -and ((Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath $target.Relative) -or
            (Test-IsTrackedFile -RepoRoot $RepoPath -RelativePath "$($target.Relative).bak"))) {
            Write-Warning "Not restoring a tracked config or backup: $($target.Path)"
            continue
        }
        if (Test-Path -LiteralPath "$($target.Path).bak") {
            if ($isGitRepo) { Protect-McpPath -RepoRoot $RepoPath -RelativePath $target.Relative -ExcludePath $excludePath }
            if (Restore-McpJson -Path $target.Path) { $restored = $true }
        }
    }
    if (-not $restored) { Write-Error 'No eligible .bak files found. Nothing was restored.'; exit 1 }
    exit 0
}

$legacyStates = @(Get-LegacyMcpState -RepoRoot $RepoPath -IsGitRepo $isGitRepo | Where-Object {
    $_.Found -and ($Uninstall -or ($_.Client -eq 'VS Code' -and $targets[1].Selected) -or ($_.Client -eq 'Copilot CLI' -and $targets[0].Selected))
})
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
            if ($isGitRepo) {
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
        if ($DryRun) { Write-Host "  [DryRun] Would delete $binary" }
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
        if ($isGitRepo) {
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
        if ($isGitRepo) {
            Protect-McpPath -RepoRoot $RepoPath -RelativePath $target.Relative -ExcludePath $excludePath
            Protect-McpPath -RepoRoot $RepoPath -RelativePath "$($target.Relative).bak" -ExcludePath $excludePath
        }
        $target.Status = 'Ready'
    }
    catch { Write-Warning $_.Exception.Message; $target.Status = 'Skipped (error)' }
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

$definition = @{ type = 'stdio'; command = $xrayPath; args = $xrayArgs }
foreach ($target in ($targets | Where-Object { $_.Status -eq 'Ready' })) {
    try {
        $entry = if ($target.Container -eq 'servers') {
            [PSCustomObject][ordered]@{ '//' = $Script:XrayVisibleWarning; type = $definition.type; command = $definition.command; args = $definition.args }
        }
        else {
            [PSCustomObject][ordered]@{ type = $definition.type; command = $definition.command; args = $definition.args; tools = @('*') }
        }
        $target.Document.Value.($target.Container) | Add-Member -NotePropertyName 'xray' -NotePropertyValue $entry -Force
        $changed = Write-SetupJson -Document $target.Document -Backup
        $target.Status = if ($changed) { 'Configured' } else { 'Up to date' }
        if ($isGitRepo -and -not (Test-GitIgnoresPath -RepoRoot $RepoPath -RelativePath $target.Relative)) {
            throw "Git exclusion verification failed for $($target.Relative)."
        }
    }
    catch { Write-Warning $_.Exception.Message; $target.Status = 'Failed' }
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

$trust = 'Not requested'
if ($targets[0].Status -in @('Configured', 'Up to date')) {
    $trust = Set-CopilotWorkspaceTrust -RepoRoot $RepoPath -Interactive $interactive
}
$incomplete = $legacyRemaining -or @($targets | Where-Object { $_.Selected -and $_.Status -notin @('Configured', 'Up to date') }).Count -gt 0
Write-Host "`nXray MCP setup $(if ($incomplete) { 'incomplete' } else { 'completed' })." -ForegroundColor Cyan
Write-Host "Repository:    $RepoPath"
Write-Host "Xray binary:   $xrayPath"
foreach ($target in $targets) { Write-Host ("{0}: {1} ({2})" -f $target.Client, $target.Path, $target.Status) }
Write-Host ("Git excluded:  {0}" -f $(if (-not $isGitRepo) { 'No (not a Git repository)' } elseif ($incomplete) { 'Only successfully configured files; see warnings' } else { 'Yes' }))
Write-Host "CLI trusted:   $trust"
if ($legacyRemaining) { Write-Warning 'Legacy Xray state remains. A root definition or legacy filter may override the new CLI config. No successful migration is claimed.' }
if ($targets[0].Status -in @('Configured', 'Up to date')) {
    Write-Host "`nVerify from this repository: copilot mcp list; copilot mcp get xray"
}
if ($incomplete) { exit 1 }
exit 0
