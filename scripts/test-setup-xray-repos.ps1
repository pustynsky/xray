<#
.SYNOPSIS
    Isolated bulk setup tests. No live configuration, network, or process stops.
#>
param([string]$XrayBinary = (Join-Path $PSScriptRoot '..\target\release\xray.exe'))

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$powerShellExe = (Get-Process -Id $PID).Path
$bulkScript = Join-Path $PSScriptRoot 'setup-xray-repos.ps1'
$setupScript = Join-Path $PSScriptRoot 'setup-xray.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('xray-bulk-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$script:passes = 0
$script:failures = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
    $script:passes++
}

function Test-Case {
    param([string]$Name, [scriptblock]$Body)
    try { & $Body; Write-Host "PASS $Name" }
    catch { $script:failures++; Write-Host "FAIL $Name`: $($_.Exception.Message)" -ForegroundColor Red }
}

function Write-TestFile {
    param([string]$Path, [string]$Text)
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function New-BulkFixture {
    $directory = Join-Path $testRoot ([Guid]::NewGuid().ToString('N'))
    $fixture = [PSCustomObject]@{
        Directory = $directory; Root = (Join-Path $directory 'repos with spaces')
        Install = (Join-Path $directory 'installation'); Home = (Join-Path $directory 'home')
        AppData = (Join-Path $directory 'appdata'); LocalAppData = (Join-Path $directory 'localappdata')
        VSCode = (Join-Path $directory 'vscode\mcp.json')
    }
    New-Item -ItemType Directory -Path $fixture.Root, $fixture.Install, $fixture.Home -Force | Out-Null
    Copy-Item -LiteralPath $XrayBinary -Destination (Join-Path $fixture.Install 'xray.exe')
    Write-TestFile (Join-Path $directory 'empty.gitconfig') ''
    return $fixture
}

function New-TestRepo {
    param($Fixture, [string]$Name)
    $repo = Join-Path $Fixture.Root $Name
    New-Item -ItemType Directory -Path $repo -Force | Out-Null
    & git -c init.defaultBranch=main init --quiet $repo
    if ($LASTEXITCODE -ne 0) { throw 'Fixture git init failed.' }
    Write-TestFile (Join-Path $repo 'sample.rs') 'pub fn bulk_marker() {}'
    return $repo
}

function Get-TestProfilePath {
    param($Fixture, [string]$Repo, [switch]$Legacy)
    $normalized = [IO.Path]::GetFullPath($Repo).TrimEnd('\', '/').ToLowerInvariant()
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($normalized))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
    $label = ($normalized -replace '[^a-z0-9]+', '_').Trim('_')
    if ($label.Length -gt 64) { $label = $label.Substring(0, 64).TrimEnd('_') }
    $id = if ($Legacy) { $hash } else { $label + '-' + $hash.Substring(0, 12) }
    return Join-Path $Fixture.Install "workspaces\$id\config.json"
}

function Set-TestProfile {
    param($Fixture, [string]$Repo, [string[]]$Arguments = @('--ext', 'rs', '--watch', '--definitions'), [switch]$Legacy)
    $path = Get-TestProfilePath $Fixture $Repo -Legacy:$Legacy
    Write-TestFile $path (@{ version = 1; workspaceRoot = $Repo; args = $Arguments } | ConvertTo-Json -Depth 10)
    return $path
}

function Set-TestLegacy {
    param($Fixture, [string]$Repo, [string]$Relative = '.vscode\mcp.json', [string[]]$Options = @('--ext', 'rs', '--watch'))
    $container = if ($Relative -eq '.vscode\mcp.json') { 'servers' } else { 'mcpServers' }
    $entry = @{ type = 'stdio'; command = (Join-Path $Fixture.Install 'xray.exe'); args = @('serve', '--dir', $Repo) + $Options }
    Write-TestFile (Join-Path $Repo $Relative) (@{ $container = @{ xray = $entry; other = @{ command = 'other.exe' } } } | ConvertTo-Json -Depth 10)
}

function Invoke-BulkFixture {
    param($Fixture, [hashtable]$Parameters = @{}, [string]$Script = $bulkScript,
        [switch]$CaptureProgress, [string[]]$ForbiddenDiscoveryPaths = @())
    # An invalid owner/repo keeps a regressed download guard offline.
    $arguments = @{ Root = $Fixture.Root; InstallDir = $Fixture.Install; VSCodeMcpPath = $Fixture.VSCode; SkipDownload = $true; Force = $true; GithubRepo = 'offline-tests-disabled' }
    foreach ($key in $Parameters.Keys) { $arguments[$key] = $Parameters[$key] }
    $payload = @{ Script = $Script; Arguments = $arguments; Fixture = $Fixture
        CaptureProgress = [bool]$CaptureProgress; ForbiddenDiscoveryPaths = $ForbiddenDiscoveryPaths } | ConvertTo-Json -Depth 12 -Compress
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $code = @'
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$payload = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
foreach ($entry in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'GIT_*' })) { Remove-Item -LiteralPath ('Env:' + $entry.Name) }
$env:GIT_CONFIG_GLOBAL = Join-Path $payload.Fixture.Directory 'empty.gitconfig'
$env:GIT_CONFIG_NOSYSTEM = '1'
$env:COPILOT_HOME = $payload.Fixture.Home
$env:APPDATA = $payload.Fixture.AppData
$env:LOCALAPPDATA = $payload.Fixture.LocalAppData
$env:VSCODE_PORTABLE = ''
if ($payload.CaptureProgress) {
    $global:bulkProgressRecords = [Collections.Generic.List[object]]::new()
    function Write-Progress {
        param([int]$Id, [string]$Activity, [string]$Status, [string]$CurrentOperation,
            [int]$PercentComplete = -1, [switch]$Completed)
        $global:bulkProgressRecords.Add([PSCustomObject]@{
            Id = $Id; Activity = $Activity; Status = $Status; CurrentOperation = $CurrentOperation
            PercentComplete = $PercentComplete; Completed = [bool]$Completed
        })
    }
}
if (@($payload.ForbiddenDiscoveryPaths).Count -gt 0) {
    $bulkGetChildItem = Get-Command Get-ChildItem -CommandType Cmdlet
    function Get-ChildItem {
        [CmdletBinding()]
        param([string]$LiteralPath, [switch]$Directory, [switch]$Force)
        foreach ($forbidden in $payload.ForbiddenDiscoveryPaths) {
            if ($LiteralPath -and ($LiteralPath -ieq $forbidden -or
                $LiteralPath.StartsWith($forbidden + '\', [StringComparison]::OrdinalIgnoreCase))) {
                throw "Test attempted to enumerate an excluded tree: $LiteralPath"
            }
        }
        & $bulkGetChildItem @PSBoundParameters
    }
}
$parameters = @{}
foreach ($property in $payload.Arguments.PSObject.Properties) { $parameters[$property.Name] = $property.Value }
try {
    & $payload.Script @parameters
    $bulkExitCode = $LASTEXITCODE
}
finally {
    if ($payload.CaptureProgress) {
        Write-Output ('[BULK_PROGRESS]' + (ConvertTo-Json -InputObject ($global:bulkProgressRecords.ToArray()) -Depth 6 -Compress))
    }
}
exit $bulkExitCode
'@
    $code = $code.Replace('__PAYLOAD__', $encoded)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $powerShellExe
    $start.Arguments = '-NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' +
        [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(120000)) {
            $process.Kill()
            $process.WaitForExit()
            throw 'Isolated bulk setup invocation timed out.'
        }
        $text = $stdout.Result + $stderr.Result
        $progressMatch = [regex]::Match($text, '(?m)^\[BULK_PROGRESS\](.+)\r?$')
        $progress = if ($progressMatch.Success) { @($progressMatch.Groups[1].Value | ConvertFrom-Json) } else { @() }
        return [PSCustomObject]@{ ExitCode = $process.ExitCode; Text = $text; Progress = @($progress) }
    }
    finally { $process.Dispose() }
}

function Get-FixtureSnapshot {
    param($Fixture)
    return (@(Get-ChildItem -LiteralPath $Fixture.Directory -File -Force -Recurse | Sort-Object FullName | ForEach-Object {
        $_.FullName + ':' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }) -join "`n")
}

try {
    if (-not (Test-Path -LiteralPath $XrayBinary -PathType Leaf)) { throw "Build Xray first or supply -XrayBinary: $XrayBinary" }

    Test-Case 'existing-only setup preserves custom arguments and ignores new repositories' {
        $f = New-BulkFixture
        $existing = New-TestRepo $f 'existing'
        $new = New-TestRepo $f 'group\new'
        $args = @('--ext', 'rs,md', '--definitions', '--metrics', '--debounce-ms', '123', '--max-response-kb', '96', '--log-level', 'debug')
        [void](Set-TestProfile $f $existing $args)
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $profile = Get-Content -LiteralPath (Get-TestProfilePath $f $existing) -Raw | ConvertFrom-Json
        foreach ($arg in $args | Where-Object { $_ -ne 'rs,md' }) { Assert-True ($profile.args -ccontains $arg) "Lost argument $arg" }
        Assert-True ($profile.args -cnotcontains '--watch') 'Watch was silently enabled.'
        Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $new))) 'A new repo was installed by default.'
        Assert-True ($result.Text -match 'Skip') 'Skipped repository absent from plan.'
        $cli = Get-Content -LiteralPath (Join-Path $f.Home 'mcp-config.json') -Raw | ConvertFrom-Json
        Assert-True ($cli.mcpServers.xray.command -eq (Join-Path $f.Install 'xray.exe')) 'Wrong shared binary registration.'
    }

    Test-Case 'dry-run has zero filesystem side effects even with download and stop flags' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'legacy'
        Set-TestLegacy $f $repo
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ DryRun = $true; SkipDownload = $false; KillRunning = $true; IncludeNewRepos = $true }
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'DryRun changed fixture files.'
        Assert-True ($result.Text -match 'Migrate') 'Legacy migration missing from plan.'
        Assert-True ($result.Text -match 'ALL workspaces') 'Global binary effect not disclosed.'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'mcp-config.json'))) 'DryRun registered CLI.'
    }

    Test-Case 'dry-run reports discovery and preflight progress without filesystem changes' {
        $f = New-BulkFixture
        $first = New-TestRepo $f 'a'
        $second = New-TestRepo $f 'group\b'
        [void](Set-TestProfile $f $first)
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ DryRun = $true } -CaptureProgress
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Progress reporting changed dry-run filesystem state.'
        $discovery = @($result.Progress | Where-Object { $_.Activity -eq 'Discovering Git repositories' -and -not $_.Completed })
        Assert-True ($discovery.Count -ge 2 -and $discovery[0].CurrentOperation -eq $f.Root -and $discovery[0].PercentComplete -eq -1) 'Discovery did not report the current path before traversal.'
        Assert-True ($discovery[-1].Status -eq 'Discovery complete: 4 directories visited; 2 repository candidates found') 'Discovery counters do not match the fixture.'
        $checks = @($result.Progress | Where-Object { $_.Activity -eq 'Checking Xray configuration' -and -not $_.Completed })
        Assert-True ($checks.Count -eq 2 -and $checks[0].CurrentOperation -eq $first -and $checks[1].CurrentOperation -eq $second) 'Preflight progress did not identify each repository.'
        Assert-True ($checks[0].Status -eq 'Checking repository 1/2' -and $checks[0].PercentComplete -eq 0 -and $checks[1].Status -eq 'Checking repository 2/2' -and $checks[1].PercentComplete -eq 50) 'Preflight progress counts or percentages are wrong.'
        Assert-True (@($result.Progress | Where-Object Activity -eq 'Applying Xray setup').Count -eq 0) 'DryRun reported an application phase.'
        Assert-True ($result.Progress[-1].Completed) 'Progress was not cleared on dry-run exit.'
        Assert-True ($result.Text -match 'Discovering Git repositories under' -and $result.Text -match 'Checking repository \[1/2\]') 'Plain-text progress is missing without progress-bar rendering.'
    }

    Test-Case 'application progress identifies each selected repository' {
        $f = New-BulkFixture
        foreach ($name in @('a', 'b')) { [void](Set-TestProfile $f (New-TestRepo $f $name)) }
        $result = Invoke-BulkFixture $f -CaptureProgress
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $apply = @($result.Progress | Where-Object { $_.Activity -eq 'Applying Xray setup' -and -not $_.Completed })
        Assert-True ($apply.Count -eq 2 -and $apply[0].CurrentOperation -eq (Join-Path $f.Root 'a') -and $apply[1].CurrentOperation -eq (Join-Path $f.Root 'b')) 'Application progress is missing or points to another repository.'
        Assert-True ($apply[0].Status -eq 'Applying repository 1/2' -and $apply[0].PercentComplete -eq 0 -and $apply[1].Status -eq 'Applying repository 2/2' -and $apply[1].PercentComplete -eq 50) 'Application counts or percentages are wrong.'
        $applyRecords = @($result.Progress | Where-Object Activity -eq 'Applying Xray setup')
        Assert-True (@($applyRecords | Where-Object Completed).Count -eq 1 -and $applyRecords[-1].Completed) 'Application progress was cleared between repositories instead of after the phase.'
        Assert-True ($result.Progress[-1].Completed -and $result.Text -match 'Applying repository \[1/2\]') 'Application progress cleanup or plain-text output is missing.'
    }

    Test-Case 'progress is cleared for empty discovery and setup errors' {
        $f = New-BulkFixture
        $empty = Invoke-BulkFixture $f @{ DryRun = $true } -CaptureProgress
        Assert-True ($empty.ExitCode -eq 0 -and $empty.Progress[-1].Completed) 'Empty discovery left progress active or failed.'
        $missing = Invoke-BulkFixture $f @{ Root = (Join-Path $f.Directory 'missing') } -CaptureProgress
        Assert-True ($missing.ExitCode -ne 0 -and $missing.Progress[-1].Completed) 'An error left progress active or reported success.'
    }

    Test-Case 'directory masks prune matching trees before enumeration at every depth' {
        $f = New-BulkFixture
        [void](Set-TestProfile $f (New-TestRepo $f 'keep'))
        $build = Split-Path -Parent (New-TestRepo $f 'BUILD.OBJ.TEMP\hidden')
        $scratch = Split-Path -Parent (New-TestRepo $f 'group\scratch\hidden')
        $literal = Split-Path -Parent (New-TestRepo $f 'cache[1]\hidden')
        $dependency = Split-Path -Parent (New-TestRepo $f 'node_modules\hidden')
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ DryRun = $true; IncludeNewRepos = $true
            ExcludeDirectory = @('*.obj.temp', 'SCRATCH', [Management.Automation.WildcardPattern]::Escape('cache[1]')) } -ForbiddenDiscoveryPaths @($build, $scratch, $literal, $dependency)
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Exclusion discovery wrote fixture files.'
        Assert-True ($result.Text.Contains("Skipping excluded directory: $build") -and $result.Text.Contains("Skipping excluded directory: $scratch") -and $result.Text.Contains("Skipping excluded directory: $literal")) 'Case-insensitive wildcard/name exclusion or escaped brackets are missing.'
        Assert-True ($result.Text -notmatch '\\hidden') 'An excluded checkout was inspected.'
        Assert-True ($result.Text -match 'Plan only: 1 selected; 0 conflicts') 'Excluded repositories affected selection.'
    }

    Test-Case 'directory masks do not exclude the explicitly selected Root' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'build.obj.temp'
        [void](Set-TestProfile $f $repo)
        $result = Invoke-BulkFixture $f @{ Root = $repo; DryRun = $true; ExcludeDirectory = @('*.obj.temp') }
        Assert-True ($result.ExitCode -eq 0 -and $result.Text -match 'Plan only: 1 selected; 0 conflicts') $result.Text
    }

    Test-Case 'directory exclusions reject paths instead of silently matching nothing' {
        $f = New-BulkFixture
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ ExcludeDirectory = @('group\scratch') }
        Assert-True ($result.ExitCode -ne 0 -and $result.Text -match 'directory names or wildcard masks, not paths') 'An unsupported exclusion path was silently accepted.'
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Invalid exclusions changed fixture files.'
    }

    Test-Case 'malformed directory masks are rejected before discovery even for Root as checkout' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'repo'
        [void](Set-TestProfile $f $repo)
        $before = Get-FixtureSnapshot $f
        foreach ($root in @($f.Root, $repo)) {
            $result = Invoke-BulkFixture $f @{ Root = $root; DryRun = $true; ExcludeDirectory = @('[abc') }
            Assert-True ($result.ExitCode -ne 0 -and $result.Text -match 'ExcludeDirectory' -and $result.Text -match 'wildcard') 'A malformed directory mask was accepted.'
            Assert-True ($result.Text -notmatch 'Discovering Git repositories under') 'Malformed masks reached directory discovery.'
        }
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Malformed masks changed fixture files.'
    }

    Test-Case 'include-new creates profiles and ignores dependency and redirected trees' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'group\new'
        $ignored = New-TestRepo $f 'node_modules\ignored'
        $outside = Join-Path $f.Directory 'outside'
        New-Item -ItemType Directory -Path $outside | Out-Null
        & git init --quiet $outside
        $junction = Join-Path $f.Root 'redirect'
        New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
        try {
            $result = Invoke-BulkFixture $f @{ IncludeNewRepos = $true; Extensions = 'rs,md' }
            Assert-True ($result.ExitCode -eq 0) $result.Text
            Assert-True (Test-Path -LiteralPath (Get-TestProfilePath $f $repo)) 'New profile absent.'
            Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $ignored))) 'Dependency tree was scanned.'
            Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $outside))) 'Junction was followed.'
            Assert-True ($result.Text -match 'Skipping reparse point') 'Skipped junction not reported.'
        }
        finally { [IO.Directory]::Delete($junction) }
    }

    Test-Case 'legacy migration preserves metrics and unrelated servers' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'legacy'
        Set-TestLegacy $f $repo '.mcp.json' @('--ext', 'md,rs', '--metrics', '--watch')
        Set-TestLegacy $f $repo '.vscode\mcp.json' @('--watch', '--metrics', '--ext', 'RS,md')
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $profile = Get-Content -LiteralPath (Get-TestProfilePath $f $repo) -Raw | ConvertFrom-Json
        Assert-True ($profile.args -contains '--metrics') 'Metrics lost during migration.'
        foreach ($relative in @('.mcp.json', '.vscode\mcp.json')) {
            $doc = Get-Content -LiteralPath (Join-Path $repo $relative) -Raw | ConvertFrom-Json
            $servers = if ($relative -eq '.mcp.json') { $doc.mcpServers } else { $doc.servers }
            Assert-True (-not $servers.xray) "Legacy Xray remains in $relative."
            Assert-True ($servers.other.command -eq 'other.exe') 'Unrelated server changed.'
        }
        $again = Invoke-BulkFixture $f
        Assert-True ($again.ExitCode -eq 0) $again.Text
        Assert-True ((@(Get-ChildItem -LiteralPath (Join-Path $f.Install 'workspaces') -Directory)).Count -eq 1) 'Repeat setup duplicated profiles.'
    }

    Test-Case 'conflicting clients are skipped unchanged while eligible repositories proceed' {
        $f = New-BulkFixture
        $bad = New-TestRepo $f 'conflict'
        Set-TestLegacy $f $bad '.mcp.json' @('--ext', 'rs', '--metrics')
        Set-TestLegacy $f $bad '.vscode\mcp.json' @('--ext', 'rs')
        $before = [IO.File]::ReadAllText((Join-Path $bad '.mcp.json'))
        $good = New-TestRepo $f 'good'
        [void](Set-TestProfile $f $good)
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -ne 0) 'Conflict returned success.'
        Assert-True ($result.Text -match 'Conflicting Xray settings') $result.Text
        Assert-True ($before -ceq [IO.File]::ReadAllText((Join-Path $bad '.mcp.json'))) 'Conflicting configuration changed.'
        Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $bad))) 'Conflict got a profile.'
        Assert-True (Test-Path -LiteralPath (Get-TestProfilePath $f $good)) 'Eligible repository was not processed.'
    }

    Test-Case 'tracked legacy configuration cannot be migrated even with Force' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'tracked'
        Set-TestLegacy $f $repo
        & git -C $repo add -- .vscode/mcp.json
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -ne 0) 'Tracked Xray config migrated.'
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Tracked configuration or Git index changed.'
    }

    Test-Case 'malformed profiles and unsupported legacy environments are visible conflicts' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'malformed'
        Write-TestFile (Get-TestProfilePath $f $repo) '{broken'
        $legacy = New-TestRepo $f 'environment'
        Set-TestLegacy $f $legacy
        $config = Join-Path $legacy '.vscode\mcp.json'
        $doc = Get-Content -LiteralPath $config -Raw | ConvertFrom-Json
        $doc.servers.xray | Add-Member -NotePropertyName env -NotePropertyValue @{ XRAY_GUIDANCE_PREFIX = '0' }
        Write-TestFile $config ($doc | ConvertTo-Json -Depth 10)
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ DryRun = $true }
        Assert-True ($result.ExitCode -ne 0) 'Unsafe settings returned success.'
        Assert-True ($result.Text -match 'Invalid JSON') $result.Text
        Assert-True ($result.Text -match 'Cannot preserve') $result.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Conflict preflight wrote files.'
    }

    Test-Case 'readable rename preserves legacy profile directory and backups' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'profile'
        $old = Set-TestProfile $f $repo @('--ext', 'rs', '--metrics') -Legacy
        Write-TestFile "$old.bak" ([IO.File]::ReadAllText($old))
        Write-TestFile (Join-Path (Split-Path -Parent $old) 'extra.txt') 'preserve me'
        $before = Get-FixtureSnapshot $f
        $dry = Invoke-BulkFixture $f @{ DryRun = $true }
        Assert-True ($dry.ExitCode -eq 0) $dry.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'DryRun renamed or modified the old profile.'
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $current = Get-TestProfilePath $f $repo
        Assert-True (-not (Test-Path -LiteralPath $old)) 'Legacy profile directory remains.'
        Assert-True (Test-Path -LiteralPath "$current.bak") 'Backup was lost.'
        Assert-True ([IO.File]::ReadAllText((Join-Path (Split-Path -Parent $current) 'extra.txt')) -ceq 'preserve me') 'Extra profile file lost.'
    }

    Test-Case 'managed client fields and enablement survive repeat bulk setup' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'profile'
        [void](Set-TestProfile $f $repo)
        $first = Invoke-BulkFixture $f
        Assert-True ($first.ExitCode -eq 0) $first.Text
        $path = Join-Path $f.Home 'mcp-config.json'
        $doc = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        $doc.mcpServers.xray | Add-Member -NotePropertyName disabled -NotePropertyValue $true
        $doc.mcpServers.xray | Add-Member -NotePropertyName env -NotePropertyValue @{ CUSTOM = 'keep me' }
        $doc.mcpServers.xray.tools = @('xray_info')
        Write-TestFile $path ($doc | ConvertTo-Json -Depth 10)
        $second = Invoke-BulkFixture $f
        Assert-True ($second.ExitCode -eq 0) $second.Text
        $doc = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        Assert-True ($doc.mcpServers.xray.disabled -eq $true) 'Enablement field overwritten.'
        Assert-True ($doc.mcpServers.xray.env.CUSTOM -ceq 'keep me') 'Managed entry environment overwritten.'
        Assert-True ($doc.mcpServers.xray.tools[0] -eq 'xray_info') 'Managed tool restrictions overwritten.'
    }

    Test-Case 'foreign personal registration is never replaced' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'profile'
        [void](Set-TestProfile $f $repo)
        Write-TestFile (Join-Path $f.Home 'mcp-config.json') '{"mcpServers":{"xray":{"command":"foreign.exe","args":[]}}}'
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f
        Assert-True ($result.ExitCode -ne 0) 'Foreign registration accepted.'
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Foreign registration replaced.'
    }

    Test-Case 'orchestrator downloads once and passes explicit stop consent only' {
        $f = New-BulkFixture
        [void](New-TestRepo $f 'a')
        [void](New-TestRepo $f 'b')
        $scripts = Join-Path $f.Directory 'scripts'
        New-Item -ItemType Directory -Path $scripts | Out-Null
        Copy-Item -LiteralPath $bulkScript -Destination (Join-Path $scripts 'setup-xray-repos.ps1')
        Write-TestFile (Join-Path $scripts 'setup-xray.ps1') @'
param($RepoPath, $InstallDir, [switch]$UserMcp, [switch]$PlanOnly, [switch]$PreserveSettings,
    [switch]$MigrateLegacy, [switch]$Force, [switch]$SkipDownload, [switch]$KillRunning, $VSCodeMcpPath, $GithubRepo, [ref]$BinaryPrepared)
if ($PlanOnly) {
    [PSCustomObject]@{ RepoPath = $RepoPath; Existing = $true; Source = 'Profile'; Extensions = ''; Arguments = @('--ext', 'rs') }
    return
}
Add-Content -LiteralPath (Join-Path $InstallDir 'calls.jsonl') -Value (@{
    Repo = $RepoPath; Skip = [bool]$SkipDownload; Kill = [bool]$KillRunning; Preserve = [bool]$PreserveSettings
} | ConvertTo-Json -Compress)
exit 0
'@
        $script = Join-Path $scripts 'setup-xray-repos.ps1'
        $result = Invoke-BulkFixture $f @{ SkipDownload = $false } $script
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $calls = @(Get-Content -LiteralPath (Join-Path $f.Install 'calls.jsonl') | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-True ($calls.Count -eq 2) 'Not all selected repos were invoked.'
        Assert-True (-not $calls[0].Skip -and $calls[1].Skip) 'Download was not restricted to first invocation.'
        Assert-True (-not $calls[0].Kill -and -not $calls[1].Kill) 'Force implied stop consent.'
        Assert-True ($calls[0].Preserve -and $calls[1].Preserve) 'Settings preservation was not requested.'
    }

    Test-Case 'Git worktree files and Root-as-checkout are supported' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'main'
        $hooks = Join-Path $f.Directory 'empty-hooks'
        New-Item -ItemType Directory -Path $hooks | Out-Null
        & git -C $repo -c user.name=Test -c user.email=test@example.invalid -c core.autocrlf=false add sample.rs
        & git -C $repo -c user.name=Test -c user.email=test@example.invalid -c commit.gpgSign=false -c "core.hooksPath=$hooks" commit --quiet -m fixture
        if ($LASTEXITCODE -ne 0) { throw 'Fixture commit failed.' }
        $worktree = Join-Path $f.Root 'worktree'
        & git -C $repo worktree add --quiet --detach $worktree
        if ($LASTEXITCODE -ne 0) { throw 'Fixture worktree creation failed.' }
        $result = Invoke-BulkFixture $f @{ IncludeNewRepos = $true; Extensions = 'rs' }
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True (Test-Path -LiteralPath (Get-TestProfilePath $f $worktree)) 'Worktree .git file was not discovered.'
        $again = Invoke-BulkFixture $f @{ Root = $repo }
        Assert-True ($again.ExitCode -eq 0) $again.Text
    }

    Test-Case 'invalid nested Git metadata cannot redirect setup outside Root' {
        $f = New-BulkFixture
        $outer = New-TestRepo $f 'outer'
        $group = Join-Path $outer 'group'
        New-Item -ItemType Directory -Path (Join-Path $group 'pkg\.git') -Force | Out-Null
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ Root = $group; IncludeNewRepos = $true; Extensions = 'rs' }
        Assert-True ($result.ExitCode -ne 0 -and $result.Text -match 'different Git checkout') 'Invalid metadata redirected setup to the parent checkout.'
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'An out-of-root checkout was enrolled.'
        [void](Set-TestProfile $f $outer)
        Set-TestLegacy $f $outer -Options @('--ext', 'rs', '--watch', '--definitions')
        $before = Get-FixtureSnapshot $f
        foreach ($includeNew in @($false, $true)) {
            foreach ($dryRun in @($false, $true)) {
                $result = Invoke-BulkFixture $f @{ Root = $group; IncludeNewRepos = $includeNew; DryRun = $dryRun; Extensions = 'rs' }
                Assert-True ($result.ExitCode -ne 0 -and $result.Text -match 'different Git checkout') $result.Text
                Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'The out-of-root profile or legacy configuration was changed.'
            }
        }
    }

    Test-Case 'no eligible repositories means no download or registration' {
        $f = New-BulkFixture
        [void](New-TestRepo $f 'new')
        $before = Get-FixtureSnapshot $f
        $result = Invoke-BulkFixture $f @{ SkipDownload = $false }
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Empty selection changed files.'
        $include = Invoke-BulkFixture $f @{ IncludeNewRepos = $true; Force = $false }
        Assert-True ($include.ExitCode -ne 0) 'Noninteractive setup applied without confirmation.'
        Assert-True ($before -ceq (Get-FixtureSnapshot $f)) 'Unapproved setup changed files.'
    }

    Test-Case 'fresh-install dry-run does not create the installation directory' {
        $f = New-BulkFixture
        [void](New-TestRepo $f 'new')
        $absent = Join-Path $f.Directory 'not installed'
        $result = Invoke-BulkFixture $f @{ InstallDir = $absent; IncludeNewRepos = $true; SkipDownload = $false; DryRun = $true }
        Assert-True ($result.ExitCode -eq 0) $result.Text
        Assert-True (-not (Test-Path -LiteralPath $absent)) 'DryRun created the installation directory.'
    }

    Test-Case 'new-repository setup detects extensions without interactive input' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'new'
        foreach ($index in 1..4) { Write-TestFile (Join-Path $repo "source$index.rs") 'pub fn detected_marker() {}' }
        $result = Invoke-BulkFixture $f @{ IncludeNewRepos = $true }
        Assert-True ($result.ExitCode -eq 0) $result.Text
        $profile = Get-Content -LiteralPath (Get-TestProfilePath $f $repo) -Raw | ConvertFrom-Json
        $extIndex = [array]::IndexOf($profile.args, '--ext')
        Assert-True ($extIndex -ge 0 -and @($profile.args[$extIndex + 1] -split ',') -contains 'rs') 'Detected Rust extension was not installed.'
    }

    Test-Case 'a new repository below the detector threshold requests explicit extensions' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'small'
        $result = Invoke-BulkFixture $f @{ IncludeNewRepos = $true }
        Assert-True ($result.ExitCode -ne 0) 'An empty extension selection reported success.'
        Assert-True ($result.Text -match 'Supply -Extensions') 'Missing extension guidance.'
        Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $repo))) 'An empty-extension profile was created.'
    }

    Test-Case 'failed binary preparation stops the batch without repeated downloads' {
        $f = New-BulkFixture
        [void](New-TestRepo $f 'a')
        [void](New-TestRepo $f 'b')
        $scripts = Join-Path $f.Directory 'scripts'
        New-Item -ItemType Directory -Path $scripts | Out-Null
        Copy-Item -LiteralPath $bulkScript -Destination (Join-Path $scripts 'setup-xray-repos.ps1')
        Write-TestFile (Join-Path $scripts 'setup-xray.ps1') @'
param($RepoPath, $InstallDir, [switch]$UserMcp, [switch]$PlanOnly, [switch]$PreserveSettings,
    [switch]$MigrateLegacy, [switch]$Force, [switch]$SkipDownload, [switch]$KillRunning, $VSCodeMcpPath, $GithubRepo, [ref]$BinaryPrepared)
if ($PlanOnly) {
    [PSCustomObject]@{ RepoPath = $RepoPath; Existing = $true; Source = 'Profile'; Extensions = ''; Arguments = @('--ext', 'rs') }
    return
}
Add-Content -LiteralPath (Join-Path $InstallDir 'attempts.txt') -Value 'download attempted'
exit 1
'@
        $result = Invoke-BulkFixture $f @{ SkipDownload = $false } (Join-Path $scripts 'setup-xray-repos.ps1')
        Assert-True ($result.ExitCode -ne 0) 'Binary failure returned success.'
        Assert-True (@(Get-Content -LiteralPath (Join-Path $f.Install 'attempts.txt')).Count -eq 1) 'Failed download retried per repository.'
        Assert-True ($result.Text -match 'stopping the batch') 'Batch stop not reported.'
        Assert-True ($result.Text -match 'Not run') 'Unprocessed repositories were not marked Not run.'
    }

    Test-Case 'repository failures after binary preparation do not stop other repositories' {
        $f = New-BulkFixture
        $empty = New-TestRepo $f 'a-empty'
        Remove-Item -LiteralPath (Join-Path $empty 'sample.rs')
        $small = New-TestRepo $f 'b-small'
        $good = New-TestRepo $f 'c-good'
        foreach ($index in 1..4) { Write-TestFile (Join-Path $good "source$index.rs") 'pub fn detected_marker() {}' }
        $scripts = Join-Path $f.Directory 'scripts'
        New-Item -ItemType Directory -Path $scripts | Out-Null
        Copy-Item -LiteralPath $bulkScript -Destination (Join-Path $scripts 'setup-xray-repos.ps1')
        $installerText = [IO.File]::ReadAllText($setupScript)
        $downloadCall = 'Save-XrayReleaseAsset -GithubRepo $GithubRepo -Destination $downloaded'
        Assert-True ([regex]::Matches($installerText, [regex]::Escape($downloadCall)).Count -eq 1) 'Download fixture replacement no longer matches the installer.'
        $downloadStub = @'
Copy-Item -LiteralPath '__BINARY__' -Destination $downloaded
            Add-Content -LiteralPath (Join-Path $InstallDir 'downloads.txt') -Value 'download'
'@
        $downloadStub = $downloadStub.Replace('__BINARY__', ([IO.Path]::GetFullPath($XrayBinary)).Replace("'", "''"))
        Write-TestFile (Join-Path $scripts 'setup-xray.ps1') ($installerText.Replace($downloadCall, $downloadStub))
        $result = Invoke-BulkFixture $f @{ IncludeNewRepos = $true; SkipDownload = $false } (Join-Path $scripts 'setup-xray-repos.ps1')
        Assert-True ($result.ExitCode -ne 0) 'Partial repository failure reported success.'
        Assert-True (@(Get-Content -LiteralPath (Join-Path $f.Install 'downloads.txt')).Count -eq 1) 'The shared executable was downloaded repeatedly.'
        Assert-True ($result.Text -match 'No recognized code/text files' -and $result.Text -match 'Supply -Extensions') 'Failed repository guidance was omitted.'
        Assert-True ($result.Text -notmatch 'Not run|stopping the batch') 'Repository failure was misclassified as binary preparation failure.'
        Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $empty))) 'The empty repository got a profile.'
        Assert-True (-not (Test-Path -LiteralPath (Get-TestProfilePath $f $small))) 'The below-threshold repository got a profile.'
        Assert-True (Test-Path -LiteralPath (Get-TestProfilePath $f $good)) 'The later eligible repository was not configured.'
    }

    Test-Case 'a changed first repository is skipped before preparing the binary for the next' {
        $f = New-BulkFixture
        [void](New-TestRepo $f 'a')
        [void](New-TestRepo $f 'b')
        $scripts = Join-Path $f.Directory 'scripts'
        New-Item -ItemType Directory -Path $scripts | Out-Null
        Copy-Item -LiteralPath $bulkScript -Destination (Join-Path $scripts 'setup-xray-repos.ps1')
        Write-TestFile (Join-Path $scripts 'setup-xray.ps1') @'
param($RepoPath, $InstallDir, [switch]$UserMcp, [switch]$PlanOnly, [switch]$PreserveSettings,
    [switch]$MigrateLegacy, [switch]$Force, [switch]$SkipDownload, [switch]$KillRunning, $VSCodeMcpPath, $GithubRepo, [ref]$BinaryPrepared)
if ($PlanOnly) {
    if (-not $global:bulkTestPreflights) { $global:bulkTestPreflights = @{} }
    $global:bulkTestPreflights[$RepoPath]++
    $ext = if ((Split-Path -Leaf $RepoPath) -eq 'a' -and $global:bulkTestPreflights[$RepoPath] -gt 1) { 'cs' } else { 'rs' }
    [PSCustomObject]@{ RepoPath = $RepoPath; Existing = $true; HasLegacy = $false; Source = 'Profile'; Extensions = $ext; Arguments = @('--ext', $ext) }
    return
}
Add-Content -LiteralPath (Join-Path $InstallDir 'calls.jsonl') -Value (@{ Repo = $RepoPath; Skip = [bool]$SkipDownload } | ConvertTo-Json -Compress)
$BinaryPrepared.Value = $true
exit 0
'@
        $result = Invoke-BulkFixture $f @{ SkipDownload = $false } (Join-Path $scripts 'setup-xray-repos.ps1')
        Assert-True ($result.ExitCode -ne 0 -and $result.Text -match 'settings changed after planning') 'A stale plan was not rejected.'
        $calls = @(Get-Content -LiteralPath (Join-Path $f.Install 'calls.jsonl') | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-True ($calls.Count -eq 1 -and (Split-Path -Leaf $calls[0].Repo) -eq 'b' -and -not $calls[0].Skip) 'The next eligible repository did not prepare the binary.'
    }

    Test-Case 'Force never stops a running fixture executable or downloads before stop consent' {
        $f = New-BulkFixture
        $repo = New-TestRepo $f 'running'
        [void](Set-TestProfile $f $repo)
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = Join-Path $f.Install 'xray.exe'
        $start.Arguments = 'serve --standalone --dir "' + $repo + '" --ext rs'
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.EnvironmentVariables['LOCALAPPDATA'] = $f.LocalAppData
        $process = [Diagnostics.Process]::Start($start)
        try {
            Start-Sleep -Milliseconds 300
            Assert-True (-not $process.HasExited) 'Fixture MCP process did not remain running.'
            $result = Invoke-BulkFixture $f @{ SkipDownload = $false }
            Assert-True ($result.ExitCode -ne 0) 'Running shared binary update succeeded without stop consent.'
            Assert-True ($result.Text -match 'explicitly approve -KillRunning') $result.Text
            Assert-True (-not $process.HasExited) 'Force killed the unapproved process.'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'mcp-config.json'))) 'Refused binary update registered MCP.'
        }
        finally {
            $process.StandardInput.Close()
            if (-not $process.WaitForExit(10000)) { $process.Kill(); $process.WaitForExit() }
            $process.Dispose()
        }
    }
}
finally {
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}

Write-Host "Assertions: $script:passes passed; cases failed: $script:failures"
if ($script:failures -gt 0) { exit 1 }
