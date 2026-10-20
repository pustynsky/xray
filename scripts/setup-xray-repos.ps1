<#
.SYNOPSIS
    Update and migrate Xray across Git checkouts below one directory.
.DESCRIPTION
    Only previously configured repositories are selected by default.
    -IncludeNewRepos also creates profiles for other discovered checkouts.
    Displays a plan before invoking the adjacent setup-xray.ps1 installer.
    Conflicts are skipped and reported with a nonzero exit code.
.PARAMETER Root
    Directory containing repositories or grouping directories. A checkout at Root
    is included. Nested repositories inside a checkout are not searched.
.PARAMETER IncludeNewRepos
    Also configure checkouts without Xray. Extensions are detected by the installer
    unless -Extensions supplies a list for new repositories only.
.PARAMETER DryRun
    Discover and preflight only. No writes, downloads, extension scans, or stops.
.PARAMETER Force
    Approve the displayed plan without prompting. Does not authorize process stops,
    overwriting conflicting settings, or changing client enablement.
.PARAMETER KillRunning
    Explicitly allow the installer to stop processes using the shared executable
    when replacing it. All workspaces using that executable may be affected.
.PARAMETER SkipDownload
    Reuse the installed executable rather than downloading the latest release.
.PARAMETER InstallDir
    Shared installation directory, defaulting to LOCALAPPDATA\xray.
.PARAMETER VSCodeMcpPath
    Optional explicit personal VS Code MCP configuration path.
.PARAMETER GithubRepo
    GitHub repository from which the existing installer downloads Xray.
.EXAMPLE
    .\setup-xray-repos.ps1 -Root C:\Repos -DryRun
.EXAMPLE
    .\setup-xray-repos.ps1 -Root C:\Repos -IncludeNewRepos -Force -KillRunning
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Root,
    [switch]$IncludeNewRepos,
    [switch]$DryRun,
    [switch]$Force,
    [switch]$KillRunning,
    [switch]$SkipDownload,
    [string]$Extensions,
    [string]$InstallDir = "$env:LOCALAPPDATA\xray",
    [string]$VSCodeMcpPath,
    [string]$GithubRepo = 'pustynsky/xray'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

function Get-BulkRepositories {
    param([string]$Directory)

    $skip = @('.git', '.vs', '.venv', 'venv', 'node_modules', 'target', 'bin', 'obj', 'dist', 'out', 'vendor', '.next')
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($Directory)
    while ($pending.Count -gt 0) {
        $path = $pending.Pop()
        $item = Get-Item -LiteralPath $path -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            Write-Warning "Skipping reparse point: $path"
            continue
        }
        if (Test-Path -LiteralPath (Join-Path $path '.git')) {
            $path
            continue
        }
        foreach ($child in @(Get-ChildItem -LiteralPath $path -Directory -Force)) {
            if ($skip -notcontains $child.Name) { $pending.Push($child.FullName) }
        }
    }
}

function Assert-BulkMetadataPaths {
    param([string]$RepoPath)

    foreach ($relative in @('.mcp.json', '.github', '.github\mcp.json', '.vscode', '.vscode\mcp.json')) {
        $path = Join-Path $RepoPath $relative
        if ((Test-Path -LiteralPath $path) -and
            ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing redirected MCP metadata: $path."
        }
    }
}

function Invoke-BulkPreflight {
    param([string]$RepoPath)

    Assert-BulkMetadataPaths $RepoPath
    $parameters = @{
        RepoPath = $RepoPath; InstallDir = $InstallDir; UserMcp = $true
        PlanOnly = $true; PreserveSettings = $true; MigrateLegacy = $true
    }
    if ($VSCodeMcpPath) { $parameters.VSCodeMcpPath = $VSCodeMcpPath }
    $global:LASTEXITCODE = 0
    $result = @(& $installer @parameters 6>$null)
    if ($LASTEXITCODE -ne 0 -or $result.Count -ne 1 -or $null -eq $result[0].Existing) {
        throw "Installer preflight failed for $RepoPath (exit $LASTEXITCODE)."
    }
    $discoveredPath = [IO.Path]::GetFullPath($RepoPath).TrimEnd('\', '/')
    $resolvedPath = [IO.Path]::GetFullPath($result[0].RepoPath).TrimEnd('\', '/')
    if (-not [string]::Equals($discoveredPath, $resolvedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Discovered path $RepoPath resolves to a different Git checkout: $($result[0].RepoPath). Refusing to configure it."
    }
    return $result[0]
}

try {
    $rootItem = Get-Item -LiteralPath $Root -Force
    if (-not $rootItem.PSIsContainer -or ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw '-Root must be a real directory, not a file or reparse point.'
    }
    $Root = $rootItem.FullName
    $InstallDir = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDir))
    $installer = Join-Path $PSScriptRoot 'setup-xray.ps1'
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) { throw "Adjacent installer not found: $installer." }
    if (-not (Get-Command git -CommandType Application -ErrorAction SilentlyContinue)) { throw 'Git is required for repository discovery and migration.' }

    $plan = @(
        foreach ($repo in @(Get-BulkRepositories $Root | Sort-Object -Unique)) {
            try {
                $preflight = Invoke-BulkPreflight $repo
                $action = if (-not $preflight.Existing -and -not $IncludeNewRepos) { 'Skip' }
                    elseif ($preflight.HasLegacy) { 'Migrate' }
                    elseif ($preflight.Existing) { 'Update' } else { 'Create' }
                [PSCustomObject]@{
                    Repository = $repo; Source = $preflight.Source; Action = $action
                    HasLegacy = $preflight.HasLegacy
                    Extensions = if ($preflight.Existing) { $preflight.Extensions } else { $Extensions }
                    Arguments = $preflight.Arguments; Result = 'Planned'
                    Detail = if ($action -eq 'Skip') { 'No existing Xray setup; use -IncludeNewRepos.' } else { '' }
                }
            }
            catch {
                [PSCustomObject]@{
                    Repository = $repo; Source = 'Unknown'; Action = 'Conflict'
                    HasLegacy = $false
                    Extensions = ''; Arguments = @(); Result = 'Skipped'; Detail = $_.Exception.Message
                }
            }
        }
    )
    $plan | Format-Table Action, Source, @{ Label = 'Repository'; Expression = { $_.Repository }; Width = 60 },
        @{ Label = 'Extensions'; Expression = { $_.Extensions }; Width = 24 },
        @{ Label = 'Detail'; Expression = { $_.Detail }; Width = 80 } -Wrap |
        Out-String -Width 220 | Write-Host
    $selected = @($plan | Where-Object { $_.Action -notin @('Skip', 'Conflict') })
    $failed = @($plan | Where-Object { $_.Action -eq 'Conflict' }).Count -gt 0
    Write-Host "Shared executable: $(Join-Path $InstallDir 'xray.exe')"
    Write-Host $(if ($SkipDownload) { 'Binary: reuse installed executable.' } else {
        'Binary: download latest release once. This affects ALL workspaces using this executable, including those outside Root.'
    })
    Write-Host 'MCP client/UI enablement is not changed. Process-stop consent is separate from -Force.'
    if ($DryRun -or $selected.Count -eq 0) {
        Write-Host "Plan only: $($selected.Count) selected; $(@($plan | Where-Object { $_.Action -eq 'Conflict' }).Count) conflicts. No changes made."
        if ($failed) { exit 1 }
        exit 0
    }
    if (-not $Force) {
        $nonInteractive = @([Environment]::GetCommandLineArgs() | Where-Object { $_ -match '^-NonI' }).Count -gt 0
        if ($nonInteractive -or [Console]::IsInputRedirected) { throw 'Review with -DryRun, then use -Force to approve this plan in a noninteractive host.' }
        if ((Read-Host 'Apply this plan, including eligible legacy migration? (y/N)') -ine 'y') {
            Write-Host 'Cancelled. No changes made.'
            exit 0
        }
    }
    $binaryReady = [bool]$SkipDownload
    foreach ($row in $selected) {
        $preparingBinary = $false
        try {
            $current = Invoke-BulkPreflight $row.Repository
            if ($current.Source -cne $row.Source -or $current.HasLegacy -ne $row.HasLegacy -or
                ($current.Arguments | ConvertTo-Json -Compress) -cne ($row.Arguments | ConvertTo-Json -Compress)) {
                throw 'Repository settings changed after planning. Retry instead of applying a stale plan.'
            }
            $preparingBinary = -not $binaryReady
            if (-not $binaryReady -and -not $KillRunning) {
                $binary = Join-Path $InstallDir 'xray.exe'
                $running = @(Get-Process -Name xray -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $binary })
                if ($running.Count -gt 0) { throw 'Shared Xray is running. Close its connections, use -SkipDownload, or explicitly approve -KillRunning.' }
            }
            $parameters = @{
                RepoPath = $row.Repository; InstallDir = $InstallDir; UserMcp = $true
                PreserveSettings = $true; MigrateLegacy = $true; Force = $true
                SkipDownload = $binaryReady; KillRunning = [bool]$KillRunning; GithubRepo = $GithubRepo
                BinaryPrepared = [ref]$binaryReady
            }
            if ($row.Extensions) { $parameters.Extensions = $row.Extensions }
            if ($VSCodeMcpPath) { $parameters.VSCodeMcpPath = $VSCodeMcpPath }
            $global:LASTEXITCODE = 0
            & $installer @parameters
            if ($LASTEXITCODE -ne 0) { throw "Installer failed (exit $LASTEXITCODE)." }
            $binaryReady = $true
            $row.Result = 'Configured'
        }
        catch {
            $row.Result = 'Failed'
            $row.Detail = $_.Exception.Message
            $failed = $true
            Write-Warning "$($row.Repository): $($row.Detail)"
            if ($preparingBinary -and -not $binaryReady) {
                Write-Warning 'Binary preparation was not completed; stopping the batch without retrying the download for other repositories.'
                foreach ($remaining in @($selected | Where-Object { $_.Result -eq 'Planned' })) {
                    $remaining.Result = 'Not run'
                    $remaining.Detail = 'Batch stopped before binary preparation completed.'
                }
                break
            }
        }
    }
    $plan | Format-Table Action, Result, @{ Label = 'Repository'; Expression = { $_.Repository }; Width = 60 },
        @{ Label = 'Detail'; Expression = { $_.Detail }; Width = 100 } -Wrap |
        Out-String -Width 220 | Write-Host
    if ($failed) { exit 1 }
    exit 0
}
catch {
    Write-Error $_.Exception.Message
    exit 1
}
