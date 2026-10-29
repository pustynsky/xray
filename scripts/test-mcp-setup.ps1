#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Isolated regression tests for local MCP setup and explicit legacy migration.
#>
param([string]$XrayBinary, [switch]$UserMcpOnly, [string]$CasePattern = '*')

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$PSNativeCommandUseErrorActionPreference = $false
$scriptPath = Join-Path $PSScriptRoot 'setup-xray.ps1'
$powerShellExe = (Get-Process -Id $PID).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('xray-mcp-setup-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$script:passes = 0
$script:failures = 0
$oldGlobal = $env:GIT_CONFIG_GLOBAL
$oldSystem = $env:GIT_CONFIG_NOSYSTEM
$oldCopilotHome = $env:COPILOT_HOME
$env:GIT_CONFIG_GLOBAL = Join-Path $root 'global.gitconfig'
$env:GIT_CONFIG_NOSYSTEM = '1'

function Assert-True {
    param([bool]$Condition, [string]$Label)
    if (-not $Condition) { throw $Label }
    $script:passes++
}

function Test-Case {
    param([string]$Name, [scriptblock]$Body)
    if ($CasePattern -and $Name -notlike $CasePattern) { return }
    try {
        & $Body
        Write-Host "PASS $Name" -ForegroundColor Green
    }
    catch {
        $script:failures++
        Write-Host "FAIL $Name`: $($_.Exception.Message)" -ForegroundColor Red
    }
}

function Write-FixtureFile {
    param([string]$Path, [string]$Text)
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function New-Fixture {
    $directory = Join-Path $root ([Guid]::NewGuid().ToString('N'))
    $repo = Join-Path $directory 'repo with spaces'
    $install = Join-Path $directory 'install with spaces'
    New-Item -ItemType Directory -Path $repo, $install -Force | Out-Null
    Write-FixtureFile -Path (Join-Path $install 'xray.exe') -Text 'test binary; never executed'
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('init', '--quiet'))
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('config', 'user.name', 'MCP Setup Test'))
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('config', 'user.email', 'mcp-test@example.invalid'))
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('config', 'core.autocrlf', 'false'))
    Write-FixtureFile -Path (Join-Path $repo 'source.ts') -Text 'export const value = 1;'
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('add', 'source.ts'))
    [void](Invoke-SetupGit -RepoRoot $repo -Arguments @('commit', '--quiet', '-m', 'fixture baseline'))
    return [PSCustomObject]@{ Repo = $repo; Install = $install; Home = (Join-Path $directory 'copilot home') }
}

function Invoke-FixtureSetup {
    param($Fixture, [hashtable]$Parameters = @{}, [switch]$Inline, [string]$Location, [string]$BackupReplacement, [string]$DriveRoot)
    $arguments = @{
        RepoPath = $Fixture.Repo; InstallDir = $Fixture.Install
        SkipDownload = $true; Extensions = ' .CS, md,cs '; Force = $true
    }
    foreach ($key in $Parameters.Keys) { $arguments[$key] = $Parameters[$key] }
    $payload = @{ Path = $scriptPath; Arguments = $arguments; Inline = [bool]$Inline; Location = $Location; BackupReplacement = $BackupReplacement; DriveRoot = $DriveRoot } | ConvertTo-Json -Depth 10 -Compress
    $encodedPayload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $code = @'
$ErrorActionPreference = 'Stop'
$payload = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
$parameters = @{}
foreach ($property in $payload.Arguments.PSObject.Properties) { $parameters[$property.Name] = $property.Value }
if ($payload.DriveRoot) {
    New-PSDrive -Name XrayFixtureDrive -PSProvider FileSystem -Root $payload.DriveRoot | Out-Null
}
if ($payload.Location) { Set-Location -LiteralPath $payload.Location }
if ($payload.BackupReplacement) {
    function Copy-Item {
        [CmdletBinding()]
        param([string]$LiteralPath, [string]$Destination, [switch]$Force)
        [IO.File]::WriteAllText($LiteralPath, $payload.BackupReplacement, [Text.UTF8Encoding]::new($false))
        $copyItem = $ExecutionContext.InvokeCommand.GetCommand('Copy-Item', [Management.Automation.CommandTypes]::Cmdlet)
        & $copyItem @PSBoundParameters
    }
}
$global:LASTEXITCODE = 73
try {
    if ($payload.Inline) {
        & ([scriptblock]::Create([IO.File]::ReadAllText($payload.Path))) @parameters
    }
    else {
        & $payload.Path @parameters
    }
    $setupExitCode = $LASTEXITCODE
    Write-Output "XRAY_SETUP_RETURNED:$setupExitCode"
    exit $setupExitCode
}
catch {
    Write-Output 'XRAY_SETUP_CAUGHT_ERROR'
    Write-Output ("Setup failed: " + $_.Exception.Message)
    exit 1
}
'@.Replace('__PAYLOAD__', $encodedPayload)
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $powerShellExe
    $info.Arguments = '-NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['COPILOT_HOME'] = $Fixture.Home
    $info.EnvironmentVariables['APPDATA'] = Join-Path $Fixture.Home 'appdata'
    $info.EnvironmentVariables.Remove('VSCODE_PORTABLE')
    return Invoke-FixtureProcess -Info $info -Label 'Setup'
}

function New-UserMcpFixture {
    if (-not $XrayBinary -or -not (Test-Path -LiteralPath $XrayBinary -PathType Leaf)) {
        throw 'User MCP tests require -XrayBinary pointing to a freshly built executable.'
    }
    $fixture = New-Fixture
    Copy-Item -LiteralPath $XrayBinary -Destination (Join-Path $fixture.Install 'xray.exe') -Force
    return $fixture
}

function Invoke-UserMcpSetup {
    param($Fixture, [hashtable]$Parameters = @{}, [switch]$Inline)
    $options = @{ UserMcp = $true; VSCodeMcpPath = (Join-Path $Fixture.Home 'vscode\mcp.json') }
    foreach ($key in $Parameters.Keys) { $options[$key] = $Parameters[$key] }
    Invoke-FixtureSetup $Fixture $options -Inline:$Inline
}

function Move-FixtureProfileToLegacy {
    param($Fixture)
    $profiles = Join-Path $Fixture.Install 'workspaces'
    $current = Get-WorkspaceProfilePath $profiles $Fixture.Repo
    $hash = [Security.Cryptography.SHA256]::Create()
    try {
        $key = [IO.Path]::GetFullPath($Fixture.Repo).TrimEnd('\', '/').ToLowerInvariant()
        $id = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($key)))).Replace('-', '').ToLowerInvariant()
        $legacy = Join-Path (Join-Path $profiles $id) 'config.json'
    }
    finally { $hash.Dispose() }
    if ($current -ine $legacy) {
        [IO.Directory]::Move((Split-Path -Parent $current), (Split-Path -Parent $legacy))
    }
    return $legacy
}

function Invoke-FixtureProcess {
    param([Diagnostics.ProcessStartInfo]$Info, [string]$Label)

    $process = New-Object Diagnostics.Process
    $process.StartInfo = $Info
    try {
        [void]$process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) { throw "$Label timed out" }
        return [PSCustomObject]@{ ExitCode = $process.ExitCode; Output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult() }
    }
    finally {
        if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $process.Dispose()
    }
}

function Assert-Success {
    param($Result)
    Assert-True ($Result.ExitCode -eq 0) "Setup failed: $($Result.Output)"
}

function Assert-SetupReturned {
    param($Result)
    Assert-Success $Result
    Assert-True ($Result.Output -match '(?m)^XRAY_SETUP_RETURNED:0\r?$') "Setup did not return control with a clean native status: $($Result.Output)"
}

function Invoke-FixtureLauncher {
    param($Fixture, [string]$Arguments = '')

    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $env:ComSpec
    $info.Arguments = '/d /s /c ""' + (Join-Path $Fixture.Repo 'copilot-xray.cmd') + '" ' + $Arguments + '"'
    $info.WorkingDirectory = $Fixture.Install
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['PATH'] = $Fixture.Install + ';' + (Join-Path $env:SystemRoot 'System32')
    return Invoke-FixtureProcess -Info $info -Label 'CLI launcher'
}

function Get-FixtureJson {
    param([string]$Path)
    return (Read-SetupJson -Path $Path).Value
}

function Assert-XrayGitEnvironment {
    param($Environment, [int]$Count = 3)
    Assert-True ($Environment.GIT_CONFIG_COUNT -ceq [string]$Count) 'Exact Git override count'
    $expected = @('safe.bareRepository', 'explicit', 'credential.interactive', 'never', 'core.fsmonitor', 'false')
    for ($i = 0; $i -lt 3; $i++) {
        Assert-True ($Environment."GIT_CONFIG_KEY_$i" -ceq $expected[2 * $i]) "Git key $i"
        Assert-True ($Environment."GIT_CONFIG_VALUE_$i" -ceq $expected[2 * $i + 1]) "Git value $i"
    }
}

function Get-EnvironmentTarget {
    param($Environment, [string]$Path = 'fixture.json')
    $entry = [PSCustomObject]@{}
    if ($PSBoundParameters.ContainsKey('Environment')) {
        $entry | Add-Member -NotePropertyName env -NotePropertyValue $Environment
    }
    return [PSCustomObject]@{
        Path = $Path; Container = 'servers'
        Document = [PSCustomObject]@{ Value = [PSCustomObject]@{ servers = [PSCustomObject]@{ xray = $entry } } }
    }
}

function Receive-FixtureMcpResponse {
    param([Diagnostics.Process]$Process, [int]$Id)
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt 20000) {
        $read = $Process.StandardOutput.ReadLineAsync()
        if (-not $read.Wait([int](20000 - $timer.ElapsedMilliseconds))) { throw 'MCP response timed out.' }
        $line = $read.GetAwaiter().GetResult()
        if ($null -eq $line) { throw 'MCP exited before responding.' }
        $message = $line | ConvertFrom-Json
        if ($message.id -eq $Id) { return $message }
    }
    throw 'MCP response timed out.'
}

function Add-LegacyFixture {
    param($Fixture)
    $path = Join-Path $Fixture.Repo '.mcp.json'
    $canonical = "{`r`n  `"mcpServers`": {`r`n    `"notes`": { `"command`": `"notes`", `"args`": [] }`r`n  }`r`n}`r`n"
    Write-FixtureFile -Path $path -Text $canonical
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('add', '.mcp.json'))
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('commit', '--quiet', '-m', 'tracked MCP baseline'))
    $entry = @{ command = 'old-xray.exe'; args = @('serve'); _xrayMcpMarker = $Script:XrayMcpMarker } | ConvertTo-Json -Compress
    $injected = $canonical.Replace("`"mcpServers`": {`r`n", "`"mcpServers`": {`r`n    `"xray`":$entry,`r`n")
    Write-FixtureFile -Path $path -Text $injected
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('config', 'filter.xray-mcp.clean', 'cat'))
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('config', 'filter.xray-mcp.smudge', 'cat'))
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('config', 'filter.xray-mcp.required', 'false'))
    Write-FixtureFile -Path (Join-Path $Fixture.Repo '.git\info\attributes') -Text ".mcp.json filter=xray-mcp`n*.txt eol=lf`n"
    Write-FixtureFile -Path (Join-Path $Fixture.Repo '.git\xray-mcp\snapshot.txt') -Text $entry
    [void](Invoke-SetupGit -RepoRoot $Fixture.Repo -Arguments @('update-index', '--skip-worktree', '--assume-unchanged', '--', '.mcp.json'))
    return $canonical
}

try {
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$errors)
    if ($errors.Count -gt 0) { throw ($errors | Out-String) }
    foreach ($function in $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
        . ([scriptblock]::Create($function.Extent.Text))
    }
    foreach ($name in @('$Script:XrayMcpMarker', '$Script:XrayVisibleWarning')) {
        $assignment = $ast.Find({ param($node) $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq $name }, $true)
        . ([scriptblock]::Create($assignment.Extent.Text))
    }

    Test-Case 'Git environment defaults preserve custom pairs and normalize compatible aliases' {
        $first = Get-EnvironmentTarget ([PSCustomObject][ordered]@{
            CUSTOM = 'keep'
            GIT_CONFIG_COUNT = '5'
            GIT_CONFIG_KEY_0 = 'credential.interactive'; GIT_CONFIG_VALUE_0 = 'false'
            GIT_CONFIG_KEY_1 = 'core.fsmonitor'; GIT_CONFIG_VALUE_1 = 'OFF'
            GIT_CONFIG_KEY_2 = 'safe.directory'; GIT_CONFIG_VALUE_2 = 'first'
            GIT_CONFIG_KEY_3 = 'safe.directory'; GIT_CONFIG_VALUE_3 = ''
            GIT_CONFIG_KEY_4 = 'safe.directory'; GIT_CONFIG_VALUE_4 = 'last'
        })
        $second = Get-EnvironmentTarget -Path 'other.json'
        Resolve-XrayMcpEnvironment @($first, $second)
        foreach ($target in @($first, $second)) {
            Assert-XrayGitEnvironment $target.Environment 6
            Assert-True ($target.Environment.GIT_CONFIG_KEY_3 -ceq 'safe.directory' -and
                $target.Environment.GIT_CONFIG_KEY_4 -ceq 'safe.directory' -and
                $target.Environment.GIT_CONFIG_KEY_5 -ceq 'safe.directory') 'Repeated keys retained'
            Assert-True ($target.Environment.GIT_CONFIG_VALUE_3 -ceq 'first' -and
                $target.Environment.GIT_CONFIG_VALUE_4 -ceq '' -and
                $target.Environment.GIT_CONFIG_VALUE_5 -ceq 'last') 'Custom order and empty reset retained'
        }
        Assert-True ($first.Environment.CUSTOM -ceq 'keep') 'Unrelated environment retained'
        Assert-True (-not $second.Environment.PSObject.Properties['CUSTOM']) 'Unrelated environment not copied to another host'
    }

    Test-Case 'Git environment rejects invalid shapes and conflicting overrides without disclosing values' {
        $invalid = @(
            'not an object',
            [PSCustomObject]@{ GIT_CONFIG_KEY_0 = 'core.quotePath'; GIT_CONFIG_VALUE_0 = 'false' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = 0 },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '-1' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '2147483648' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '0'; GIT_CONFIG_KEY_0 = 'unused' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_1 = 'gap'; GIT_CONFIG_VALUE_1 = 'false' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'invalid-key'; GIT_CONFIG_VALUE_0 = 'private-setting-value' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'section.1name'; GIT_CONFIG_VALUE_0 = 'false' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = "section.sub`nsection.name"; GIT_CONFIG_VALUE_0 = 'false' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'http.extraHeader'; GIT_CONFIG_VALUE_0 = ("private" + [char]0 + 'value') },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'core.fsmonitor'; GIT_CONFIG_VALUE_0 = 'private-setting-value' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'safe.bareRepository'; GIT_CONFIG_VALUE_0 = 'all' },
            [PSCustomObject]@{ GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'credential.interactive'; GIT_CONFIG_VALUE_0 = 'true' }
        )
        foreach ($environment in $invalid) {
            $target = Get-EnvironmentTarget $environment
            $failure = $null
            try { Resolve-XrayMcpEnvironment @($target) }
            catch { $failure = $_.Exception.Message }
            Assert-True ($null -ne $failure) 'Invalid Git environment rejected'
            Assert-True ($failure -notlike '*private-setting-value*') 'Private Git value not disclosed'
            Assert-True (-not $target.PSObject.Properties['Environment']) 'Rejected environment was not applied'
        }
        $first = Get-EnvironmentTarget ([PSCustomObject]@{
            GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'http.extraHeader'; GIT_CONFIG_VALUE_0 = 'private-setting-value'
        })
        $second = Get-EnvironmentTarget ([PSCustomObject]@{
            GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = 'http.extraHeader'; GIT_CONFIG_VALUE_0 = 'other-private-value'
        }) -Path 'other.json'
        $failure = $null
        try { Resolve-XrayMcpEnvironment @($first, $second) }
        catch { $failure = $_.Exception.Message }
        Assert-True ($failure -like '*Conflicting numbered Git overrides*') 'Different ordered overrides refused'
        Assert-True ($failure -notmatch 'private-setting-value|other-private-value') 'Conflicting values not disclosed'
    }

    Test-Case 'Git environment accepts Git key grammar for subsection names and empty subsections' {
        foreach ($key in @('1section.name', 'section..name', 'http.https://example.invalid/.extraHeader')) {
            $target = Get-EnvironmentTarget ([PSCustomObject]@{
                GIT_CONFIG_COUNT = '1'; GIT_CONFIG_KEY_0 = $key; GIT_CONFIG_VALUE_0 = 'value'
            })
            Resolve-XrayMcpEnvironment @($target)
            Assert-True ($target.Environment.GIT_CONFIG_KEY_3 -ceq $key) 'Valid Git key retained exactly'
        }
    }

    if ($UserMcpOnly -and -not $XrayBinary) { throw '-UserMcpOnly requires -XrayBinary.' }
    if ($XrayBinary) {
        Test-Case 'caller continuation for inline personal setup plan restore and uninstall' {
            $f = New-UserMcpFixture
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ PlanOnly = $true } -Inline)
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ Extensions = 'rs' } -Inline)
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ Extensions = 'ts' } -Inline)
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ Restore = $true } -Inline)
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ Uninstall = $true; DryRun = $true } -Inline)
            Assert-SetupReturned (Invoke-UserMcpSetup $f @{ Uninstall = $true } -Inline)
        }

        Test-Case 'personal Git environment upgrades both hosts and survives ordinary and preserving updates' {
            foreach ($preserve in @($false, $true)) {
                $f = New-UserMcpFixture
                Assert-Success (Invoke-UserMcpSetup $f)
                $cli = Join-Path $f.Home 'mcp-config.json'
                $vscode = Join-Path $f.Home 'vscode\mcp.json'
                foreach ($path in @($cli, $vscode)) {
                    $doc = Get-FixtureJson $path
                    $entry = if ($path -eq $cli) { $doc.mcpServers.xray } else { $doc.servers.xray }
                    Assert-XrayGitEnvironment $entry.env
                    $environment = [ordered]@{ CUSTOM = $path }
                    if ($path -eq $cli) {
                        $environment.GIT_CONFIG_COUNT = '1'
                        $environment.GIT_CONFIG_KEY_0 = 'core.quotePath'
                        $environment.GIT_CONFIG_VALUE_0 = 'false'
                    }
                    $entry.env = [PSCustomObject]$environment
                    Write-FixtureFile $path ($doc | ConvertTo-Json -Depth 20)
                }
                Assert-Success (Invoke-UserMcpSetup $f @{ PreserveSettings = $preserve })
                foreach ($path in @($cli, $vscode)) {
                    $doc = Get-FixtureJson $path
                    $entry = if ($path -eq $cli) { $doc.mcpServers.xray } else { $doc.servers.xray }
                    Assert-XrayGitEnvironment $entry.env 4
                    Assert-True ($entry.env.CUSTOM -ceq $path) 'Host-specific environment retained'
                    Assert-True ($entry.env.GIT_CONFIG_KEY_3 -ceq 'core.quotePath' -and
                        $entry.env.GIT_CONFIG_VALUE_3 -ceq 'false') 'Additional Git override shared'
                }
                $before = @{}
                foreach ($path in @($cli, $vscode, "$cli.bak", "$vscode.bak")) {
                    $before[$path] = (Get-FileHash -LiteralPath $path).Hash
                }
                Assert-Success (Invoke-UserMcpSetup $f @{ PreserveSettings = $preserve })
                foreach ($path in $before.Keys) {
                    Assert-True ((Get-FileHash -LiteralPath $path).Hash -ceq $before[$path]) 'Repeat setup preserves config and backup bytes'
                }
            }
        }

        Test-Case 'personal Git environment conflict and plan-only stop before profile or config writes' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $cli = Join-Path $f.Home 'mcp-config.json'
            $vscode = Join-Path $f.Home 'vscode\mcp.json'
            $profilePath = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            foreach ($malformedKey in @($false, $true)) {
                $doc = Get-FixtureJson $cli
                if ($malformedKey) {
                    $doc.mcpServers.xray.env.GIT_CONFIG_VALUE_0 = 'explicit'
                    $doc.mcpServers.xray.env.GIT_CONFIG_COUNT = '4'
                    $doc.mcpServers.xray.env | Add-Member GIT_CONFIG_KEY_3 'invalid-key'
                    $doc.mcpServers.xray.env | Add-Member GIT_CONFIG_VALUE_3 'private-setting-value'
                }
                else { $doc.mcpServers.xray.env.GIT_CONFIG_VALUE_0 = 'all' }
                Write-FixtureFile $cli ($doc | ConvertTo-Json -Depth 20)
                $before = @{}
                foreach ($path in @($cli, $vscode, $profilePath)) { $before[$path] = (Get-FileHash -LiteralPath $path).Hash }
                foreach ($plan in @($false, $true)) {
                    $result = Invoke-UserMcpSetup $f @{ PreserveSettings = $true; PlanOnly = $plan }
                    $message = if ($malformedKey) { '*Invalid Git configuration key or value*' } else { '*Conflicting Xray Git setting*' }
                    Assert-True ($result.ExitCode -ne 0 -and $result.Output -like $message) 'Conflict or malformed key explicitly refused'
                    Assert-True ($result.Output -notlike '*private-setting-value*') 'Malformed value not disclosed'
                    foreach ($path in $before.Keys) {
                        Assert-True ((Get-FileHash -LiteralPath $path).Hash -ceq $before[$path]) 'Conflict preserves profile and client bytes'
                        Assert-True (-not (Test-Path -LiteralPath "$path.bak")) 'Conflict does not create a backup'
                    }
                }
            }
        }

        Test-Case 'personal Git environment admits simultaneous hosts with different inherited Git overrides' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $entries = @(
                (Get-FixtureJson (Join-Path $f.Home 'mcp-config.json')).mcpServers.xray,
                (Get-FixtureJson (Join-Path $f.Home 'vscode\mcp.json')).servers.xray
            )
            $clients = @()
            $inputEncoding = [Console]::InputEncoding
            try {
                # .NET Framework uses the console encoding, including its BOM, for redirected stdin.
                [Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
                for ($i = 0; $i -lt $entries.Count; $i++) {
                    $entry = $entries[$i]
                    $info = New-Object Diagnostics.ProcessStartInfo
                    $info.FileName = $entry.command
                    $info.Arguments = ($entry.args | ForEach-Object { '"' + $_ + '"' }) -join ' '
                    $info.WorkingDirectory = $f.Repo
                    $info.UseShellExecute = $false
                    $info.RedirectStandardInput = $true
                    $info.RedirectStandardOutput = $true
                    $info.RedirectStandardError = $true
                    foreach ($key in @($info.EnvironmentVariables.Keys)) {
                        if ($key -match '^GIT_CONFIG_(COUNT$|KEY_|VALUE_)') { $info.EnvironmentVariables.Remove($key) }
                    }
                    if ($i -eq 1) {
                        $info.EnvironmentVariables['GIT_CONFIG_COUNT'] = '1'
                        $info.EnvironmentVariables['GIT_CONFIG_KEY_0'] = 'safe.bareRepository'
                        $info.EnvironmentVariables['GIT_CONFIG_VALUE_0'] = 'explicit'
                    }
                    foreach ($property in $entry.env.PSObject.Properties) {
                        $info.EnvironmentVariables[$property.Name] = $property.Value
                    }
                    $info.EnvironmentVariables['XRAY_BACKEND_IDLE_SECONDS'] = '1'
                    $info.EnvironmentVariables['XRAY_GUIDANCE_PREFIX'] = '0'
                    $client = New-Object Diagnostics.Process
                    $client.StartInfo = $info
                    [void]$client.Start()
                    $clients += $client
                    $null = $client.StandardError.ReadToEndAsync()
                    $client.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"installer-test","version":"1"}}}')
                    $response = Receive-FixtureMcpResponse $client 1
                    Assert-True ($null -eq $response.error -and $null -ne $response.result.serverInfo) 'Host initialized'
                    $client.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
                }
                foreach ($client in $clients) {
                    $client.StandardInput.WriteLine('{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"xray_info","arguments":{}}}')
                    $response = Receive-FixtureMcpResponse $client 2
                    Assert-True ($null -eq $response.error -and -not $response.result.isError -and
                        @($response.result.content).Count -gt 0) 'Both hosts can call xray_info while connected'
                }
            }
            finally {
                [Console]::InputEncoding = $inputEncoding
                foreach ($client in $clients) {
                    $client.StandardInput.Close()
                    if (-not $client.WaitForExit(10000)) { $client.Kill(); $client.WaitForExit() }
                    $client.Dispose()
                }
                $timer = [Diagnostics.Stopwatch]::StartNew()
                do {
                    $running = @(Get-Process -Name xray -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $f.Install 'xray.exe') })
                    if ($running.Count -eq 0) { break }
                    Start-Sleep -Milliseconds 100
                } while ($timer.ElapsedMilliseconds -lt 10000)
                Assert-True ($running.Count -eq 0) 'Disposable backend stopped after its idle timeout'
            }
        }

        Test-Case 'personal profile names are readable stable bounded and distinct' {
            $profiles = 'C:\Profiles'
            $path = Get-WorkspaceProfilePath $profiles 'C:\Repos\MyProject'
            Assert-True ($path -ceq 'C:\Profiles\c_repos_myproject-22bf3ee8f773\config.json') 'Readable path with the expected hash suffix'
            Assert-True ((Get-WorkspaceProfilePath $profiles 'C:\REPOS\MyProject\') -ceq $path) 'Case and trailing separators do not change the name'
            Assert-True ((Get-WorkspaceProfilePath $profiles 'C:\Repos\unused\..\MyProject') -ceq $path) 'Dot components are normalized'
            $unc = Get-WorkspaceProfilePath $profiles '\\server\share\My Project'
            Assert-True ((Split-Path -Leaf (Split-Path -Parent $unc)) -cmatch '^server_share_my_project-[a-f0-9]{12}$') 'UNC paths and spaces produce safe readable names'
            $a = Get-WorkspaceProfilePath $profiles 'C:\Repos\foo_bar'
            $b = Get-WorkspaceProfilePath $profiles 'C:\Repos\foo\bar'
            Assert-True ($a -cne $b) 'Different roots with the same readable prefix remain distinct'
            $long = Get-WorkspaceProfilePath $profiles ('C:\Repos\' + ('a' * 180))
            $name = Split-Path -Leaf (Split-Path -Parent $long)
            Assert-True ($name.Length -le 77 -and $name -cmatch '^[a-z0-9_]+-[a-f0-9]{12}$') 'Long paths produce bounded safe directory names'
        }

        Test-Case 'personal profile migration renames only the selected workspace and preserves backups' {
            $a = New-UserMcpFixture
            $b = New-Fixture
            $b.Install = $a.Install
            $b.Home = $a.Home
            Assert-Success (Invoke-UserMcpSetup $a)
            Assert-Success (Invoke-UserMcpSetup $b @{ Extensions = 'rs' })
            $profiles = Join-Path $a.Install 'workspaces'
            $aPath = Get-WorkspaceProfilePath $profiles $a.Repo
            $bPath = Get-WorkspaceProfilePath $profiles $b.Repo
            $before = [IO.File]::ReadAllText($aPath)
            $backup = $before.Replace('cs,md', 'ts')
            Write-FixtureFile "$aPath.bak" $backup
            Write-FixtureFile (Join-Path (Split-Path -Parent $aPath) 'keep.txt') 'preserve this file'
            $aLegacy = Move-FixtureProfileToLegacy $a
            $bLegacy = Move-FixtureProfileToLegacy $b
            $cli = Join-Path $a.Home 'mcp-config.json'
            $vscode = Join-Path $a.Home 'vscode\mcp.json'
            $unchanged = @{}
            foreach ($path in @($bLegacy, $cli, $vscode)) { $unchanged[$path] = (Get-FileHash $path).Hash }
            Assert-Success (Invoke-UserMcpSetup $a)
            Assert-True (-not (Test-Path -LiteralPath (Split-Path -Parent $aLegacy))) 'Old selected workspace directory is gone'
            Assert-True ([IO.File]::ReadAllText($aPath) -ceq $before) 'Renaming preserves unchanged profile bytes'
            Assert-True ([IO.File]::ReadAllText("$aPath.bak") -ceq $backup) 'Existing backup follows the profile without being overwritten'
            Assert-True ([IO.File]::ReadAllText((Join-Path (Split-Path -Parent $aPath) 'keep.txt')) -ceq 'preserve this file') 'Other directory contents are preserved'
            foreach ($path in $unchanged.Keys) { Assert-True ((Get-FileHash $path).Hash -ceq $unchanged[$path]) "Unrelated profile or shared registration unchanged: $path" }
            Assert-True (@(Get-WorkspaceProfileFiles $profiles).Count -eq 2) 'Migration leaves one profile per workspace'
            $time = (Get-Item -LiteralPath $aPath).LastWriteTimeUtc
            Assert-Success (Invoke-UserMcpSetup $a)
            Assert-True ((Get-Item -LiteralPath $aPath).LastWriteTimeUtc -eq $time) 'Repeated setup does not rewrite the migrated profile'
            Assert-True (-not (Test-Path -LiteralPath $aLegacy)) 'Repeated setup does not recreate the old profile'
        }

        Test-Case 'personal profile legacy restore and uninstall preserve lifecycle behavior' {
            foreach ($action in @('restore', 'uninstall')) {
                $f = New-UserMcpFixture
                Assert-Success (Invoke-UserMcpSetup $f)
                $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                $before = [IO.File]::ReadAllText($profile)
                Assert-Success (Invoke-UserMcpSetup $f @{ Extensions = 'ts' })
                $legacy = Move-FixtureProfileToLegacy $f
                $cli = Join-Path $f.Home 'mcp-config.json'
                $registration = [IO.File]::ReadAllText($cli)
                if ($action -eq 'restore') {
                    Assert-Success (Invoke-UserMcpSetup $f @{ Restore = $true })
                    Assert-True ([IO.File]::ReadAllText($profile) -ceq $before) 'Restore migrates the directory and restores the old settings'
                    Assert-True (-not (Test-Path -LiteralPath $legacy)) 'Restore does not leave the old profile'
                    Assert-True ([IO.File]::ReadAllText($cli) -ceq $registration) 'Restore leaves the shared registration unchanged'
                }
                else {
                    $paths = @($legacy, "$legacy.bak", $cli, (Join-Path $f.Home 'vscode\mcp.json'))
                    $hashes = @{}
                    foreach ($path in $paths) { $hashes[$path] = (Get-FileHash $path).Hash }
                    Assert-Success (Invoke-UserMcpSetup $f @{ Uninstall = $true; DryRun = $true })
                    foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $hashes[$path]) "Dry run preserves legacy state: $path" }
                    Assert-True (-not (Test-Path -LiteralPath $profile)) 'Dry run does not create a readable profile'
                    Assert-Success (Invoke-UserMcpSetup $f @{ Uninstall = $true })
                    Assert-True (-not (Test-Path -LiteralPath $legacy) -and -not (Test-Path -LiteralPath "$legacy.bak")) 'Uninstall removes the old profile and backup'
                    Assert-True (-not (Test-Path -LiteralPath $profile)) 'Uninstall does not create a replacement profile'
                    Assert-True (-not (Get-FixtureJson $cli).mcpServers.PSObject.Properties['xray']) 'Last legacy profile uninstall removes the managed registration'
                }
            }
        }

        Test-Case 'personal profile migration refuses wrong roots and duplicate profiles without writes' {
            foreach ($kind in @('legacy-root', 'readable-root', 'backup-root', 'duplicate')) {
                $f = New-UserMcpFixture
                Assert-Success (Invoke-UserMcpSetup $f)
                $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                $original = [IO.File]::ReadAllText($profile)
                $legacy = if ($kind -eq 'readable-root') { $null } else { Move-FixtureProfileToLegacy $f }
                $path = if ($legacy) { $legacy } else { $profile }
                if ($kind -eq 'duplicate') { Write-FixtureFile $profile $original }
                else {
                    $changed = Get-FixtureJson $path
                    $changed.workspaceRoot = Join-Path $f.Repo 'other'
                    if ($kind -eq 'backup-root') { $path = "$path.bak" }
                    Write-FixtureFile $path ($changed | ConvertTo-Json -Depth 10)
                }
                $files = @(Get-ChildItem -LiteralPath $f.Install, $f.Home -Recurse -File)
                $before = @{}
                foreach ($file in $files) { $before[$file.FullName] = (Get-FileHash $file.FullName).Hash }
                foreach ($options in @(@{}, @{ Restore = $true }, @{ Uninstall = $true }, @{ Uninstall = $true; DryRun = $true })) {
                    $result = Invoke-UserMcpSetup $f $options
                    Assert-True ($result.ExitCode -ne 0 -and $result.Output -match 'workspace profile') "Unsafe profile operation explicitly refused: $kind"
                    foreach ($file in $before.Keys) { Assert-True ((Get-FileHash $file).Hash -ceq $before[$file]) "Refusal preserves bytes: $kind, $file" }
                    Assert-True (@(Get-ChildItem -LiteralPath $f.Install, $f.Home -Recurse -File).Count -eq $files.Count) 'Refusal creates no files or backups'
                }
            }
        }

        Test-Case 'personal profile recovery restores corrupt or missing current JSON from a matching backup' {
            foreach ($legacy in @($false, $true)) {
                foreach ($inline in @($false, $true)) {
                    $f = New-UserMcpFixture
                    Assert-Success (Invoke-UserMcpSetup $f)
                    $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                    $original = [IO.File]::ReadAllText($profile)
                    Write-FixtureFile "$profile.bak" $original
                    $current = if ($legacy) { Move-FixtureProfileToLegacy $f } else { $profile }
                    $corrupt = if ($inline) { '' } elseif ($legacy) { '[]' } else { '{' }
                    Write-FixtureFile $current $corrupt
                    $cli = Join-Path $f.Home 'mcp-config.json'
                    $vscode = Join-Path $f.Home 'vscode\mcp.json'
                    $before = @{}
                    foreach ($path in @($cli, $vscode)) { $before[$path] = (Get-FileHash $path).Hash }
                    $result = Invoke-UserMcpSetup $f @{ Restore = $true } -Inline:$inline
                    Assert-Success $result
                    Assert-True ([regex]::Matches($result.Output, 'Invalid workspace profile JSON').Count -eq 1) 'Recovery warns once about the unchanged corrupt current profile'
                    Assert-True ([IO.File]::ReadAllText($profile) -ceq $original) 'Exact matching backup restored over corrupt JSON'
                    Assert-True (-not (Test-Path -LiteralPath "$profile.bak")) 'Successful restore consumes the backup'
                    if ($legacy) { Assert-True (-not (Test-Path -LiteralPath (Split-Path -Parent $current))) 'Restore migrates the old directory without leaving a duplicate' }
                    foreach ($path in $before.Keys) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) 'Recovery leaves shared registrations unchanged' }
                    Write-FixtureFile "$profile.bak" $original
                    Remove-Item -LiteralPath $profile -Force
                    if ($legacy) { $null = Move-FixtureProfileToLegacy $f }
                    Assert-Success (Invoke-UserMcpSetup $f @{ Restore = $true } -Inline:$inline)
                    Assert-True ([IO.File]::ReadAllText($profile) -ceq $original) 'Backup-only directory restores without requiring a current profile'
                }
            }
        }

        Test-Case 'personal profile recovery uninstall handles corrupt JSON and preserves dry run and other workspaces' {
            foreach ($legacy in @($false, $true)) {
                foreach ($damage in @('current', 'backup', 'both')) {
                    $f = New-UserMcpFixture
                    $other = New-Fixture
                    $other.Install = $f.Install
                    $other.Home = $f.Home
                    Assert-Success (Invoke-UserMcpSetup $f)
                    Assert-Success (Invoke-UserMcpSetup $other)
                    $profiles = Join-Path $f.Install 'workspaces'
                    $readable = Get-WorkspaceProfilePath $profiles $f.Repo
                    $otherProfile = Get-WorkspaceProfilePath $profiles $other.Repo
                    $original = [IO.File]::ReadAllText($readable)
                    Write-FixtureFile "$readable.bak" $original
                    $profile = if ($legacy) { Move-FixtureProfileToLegacy $f } else { $readable }
                    if ($damage -in @('current', 'both')) { Write-FixtureFile $profile $(if ($damage -eq 'both') { '[]' } else { '{' }) }
                    if ($damage -in @('backup', 'both')) { Write-FixtureFile "$profile.bak" $(if ($damage -eq 'both') { '' } else { '{' }) }
                    $cli = Join-Path $f.Home 'mcp-config.json'
                    $vscode = Join-Path $f.Home 'vscode\mcp.json'
                    $paths = @($profile, "$profile.bak", $otherProfile, $cli, $vscode)
                    $before = @{}
                    foreach ($path in $paths) { $before[$path] = (Get-FileHash $path).Hash }
                    $result = Invoke-UserMcpSetup $f @{ Uninstall = $true; DryRun = $true } -Inline:$legacy
                    Assert-Success $result
                    Assert-True ($result.Output -match 'Invalid workspace profile JSON') 'Dry run reports malformed profile JSON explicitly'
                    foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) 'Dry run preserves all profile and client bytes' }
                    if ($legacy) { Assert-True (-not (Test-Path -LiteralPath $readable)) 'Dry run does not migrate a corrupt old profile' }
                    $result = Invoke-UserMcpSetup $f @{ Uninstall = $true } -Inline:(-not $legacy)
                    Assert-Success $result
                    Assert-True ($result.Output -match 'Invalid workspace profile JSON') 'Uninstall reports malformed profile JSON explicitly'
                    Assert-True (-not (Test-Path -LiteralPath $profile) -and -not (Test-Path -LiteralPath "$profile.bak")) 'Uninstall removes corrupt profile and backup'
                    foreach ($path in @($otherProfile, $cli, $vscode)) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) 'Other workspace and shared registrations remain unchanged' }
                    Write-FixtureFile $profile '{'
                    Write-FixtureFile "$profile.bak" '{'
                    Assert-Success (Invoke-UserMcpSetup $other @{ Uninstall = $true })
                    Assert-Success (Invoke-UserMcpSetup $f @{ Uninstall = $true; KeepBackups = $true })
                    Assert-True (-not (Test-Path -LiteralPath $profile) -and [IO.File]::ReadAllText("$profile.bak") -ceq '{') 'KeepBackups retains the corrupt backup without blocking profile removal'
                    Assert-True (-not (Get-FixtureJson $cli).mcpServers.PSObject.Properties['xray']) 'Last corrupt profile removal cleans shared registration'
                }
            }
        }

        Test-Case 'personal profile recovery setup tolerates corrupt backups but restore refuses them without writes' {
            foreach ($legacy in @($false, $true)) {
                $f = New-UserMcpFixture
                Assert-Success (Invoke-UserMcpSetup $f)
                $readable = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                $original = [IO.File]::ReadAllText($readable)
                $badBackup = if ($legacy) { '[]' } else { '{' }
                Write-FixtureFile "$readable.bak" $badBackup
                $profile = if ($legacy) { Move-FixtureProfileToLegacy $f } else { $readable }
                $cli = Join-Path $f.Home 'mcp-config.json'
                $paths = @($profile, "$profile.bak", $cli)
                $before = @{}
                foreach ($path in $paths) { $before[$path] = (Get-FileHash $path).Hash }
                $result = Invoke-UserMcpSetup $f @{ Restore = $true } -Inline:$legacy
                Assert-True ($result.ExitCode -ne 0) 'Restore refuses a corrupt backup explicitly'
                foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) 'Failed restore preserves existing profile, backup and client bytes' }
                if ($legacy) { Assert-True (-not (Test-Path -LiteralPath $readable)) 'Failed restore does not rename the old directory' }
                $result = Invoke-UserMcpSetup $f -Inline:$legacy
                Assert-Success $result
                Assert-True ([regex]::Matches($result.Output, 'Invalid workspace profile JSON').Count -eq 1) 'Setup warns once about the unchanged unused corrupt backup'
                Assert-True ([IO.File]::ReadAllText($readable) -ceq $original) 'Unchanged setup preserves current profile bytes'
                Assert-True ([IO.File]::ReadAllText("$readable.bak") -ceq $badBackup) 'Unchanged setup does not overwrite the unused corrupt backup'
                Assert-Success (Invoke-UserMcpSetup $f @{ Extensions = 'ts' } -Inline:$legacy)
                Assert-True ([IO.File]::ReadAllText("$readable.bak") -ceq $original) 'Changed setup replaces the stale backup with the approved current profile'
                Assert-True ((Get-FixtureJson $readable).args[1] -eq 'ts') 'Setup applies requested settings despite a stale corrupt backup'
            }
        }

        Test-Case 'personal profile recovery setup preserves a good backup when the current profile is corrupt' {
            foreach ($legacy in @($false, $true)) {
                foreach ($inline in @($false, $true)) {
                    $f = New-UserMcpFixture
                    Assert-Success (Invoke-UserMcpSetup $f)
                    $readable = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                    $original = [IO.File]::ReadAllText($readable)
                    Write-FixtureFile "$readable.bak" $original
                    $profile = if ($legacy) { Move-FixtureProfileToLegacy $f } else { $readable }
                    Write-FixtureFile $profile '{'
                    $paths = @($profile, "$profile.bak", (Join-Path $f.Home 'mcp-config.json'), (Join-Path $f.Home 'vscode\mcp.json'))
                    $before = @{}
                    foreach ($path in $paths) { $before[$path] = (Get-FileHash $path).Hash }
                    $result = Invoke-UserMcpSetup $f @{ Extensions = 'ts' } -Inline:$inline
                    Assert-True ($result.ExitCode -ne 0) 'Setup refuses a corrupt current profile rather than replacing the good backup'
                    foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) 'Refused setup preserves profile, backup and shared registrations' }
                    if ($legacy) { Assert-True (-not (Test-Path -LiteralPath (Split-Path -Parent $readable))) 'Refused setup does not rename the old profile directory' }
                }
            }
        }

        Test-Case 'personal profile recovery warning deduplication does not skip revalidation' {
            $f = New-Fixture
            $profiles = Join-Path $f.Install 'workspaces'
            $profile = Get-WorkspaceProfilePath $profiles $f.Repo
            $text = @{ version = 1; workspaceRoot = $f.Repo; args = @() } | ConvertTo-Json -Depth 8
            Write-FixtureFile $profile $text
            Write-FixtureFile "$profile.bak" '{'
            $reported = @{}
            $first = @(Resolve-WorkspaceProfile $profiles $f.Repo -ReportedWarnings $reported 3>&1)
            $second = @(Resolve-WorkspaceProfile $profiles $f.Repo -ReportedWarnings $reported 3>&1)
            Assert-True (@($first | Where-Object { $_ -is [Management.Automation.WarningRecord] }).Count -eq 1) 'First validation reports malformed JSON'
            Assert-True (@($second | Where-Object { $_ -is [Management.Automation.WarningRecord] }).Count -eq 0) 'Unchanged malformed JSON is not reported twice'
            Write-FixtureFile "$profile.bak" '['
            $changed = @(Resolve-WorkspaceProfile $profiles $f.Repo -ReportedWarnings $reported 3>&1)
            Assert-True (@($changed | Where-Object { $_ -is [Management.Automation.WarningRecord] }).Count -eq 1) 'New malformed contents are still reported'
            $foreign = @{ version = 1; workspaceRoot = (Join-Path $f.Repo 'other'); args = @() } | ConvertTo-Json -Depth 8
            Write-FixtureFile "$profile.bak" $foreign
            $rejected = $false
            try { $null = Resolve-WorkspaceProfile $profiles $f.Repo -ReportedWarnings $reported }
            catch { $rejected = $_.Exception.Message -like '*Invalid workspace profile root*' }
            Assert-True $rejected 'Repeated validation still refuses a replacement backup belonging to another workspace'
            Assert-True ([IO.File]::ReadAllText($profile) -ceq $text -and [IO.File]::ReadAllText("$profile.bak") -ceq $foreign) 'Repeated validation leaves both files untouched'
        }

        Test-Case 'personal profile migration refuses an occupied destination' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            $legacy = Move-FixtureProfileToLegacy $f
            $before = [IO.File]::ReadAllText($legacy)
            $foreign = Join-Path (Split-Path -Parent $profile) 'keep.txt'
            Write-FixtureFile $foreign 'unrelated directory'
            $result = Invoke-UserMcpSetup $f
            Assert-True ($result.ExitCode -ne 0) 'Occupied destination refuses migration'
            Assert-True ([IO.File]::ReadAllText($legacy) -ceq $before) 'Failed rename preserves the old profile'
            Assert-True ([IO.File]::ReadAllText($foreign) -ceq 'unrelated directory') 'Occupied destination is not replaced or merged'
        }

        Test-Case 'personal profile migration waits for client and executable preflight' {
            foreach ($failure in @('client', 'executable')) {
                $f = New-UserMcpFixture
                Assert-Success (Invoke-UserMcpSetup $f)
                $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
                $legacy = Move-FixtureProfileToLegacy $f
                $before = [IO.File]::ReadAllText($legacy)
                if ($failure -eq 'client') { Write-FixtureFile (Join-Path $f.Home 'mcp-config.json') '{' }
                else { Write-FixtureFile (Join-Path $f.Install 'xray.exe') 'unsupported executable' }
                $result = Invoke-UserMcpSetup $f
                Assert-True ($result.ExitCode -ne 0) "Preflight fails: $failure"
                Assert-True ([IO.File]::ReadAllText($legacy) -ceq $before) "Legacy profile untouched before successful preflight: $failure"
                Assert-True (-not (Test-Path -LiteralPath $profile)) "No new profile on failed preflight: $failure"
            }
        }

        Test-Case 'personal lifecycle rejects restore and uninstall together before writes' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $profilePath = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            Write-FixtureFile "$profilePath.bak" ([IO.File]::ReadAllText($profilePath))
            $workspaceCli = Join-Path $f.Repo '.github\mcp.json'
            $workspaceVSCode = Join-Path $f.Repo '.vscode\mcp.json'
            Write-FixtureFile $workspaceCli '{"mcpServers":{"other":{"command":"other.exe"}}}'
            Write-FixtureFile $workspaceVSCode '{"servers":{"other":{"command":"other.exe"}}}'
            Write-FixtureFile "$workspaceCli.bak" '{"mcpServers":{"xray":{"command":"old-xray.exe"},"other":{"command":"other.exe"}}}'
            Write-FixtureFile "$workspaceVSCode.bak" '{"servers":{"xray":{"command":"old-xray.exe"},"other":{"command":"other.exe"}}}'
            $paths = @($profilePath, "$profilePath.bak", $workspaceCli, "$workspaceCli.bak", $workspaceVSCode, "$workspaceVSCode.bak",
                (Join-Path $f.Home 'mcp-config.json'), (Join-Path $f.Home 'vscode\mcp.json'),
                (Join-Path $f.Repo '.git\index'), (Join-Path $f.Repo '.git\info\exclude'),
                (Join-Path $f.Repo '.git\config'), (Join-Path $f.Install 'xray.exe'))
            $before = @{}
            foreach ($path in $paths) { $before[$path] = (Get-FileHash $path).Hash }
            foreach ($personal in @($false, $true)) {
                foreach ($dryRun in @($false, $true)) {
                    foreach ($inline in @($false, $true)) {
                        $options = @{ Restore = $true; Uninstall = $true; DryRun = $dryRun }
                        if ($personal) {
                            $options.UserMcp = $true
                            $options.VSCodeMcpPath = Join-Path $f.Home 'vscode\mcp.json'
                        }
                        $result = Invoke-FixtureSetup $f $options -Inline:$inline
                        $label = "UserMcp=$personal, DryRun=$dryRun, Inline=$inline"
                        Assert-True ($result.ExitCode -ne 0 -and $result.Output -like '*-Restore and -Uninstall cannot be used together*') "Incompatible actions rejected: $label"
                        foreach ($path in $paths) {
                            Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) "Incompatible actions preserve $path`: $label"
                        }
                    }
                }
            }
        }

        Test-Case 'personal upgrade refuses workspace duplicates from CLI and active VS Code profiles' {
            foreach ($source in @('cli', 'vscode', 'named-profile', 'inherited-profile', 'foreign-cli', 'foreign-vscode')) {
                $modes = @(@{}, @{ EnableVSCode = $true })
                if ($source -in @('cli', 'foreign-cli')) { $modes += @{ EnableCopilotCli = $true } }
                foreach ($mode in $modes) {
                    $f = New-UserMcpFixture
                    $user = Join-Path $f.Home 'appdata\Code\User'
                    $personal = Join-Path $user 'mcp.json'
                    $container = 'servers'
                    if ($source -in @('cli', 'foreign-cli')) {
                        $personal = Join-Path $f.Home 'mcp-config.json'
                        $container = 'mcpServers'
                    }
                    if ($source -in @('named-profile', 'inherited-profile')) {
                        $associations = @{}
                        $associations[([Uri]($f.Repo + '\')).AbsoluteUri.TrimEnd('/')] = 'named-profile'
                        $state = @{ profileAssociations = @{ workspaces = $associations }; userDataProfiles = @(@{ location = 'named-profile'; useDefaultFlags = @{ mcp = ($source -eq 'inherited-profile') } }) }
                        Write-FixtureFile (Join-Path $user 'globalStorage\storage.json') ($state | ConvertTo-Json -Depth 8)
                        if ($source -eq 'named-profile') { $personal = Join-Path $user 'profiles\named-profile\mcp.json' }
                    }
                    $binary = if ($source -like 'foreign-*') { 'foreign.exe' } else { Join-Path $f.Install 'xray.exe' }
                    $text = @{ $container = @{ xray = @{ command = $binary; args = @('serve-workspace', '--profiles', (Join-Path $f.Install 'workspaces')); _xrayUserMcp = 1 }; other = @{ command = 'other.exe' } } } | ConvertTo-Json -Depth 8
                    Write-FixtureFile $personal $text
                    $paths = @($personal, (Join-Path $f.Repo '.git\index'), (Join-Path $f.Repo '.git\config'),
                        (Join-Path $f.Repo '.git\info\exclude'), (Join-Path $f.Install 'xray.exe'))
                    $before = @{}
                    foreach ($path in $paths) { $before[$path] = if (Test-Path $path) { (Get-FileHash $path).Hash } else { $null } }
                    $result = Invoke-FixtureSetup $f $mode
                    Assert-True ($result.ExitCode -ne 0 -and $result.Output -like '*-UserMcp*' -and $result.Output -like '*-MigrateLegacy*') "Workspace setup rejects personal registration with migration guidance: $source"
                    foreach ($path in $paths) {
                        $after = if (Test-Path $path) { (Get-FileHash $path).Hash } else { $null }
                        Assert-True ($after -ceq $before[$path]) "Preflight preserves $path"
                    }
                    Assert-True (-not (Test-Path (Join-Path $f.Repo '.github\mcp.json')) -and -not (Test-Path (Join-Path $f.Repo '.vscode\mcp.json'))) 'No workspace duplicate created'
                    Assert-True (-not (Test-Path (Join-Path $f.Install 'workspaces')) -and -not (Test-Path "$personal.bak")) 'No profile or personal backup written on conflict'
                }
            }
        }

        Test-Case 'personal upgrade migrates an old workspace install and blocks ordinary reinstall or restore' {
            $f = New-UserMcpFixture
            $corporate = Join-Path $f.Repo '.mcp.json'
            $corporateText = '{"mcpServers":{"corporate":{"command":"corporate.exe"}}}'
            Write-FixtureFile $corporate $corporateText
            [void](Invoke-SetupGit $f.Repo @('add', '--', '.mcp.json'))
            [void](Invoke-SetupGit $f.Repo @('commit', '--quiet', '-m', 'corporate MCP'))
            $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
            $workspaceCli = Join-Path $f.Repo '.github\mcp.json'
            $workspaceVSCode = Join-Path $f.Repo '.vscode\mcp.json'
            Write-FixtureFile $workspaceCli '{"mcpServers":{"other":{"command":"other.exe"}}}'
            Write-FixtureFile $workspaceVSCode '{"servers":{"other":{"command":"other.exe"}}}'
            Assert-Success (Invoke-FixtureSetup $f)
            Assert-True ((Get-FixtureJson $workspaceCli).mcpServers.xray.command -eq (Join-Path $f.Install 'xray.exe')) 'Old workspace CLI installation created'
            Assert-True ((Get-FixtureJson $workspaceVSCode).servers.xray.command -eq (Join-Path $f.Install 'xray.exe')) 'Old workspace VS Code installation created'
            Assert-True (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd')) 'Old CLI launcher exists before migration'
            $personalVSCode = Join-Path $f.Home 'appdata\Code\User\mcp.json'
            $parameters = @{ VSCodeMcpPath = $personalVSCode }
            $result = Invoke-UserMcpSetup $f $parameters
            Assert-True ($result.ExitCode -ne 0) 'Upgrade requires explicit workspace migration consent'
            Assert-True (-not (Test-Path (Join-Path $f.Home 'mcp-config.json'))) 'Refused migration creates no personal registration'
            $parameters.MigrateLegacy = $true
            Assert-Success (Invoke-UserMcpSetup $f $parameters)
            $cli = Get-FixtureJson $workspaceCli
            $vscode = Get-FixtureJson $workspaceVSCode
            Assert-True (-not $cli.mcpServers.PSObject.Properties['xray'] -and $cli.mcpServers.other.command -eq 'other.exe') 'Migrated CLI workspace keeps only unrelated servers'
            Assert-True (-not $vscode.servers.PSObject.Properties['xray'] -and $vscode.servers.other.command -eq 'other.exe') 'Migrated VS Code workspace keeps only unrelated servers'
            Assert-True (-not (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd'))) 'Migration removes old CLI launcher'
            Assert-True ([IO.File]::ReadAllText($corporate) -ceq $corporateText) 'Corporate config byte-identical after upgrade'
            Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Git index byte-identical after upgrade'
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            $personalCli = Join-Path $f.Home 'mcp-config.json'
            $paths = @($corporate, $workspaceCli, $workspaceVSCode, "$workspaceCli.bak", "$workspaceVSCode.bak",
                $profile, $personalCli, $personalVSCode, (Join-Path $f.Repo '.git\index'),
                (Join-Path $f.Repo '.git\info\exclude'), (Join-Path $f.Repo '.git\config'), (Join-Path $f.Install 'xray.exe'))
            $before = @{}
            foreach ($path in $paths) { $before[$path] = (Get-FileHash $path).Hash }
            foreach ($operation in @(@{}, @{ Restore = $true }, @{ MigrateLegacy = $true })) {
                $result = Invoke-FixtureSetup $f $operation
                Assert-True ($result.ExitCode -ne 0 -and $result.Output -like '*-UserMcp*') 'Ordinary reinstall or restore cannot reintroduce a workspace duplicate'
                foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) "Refused operation preserves $path" }
            }
            Assert-Success (Invoke-UserMcpSetup $f @{ VSCodeMcpPath = $personalVSCode })
            foreach ($path in $paths) { Assert-True ((Get-FileHash $path).Hash -ceq $before[$path]) "Explicit personal update is idempotent: $path" }
            Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true })
            Assert-True ((Get-FileHash $personalCli).Hash -ceq $before[$personalCli] -and (Get-FileHash $personalVSCode).Hash -ceq $before[$personalVSCode]) 'Legacy uninstall leaves personal registrations intact'
            Assert-True ((Test-Path $profile) -and (Test-Path (Join-Path $f.Install 'xray.exe'))) 'Legacy uninstall retains the active personal profile and executable'
        }

        Test-Case 'personal upgrade refuses unreadable selected personal configs before workspace writes' {
            foreach ($source in @('cli', 'vscode')) {
                foreach ($malformed in @($true, $false)) {
                    $f = New-UserMcpFixture
                    $path = if ($source -eq 'cli') { Join-Path $f.Home 'mcp-config.json' } else { Join-Path $f.Home 'appdata\Code\User\mcp.json' }
                    $text = if ($malformed) { '{' } elseif ($source -eq 'cli') { '{"mcpServers":[]}' } else { '{"servers":[]}' }
                    Write-FixtureFile $path $text
                    $result = Invoke-FixtureSetup $f
                    Assert-True ($result.ExitCode -ne 0) "Unreadable personal config blocks unsafe workspace installation: $source"
                    Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Unreadable personal config untouched'
                    Assert-True (-not (Test-Path (Join-Path $f.Repo '.github\mcp.json')) -and -not (Test-Path (Join-Path $f.Repo '.vscode\mcp.json'))) 'Failed personal preflight creates no workspace config'
                }
            }
        }

        Test-Case 'personal upgrade allows unrelated personal servers and ignores unselected VS Code state' {
            foreach ($cliOnly in @($true, $false)) {
                $f = New-UserMcpFixture
                $personalCli = Join-Path $f.Home 'mcp-config.json'
                $personalVSCode = Join-Path $f.Home 'appdata\Code\User\mcp.json'
                Write-FixtureFile $personalCli '{"mcpServers":{"other":{"command":"other.exe"}}}'
                $vscode = if ($cliOnly) { '{"servers":{"xray":{"command":"foreign.exe"}}}' } else { '{"servers":{"other":{"command":"other.exe"}}}' }
                Write-FixtureFile $personalVSCode $vscode
                if ($cliOnly) { Write-FixtureFile (Join-Path $f.Home 'appdata\Code\User\globalStorage\storage.json') '{' }
                $parameters = if ($cliOnly) { @{ EnableCopilotCli = $true } } else { @{} }
                Assert-Success (Invoke-FixtureSetup $f $parameters)
                Assert-True ([IO.File]::ReadAllText($personalCli) -ceq '{"mcpServers":{"other":{"command":"other.exe"}}}') 'Unrelated personal CLI servers preserved'
                Assert-True ([IO.File]::ReadAllText($personalVSCode) -ceq $vscode) 'Unrelated or unselected personal VS Code config preserved'
                Assert-True ((Get-FixtureJson (Join-Path $f.Repo '.github\mcp.json')).mcpServers.xray.command -eq (Join-Path $f.Install 'xray.exe')) 'No false-positive conflict for selected CLI client'
                if ($cliOnly) { Assert-True (-not (Test-Path (Join-Path $f.Repo '.vscode\mcp.json'))) 'CLI-only workspace installation does not select VS Code' }
            }
        }

        Test-Case 'personal setup preserves tracked corporate configs and index' {
            $f = New-UserMcpFixture
            $corporate = Join-Path $f.Repo '.mcp.json'
            $text = '{"mcpServers":{"corporate":{"command":"corporate.exe"}}}'
            Write-FixtureFile $corporate $text
            [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json'))
            [void](Invoke-SetupGit $f.Repo @('commit', '--quiet', '-m', 'corporate MCP'))
            $index = (Get-FileHash -LiteralPath (Join-Path $f.Repo '.git\index')).Hash
            $cli = Join-Path $f.Home 'mcp-config.json'
            $vscode = Join-Path $f.Home 'vscode\mcp.json'
            Write-FixtureFile $cli '{"mcpServers":{"other":{"command":"other.exe"}},"setting":true}'
            Write-FixtureFile $vscode '{"servers":{"other":{"command":"other.exe"}}}'
            $result = Invoke-UserMcpSetup $f
            Assert-Success $result
            Assert-True ([IO.File]::ReadAllText($corporate) -ceq $text) 'Tracked corporate MCP unchanged'
            Assert-True ((Get-FileHash -LiteralPath (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Index byte-identical'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Repo '.github\mcp.json'))) 'No workspace CLI config'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Repo '.vscode\mcp.json'))) 'No workspace VS Code config'
            $entry = (Read-SetupJson $cli).Value.mcpServers
            Assert-True ($entry.other.command -eq 'other.exe') 'Other CLI server preserved'
            Assert-True ($entry.xray.args[0] -eq 'serve-workspace') 'Native workspace dispatcher registered'
            $vscodeEntry = (Read-SetupJson $vscode).Value.servers.xray
            Assert-True (-not $vscodeEntry.PSObject.Properties['cwd']) 'Personal entry does not pin a workspace or contain an unresolved CWD'
            Assert-True (($vscodeEntry | ConvertTo-Json -Depth 10) -notmatch '\$\{') 'Personal entry is portable to Copilot harness'
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            Assert-True ((Read-SetupJson $profile).Value.workspaceRoot -eq $f.Repo) 'Profile binds actual workspace'
            $before = [IO.File]::ReadAllText($cli)
            $result = Invoke-UserMcpSetup $f
            Assert-Success $result
            Assert-True ([IO.File]::ReadAllText($cli) -ceq $before) 'Personal registration idempotent'
        }

        Test-Case 'personal setup upgrades the old variable-based VS Code registration' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $vscode = Join-Path $f.Home 'vscode\mcp.json'
            $document = Read-SetupJson $vscode
            $document.Value.servers.xray | Add-Member -NotePropertyName cwd -NotePropertyValue '${workspaceFolder}'
            $document.Value.servers | Add-Member -NotePropertyName other -NotePropertyValue ([PSCustomObject]@{ command = 'other.exe' })
            [void](Write-SetupJson $document)
            Assert-Success (Invoke-UserMcpSetup $f)
            $updated = (Read-SetupJson $vscode).Value.servers
            Assert-True (-not $updated.xray.PSObject.Properties['cwd']) 'Old unresolved workspace variable removed'
            Assert-True ($updated.other.command -eq 'other.exe') 'Other VS Code servers preserved during upgrade'
        }

        Test-Case 'two workspace profiles share registrations and uninstall safely' {
            $a = New-UserMcpFixture
            $b = New-Fixture
            $b.Install = $a.Install
            $b.Home = $a.Home
            $result = Invoke-UserMcpSetup $a
            Assert-Success $result
            $cli = Join-Path $a.Home 'mcp-config.json'
            $registration = [IO.File]::ReadAllText($cli)
            $result = Invoke-UserMcpSetup $b @{ Extensions = 'ts' }
            Assert-Success $result
            Assert-True ([IO.File]::ReadAllText($cli) -ceq $registration) 'One shared registration'
            $profiles = Join-Path $a.Install 'workspaces'
            $aProfile = Get-WorkspaceProfilePath $profiles $a.Repo
            $bProfile = Get-WorkspaceProfilePath $profiles $b.Repo
            Assert-True ((Read-SetupJson $bProfile).Value.args[1] -eq 'ts') 'Independent workspace settings'
            $result = Invoke-UserMcpSetup $a @{ Uninstall = $true; DryRun = $true }
            Assert-True ($result.ExitCode -eq 0 -and (Test-Path -LiteralPath $aProfile)) 'Dry run preserves profile'
            $result = Invoke-UserMcpSetup $a @{ Uninstall = $true }
            Assert-Success $result
            Assert-True (-not (Test-Path -LiteralPath $aProfile)) 'Only selected profile removed'
            Assert-True (Test-Path -LiteralPath $bProfile) 'Other profile retained'
            Assert-True ([IO.File]::ReadAllText($cli) -ceq $registration) 'Registration retained for other workspace'
            $result = Invoke-UserMcpSetup $b @{ Uninstall = $true }
            Assert-Success $result
            Assert-True (-not (Read-SetupJson $cli).Value.mcpServers.PSObject.Properties['xray']) 'Last managed registration removed'
            Assert-True (Test-Path -LiteralPath (Join-Path $a.Install 'xray.exe')) 'Shared binary retained for other VS Code profiles'
        }

        Test-Case 'personal profile backup and restore do not roll back shared registrations' {
            $f = New-UserMcpFixture
            $result = Invoke-UserMcpSetup $f @{ Extensions = 'rs' }
            Assert-Success $result
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            $before = [IO.File]::ReadAllText($profile)
            $result = Invoke-UserMcpSetup $f @{ Extensions = 'ts' }
            Assert-Success $result
            $result = Invoke-UserMcpSetup $f @{ Restore = $true }
            Assert-Success $result
            Assert-True ([IO.File]::ReadAllText($profile) -ceq $before) 'Selected workspace settings restored'
            Assert-True ((Read-SetupJson (Join-Path $f.Home 'mcp-config.json')).Value.mcpServers.xray.args[0] -eq 'serve-workspace') 'Shared registration unchanged'
        }

        Test-Case 'malformed and unmanaged personal configs fail before writing profiles' {
            foreach ($content in @('{', '{"mcpServers":{"xray":{"command":"foreign.exe"}}}')) {
                $f = New-UserMcpFixture
                $cli = Join-Path $f.Home 'mcp-config.json'
                Write-FixtureFile $cli $content
                $result = Invoke-UserMcpSetup $f @{ Force = $false }
                Assert-True ($result.ExitCode -ne 0) 'Unsafe replacement rejected'
                Assert-True ([IO.File]::ReadAllText($cli) -ceq $content) 'Personal data unchanged'
                Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Install 'workspaces'))) 'No success-shaped profile written'
            }
        }

        Test-Case 'personal migration requires consent and never modifies tracked Xray entries' {
            $f = New-UserMcpFixture
            $local = Join-Path $f.Repo '.vscode\mcp.json'
            $text = @{ servers = @{ xray = @{ command = (Join-Path $f.Install 'xray.exe'); args = @('serve', '--dir', $f.Repo) }; other = @{ command = 'other.exe' } } } | ConvertTo-Json -Depth 8
            Write-FixtureFile $local $text
            $result = Invoke-UserMcpSetup $f
            Assert-True ($result.ExitCode -ne 0) 'Migration consent required even with Force'
            Assert-True ([IO.File]::ReadAllText($local) -ceq $text) 'Local entry retained without consent'
            $result = Invoke-UserMcpSetup $f @{ MigrateLegacy = $true }
            Assert-Success $result
            Assert-True (-not (Read-SetupJson $local).Value.servers.PSObject.Properties['xray']) 'Approved local entry removed'
            Assert-True ((Read-SetupJson $local).Value.servers.other.command -eq 'other.exe') 'Other local server retained'
            $f = New-UserMcpFixture
            $local = Join-Path $f.Repo '.mcp.json'
            $text = @{ mcpServers = @{ xray = @{ command = (Join-Path $f.Install 'xray.exe') } } } | ConvertTo-Json -Depth 8
            Write-FixtureFile $local $text
            [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json'))
            $result = Invoke-UserMcpSetup $f @{ MigrateLegacy = $true }
            Assert-True ($result.ExitCode -ne 0) 'Tracked entry migration refused'
            Assert-True ([IO.File]::ReadAllText($local) -ceq $text) 'Tracked Xray config unchanged'
        }

        Test-Case 'uninstall preflights personal JSON before deleting the workspace profile' {
            $f = New-UserMcpFixture
            $result = Invoke-UserMcpSetup $f
            Assert-Success $result
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            Write-FixtureFile (Join-Path $f.Home 'mcp-config.json') '{'
            $result = Invoke-UserMcpSetup $f @{ Uninstall = $true }
            Assert-True ($result.ExitCode -ne 0) 'Malformed uninstall config rejected'
            Assert-True (Test-Path -LiteralPath $profile) 'Profile preserved on preflight failure'
        }

        Test-Case 'personal migration removes only a filtered local overlay and preserves the index' {
            $f = New-UserMcpFixture
            $canonical = Add-LegacyFixture $f
            [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
            $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
            $result = Invoke-UserMcpSetup $f @{ MigrateLegacy = $true }
            Assert-Success $result
            Assert-True ([IO.File]::ReadAllText((Join-Path $f.Repo '.mcp.json')) -ceq $canonical) 'Only injected line removed; corporate bytes preserved'
            Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Index byte-identical after filter migration'
            Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'Owned filter directory removed'
            Assert-True ((Invoke-SetupGit $f.Repo @('config', '--local', '--get', 'filter.xray-mcp.clean') @(0, 1)).ExitCode -eq 1) 'Owned filter registration removed'
            Assert-True ([IO.File]::ReadAllText((Join-Path $f.Repo '.git\info\attributes')).Contains('*.txt eol=lf')) 'Unrelated attributes retained'
            Assert-Success (Invoke-UserMcpSetup $f)
        }

        Test-Case 'personal orphan migration disables a real smudge filter before recheckout' {
            $f = New-UserMcpFixture
            $canonical = Add-LegacyFixture $f
            $path = Join-Path $f.Repo '.mcp.json'
            $injected = [IO.File]::ReadAllText($path)
            [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
            Write-FixtureFile (Join-Path $f.Repo '.git\xray-mcp\injected.json') $injected
            [void](Invoke-SetupGit $f.Repo @('config', 'filter.xray-mcp.smudge', 'cat .git/xray-mcp/injected.json'))
            Remove-Item -LiteralPath $path -Force
            [void](Invoke-SetupGit $f.Repo @('checkout-index', '--force', '--', '.mcp.json'))
            Assert-True ($null -ne (Get-FixtureJson $path).mcpServers.PSObject.Properties['xray']) 'Fixture really reinjects Xray on checkout'
            Write-FixtureFile $path $canonical
            $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
            $result = Invoke-UserMcpSetup $f
            Assert-True ($result.ExitCode -ne 0) 'Orphan filter requires migration consent'
            Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp')) 'Filter retained without consent'
            Assert-True (-not (Test-Path (Join-Path $f.Home 'mcp-config.json'))) 'No registration without migration consent'
            Assert-Success (Invoke-UserMcpSetup $f @{ MigrateLegacy = $true })
            Assert-True ([IO.File]::ReadAllText($path) -ceq $canonical) 'Metadata-only cleanup preserves workspace bytes'
            Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Metadata-only cleanup preserves index bytes'
            Assert-True (-not (Test-Path "$path.bak")) 'Unchanged workspace file needs no backup'
            Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'Orphan filter resources removed'
            Assert-True ((Invoke-SetupGit $f.Repo @('config', '--local', '--get', 'filter.xray-mcp.smudge') @(0, 1)).ExitCode -eq 1) 'Smudge registration removed'
            $attributes = [IO.File]::ReadAllText((Join-Path $f.Repo '.git\info\attributes'))
            Assert-True ($attributes.Contains('*.txt eol=lf') -and $attributes -notmatch 'filter=xray-mcp') 'Only legacy attributes removed'
            Remove-Item -LiteralPath $path -Force
            [void](Invoke-SetupGit $f.Repo @('checkout-index', '--force', '--', '.mcp.json'))
            $json = Get-FixtureJson $path
            Assert-True ($null -eq $json.mcpServers.PSObject.Properties['xray'] -and $json.mcpServers.notes.command -eq 'notes') 'Real recheckout cannot restore the old workspace entry'
            Assert-Success (Invoke-UserMcpSetup $f)
        }

        Test-Case 'personal orphan migration cleans missing root and VS Code configs without creating files' {
            foreach ($definition in @(
                @{ Relative = '.mcp.json'; Filter = 'xray-mcp' },
                @{ Relative = '.vscode/mcp.json'; Filter = 'xray-vscode-mcp' }
            )) {
                $f = New-UserMcpFixture
                $path = Join-Path $f.Repo $definition.Relative
                $filterDirectory = Join-Path (Join-Path $f.Repo '.git') $definition.Filter
                Write-FixtureFile (Join-Path $filterDirectory 'snapshot.txt') 'orphan'
                [void](Invoke-SetupGit $f.Repo @('config', "filter.$($definition.Filter).smudge", 'cat'))
                Write-FixtureFile (Join-Path $f.Repo '.git\info\attributes') "$($definition.Relative) filter=$($definition.Filter) text eol=lf`n*.txt eol=lf`n"
                $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
                Assert-Success (Invoke-UserMcpSetup $f @{ MigrateLegacy = $true })
                Assert-True (-not (Test-Path $path) -and -not (Test-Path "$path.bak")) 'Metadata cleanup does not create a config or backup'
                Assert-True (-not (Test-Path $filterDirectory)) 'Orphan filter directory removed'
                Assert-True ((Invoke-SetupGit $f.Repo @('config', '--local', '--get', "filter.$($definition.Filter).smudge") @(0, 1)).ExitCode -eq 1) 'Orphan filter configuration removed'
                $attributes = [IO.File]::ReadAllText((Join-Path $f.Repo '.git\info\attributes'))
                Assert-True ($attributes.Contains('text eol=lf') -and $attributes.Contains('*.txt eol=lf') -and $attributes -notmatch 'filter=') 'Other attributes preserved'
                Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Missing config cleanup preserves the index'
            }
        }

        Test-Case 'personal orphan migration refuses shared worktrees, index entries and index flags' {
            foreach ($kind in @('skip-worktree', 'assume-unchanged', 'multiple-worktrees', 'index-entry')) {
                $f = New-UserMcpFixture
                $canonical = Add-LegacyFixture $f
                $path = Join-Path $f.Repo '.mcp.json'
                [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
                Write-FixtureFile $path $canonical
                if ($kind -eq 'multiple-worktrees') {
                    [void](Invoke-SetupGit $f.Repo @('worktree', 'add', '--quiet', '--detach', (Join-Path (Split-Path $f.Repo -Parent) 'second worktree')))
                }
                elseif ($kind -eq 'index-entry') {
                    $text = $canonical.Replace('"notes":', '"xray":')
                    Write-FixtureFile $path $text
                    [void](Invoke-SetupGit $f.Repo @('add', '--', '.mcp.json'))
                    Write-FixtureFile $path $canonical
                }
                else { [void](Invoke-SetupGit $f.Repo @('update-index', "--$kind", '--', '.mcp.json')) }
                $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
                $result = Invoke-UserMcpSetup $f @{ MigrateLegacy = $true }
                Assert-True ($result.ExitCode -ne 0) "Unsafe orphan cleanup refused: $kind"
                Assert-True ([IO.File]::ReadAllText($path) -ceq $canonical) 'Workspace bytes preserved on refusal'
                Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Index bytes preserved on refusal'
                Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp')) 'Filter resources preserved on refusal'
                Assert-True (-not (Test-Path (Join-Path $f.Home 'mcp-config.json'))) 'Unsafe orphan cleanup does not register personal MCP'
            }
        }

        Test-Case 'personal migration cleans legacy metadata for an untracked owned entry' {
            $f = New-UserMcpFixture
            $path = Join-Path $f.Repo '.mcp.json'
            $text = @{ mcpServers = @{ xray = @{ command = (Join-Path $f.Install 'xray.exe') }; other = @{ command = 'other.exe' } } } | ConvertTo-Json -Depth 8
            Write-FixtureFile $path $text
            Write-FixtureFile (Join-Path $f.Repo '.git\xray-mcp\snapshot.txt') 'legacy'
            Write-FixtureFile (Join-Path $f.Repo '.git\info\attributes') ".mcp.json filter=xray-mcp`n"
            [void](Invoke-SetupGit $f.Repo @('config', 'filter.xray-mcp.smudge', 'cat'))
            Assert-Success (Invoke-UserMcpSetup $f @{ MigrateLegacy = $true })
            $json = Get-FixtureJson $path
            Assert-True ($null -eq $json.mcpServers.PSObject.Properties['xray'] -and $json.mcpServers.other.command -eq 'other.exe') 'Only owned untracked entry removed'
            Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq $text) 'Original untracked entry backed up'
            Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'Untracked entry filter resources removed'
            Assert-True ((Invoke-SetupGit $f.Repo @('config', '--local', '--get', 'filter.xray-mcp.smudge') @(0, 1)).ExitCode -eq 1) 'Untracked entry filter registration removed'
        }

        Test-Case 'personal migration revalidates untracked entries and Git protection before writing' {
            foreach ($definition in @(
                @{ Relative = '.mcp.json'; Container = 'mcpServers' },
                @{ Relative = '.github/mcp.json'; Container = 'mcpServers' },
                @{ Relative = '.vscode/mcp.json'; Container = 'servers' }
            )) {
                foreach ($kind in @('foreign-owner', 'changed-owned-entry', 'changed-other-server', 'deleted-config', 'tracked-config', 'tracked-backup')) {
                    $f = New-UserMcpFixture
                    $path = Join-Path $f.Repo $definition.Relative
                    $text = @{ $definition.Container = @{ xray = @{ command = (Join-Path $f.Install 'xray.exe'); args = @('serve') }; other = @{ command = 'other.exe' } } } | ConvertTo-Json -Depth 8
                    Write-FixtureFile $path $text
                    $plan = @(Get-UserMcpMigrationConfigs $f.Repo (Join-Path $f.Install 'xray.exe') $true $true)[0]
                    $changed = $text
                    switch ($kind) {
                        'foreign-owner' { $changed = $text.Replace('xray.exe', 'foreign.exe') }
                        'changed-owned-entry' { $changed = $text.Replace('"serve"', '"serve-workspace"') }
                        'changed-other-server' { $changed = $text.Replace('other.exe', 'updated-other.exe') }
                        'deleted-config' { Remove-Item -LiteralPath $path -Force }
                        'tracked-config' { [void](Invoke-SetupGit $f.Repo @('add', '--', $definition.Relative)) }
                        'tracked-backup' {
                            Write-FixtureFile "$path.bak" 'tracked backup'
                            [void](Invoke-SetupGit $f.Repo @('add', '--', "$($definition.Relative).bak"))
                        }
                    }
                    if ($kind -notin @('deleted-config', 'tracked-config', 'tracked-backup')) { Write-FixtureFile $path $changed }
                    $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
                    $excludePath = Join-Path $f.Repo '.git\info\exclude'
                    $excludeBefore = if (Test-Path $excludePath) { [IO.File]::ReadAllText($excludePath) } else { $null }
                    $rejected = $false
                    try {
                        Remove-UserMcpWorkspaceEntry -RepoRoot $f.Repo -IsGitRepo $true -ExcludePath (Join-Path $f.Repo '.git\info\exclude') -Migration $plan
                    }
                    catch { $rejected = $true }
                    Assert-True $rejected "Concurrent change refused: $($definition.Relative), $kind"
                    if ($kind -eq 'deleted-config') { Assert-True (-not (Test-Path $path)) 'Deleted config not recreated' }
                    else { Assert-True ([IO.File]::ReadAllText($path) -ceq $changed) 'Concurrent workspace text preserved' }
                    if ($kind -eq 'tracked-backup') { Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq 'tracked backup') 'Tracked backup preserved' }
                    else { Assert-True (-not (Test-Path "$path.bak")) 'No backup written on refusal' }
                    Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'Concurrent index state preserved'
                    $excludeAfter = if (Test-Path $excludePath) { [IO.File]::ReadAllText($excludePath) } else { $null }
                    Assert-True ($excludeAfter -ceq $excludeBefore) 'No exclusions changed before revalidation'
                }
            }
        }

        Test-Case 'personal migration entry removal pins the approved text through reread' {
            $f = New-UserMcpFixture
            $path = Join-Path $f.Repo '.mcp.json'
            $text = '{"mcpServers":{"xray":{"command":"xray.exe"},"other":{"command":"other.exe"}}}'
            $changed = $text.Replace('xray.exe', 'foreign.exe')
            Write-FixtureFile $path $changed
            $rejected = $false
            try { [void](Remove-XrayServerEntry -Path $path -ContainerKey 'mcpServers' -ExpectedText $text) }
            catch { $rejected = $_.Exception.Message -like '*changed during setup*' }
            Assert-True $rejected 'Replacement between migration validation and removal rejected explicitly'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $changed) 'Replacement entry preserved'
            Assert-True (-not (Test-Path "$path.bak")) 'Replacement entry never backed up or mutated'
        }

        Test-Case 'personal orphan migration revalidates metadata-only plans' {
            $f = New-UserMcpFixture
            $canonical = Add-LegacyFixture $f
            $path = Join-Path $f.Repo '.mcp.json'
            [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
            Write-FixtureFile $path $canonical
            $plan = @(Get-UserMcpMigrationConfigs $f.Repo (Join-Path $f.Install 'xray.exe') $true $true)[0]
            Assert-True ($null -ne $plan) 'Orphan filter produces a migration plan'
            $changed = $canonical.Replace('"notes"', '"updated-notes"')
            Write-FixtureFile $path $changed
            $rejected = $false
            try {
                Remove-UserMcpWorkspaceEntry -RepoRoot $f.Repo -IsGitRepo $true -ExcludePath (Join-Path $f.Repo '.git\info\exclude') -Migration $plan
            }
            catch { $rejected = $_.Exception.Message -like '*changed during setup*' }
            Assert-True $rejected 'Metadata-only plan rejects changed workspace text'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $changed) 'Metadata-only revalidation preserves changed text'
            Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp')) 'Filter retained after metadata-only plan invalidation'
        }

        Test-Case 'personal filtered migration refuses other local changes, index entries and index flags' {
            foreach ($kind in @('local-changes', 'index-entry', 'index-flags', 'multiple-worktrees')) {
                $f = New-UserMcpFixture
                $null = Add-LegacyFixture $f
                $path = Join-Path $f.Repo '.mcp.json'
                if ($kind -ne 'index-flags') {
                    [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
                }
                if ($kind -eq 'local-changes') {
                    Write-FixtureFile $path ([IO.File]::ReadAllText($path).Replace('"notes"', '"other-notes"'))
                }
                if ($kind -eq 'index-entry') { [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json')) }
                if ($kind -eq 'multiple-worktrees') {
                    [void](Invoke-SetupGit $f.Repo @('worktree', 'add', '--quiet', '--detach', (Join-Path (Split-Path $f.Repo -Parent) 'second worktree')))
                }
                $before = [IO.File]::ReadAllText($path)
                $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
                $result = Invoke-UserMcpSetup $f @{ MigrateLegacy = $true }
                Assert-True ($result.ExitCode -ne 0) "Unsafe filtered migration refused: $kind"
                Assert-True ([IO.File]::ReadAllText($path) -ceq $before) "Working copy preserved: $kind"
                Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) "Index preserved: $kind"
                Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp')) "Filter resources preserved: $kind"
                Assert-True (-not (Test-Path (Join-Path $f.Home 'mcp-config.json'))) "No partial registration: $kind"
            }
        }

        Test-Case 'personal filtered migration revalidates the index before writing' {
            $f = New-UserMcpFixture
            $canonical = Add-LegacyFixture $f
            [void](Invoke-SetupGit $f.Repo @('update-index', '--no-skip-worktree', '--no-assume-unchanged', '--', '.mcp.json'))
            $plan = @(Get-UserMcpMigrationConfigs $f.Repo (Join-Path $f.Install 'xray.exe') $true $true)[0]
            $path = Join-Path $f.Repo '.mcp.json'
            $injected = [IO.File]::ReadAllText($path)
            Write-FixtureFile $path ($canonical.Replace('"notes"', '"updated-notes"'))
            [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json'))
            Write-FixtureFile $path $injected
            $index = (Get-FileHash (Join-Path $f.Repo '.git\index')).Hash
            $rejected = $false
            try {
                Remove-UserMcpWorkspaceEntry -RepoRoot $f.Repo -IsGitRepo $true -ExcludePath (Join-Path $f.Repo '.git\info\exclude') -Migration $plan
            }
            catch { $rejected = $true }
            Assert-True $rejected 'Concurrent index change refused'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $injected) 'Concurrent staged changes do not overwrite working copy'
            Assert-True ((Get-FileHash (Join-Path $f.Repo '.git\index')).Hash -ceq $index) 'New staged state retained'
        }

        Test-Case 'personal setup rejects unsupported executable before writing configs' {
            $f = New-Fixture
            $result = Invoke-UserMcpSetup $f
            Assert-True ($result.ExitCode -ne 0) 'Unsupported executable rejected'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'mcp-config.json'))) 'No unusable registration'
        }

        Test-Case 'VS Code named profile, inherited MCP and explicit path selection' {
            $f = New-UserMcpFixture
            $oldAppData = $env:APPDATA
            $oldPortable = $env:VSCODE_PORTABLE
            try {
                $env:APPDATA = Join-Path $f.Home 'appdata'
                $env:VSCODE_PORTABLE = $null
                $user = Join-Path $env:APPDATA 'Code\User'
                $state = Join-Path $user 'globalStorage\storage.json'
                $associations = @{}
                $associations[([Uri]($f.Repo + '\')).AbsoluteUri.TrimEnd('/')] = 'named-profile'
                $value = @{ profileAssociations = @{ workspaces = $associations }; userDataProfiles = @(@{ location = 'named-profile'; useDefaultFlags = @{ mcp = $false } }) }
                Write-FixtureFile $state ($value | ConvertTo-Json -Depth 8)
                Assert-True ((Get-UserVSCodeMcpPath $f.Repo '') -eq (Join-Path $user 'profiles\named-profile\mcp.json')) 'Named profile resolved'
                $encodedUri = ([Uri]($f.Repo + '\')).AbsoluteUri.TrimEnd('/') -replace '^file:///([A-Za-z]):/', 'file:///$1%3A/'
                $value.profileAssociations.workspaces = [ordered]@{ 'file:///Z%3A/Unrelated%20Folder' = 'unrelated-profile' }
                $value.profileAssociations.workspaces[$encodedUri.ToLowerInvariant() + '/'] = 'named-profile'
                Write-FixtureFile $state ($value | ConvertTo-Json -Depth 8)
                Assert-True ((Get-UserVSCodeMcpPath $f.Repo '') -eq (Join-Path $user 'profiles\named-profile\mcp.json')) 'Encoded drive, spaces, case and trailing slash resolved'
                $value.profileAssociations.workspaces = [ordered]@{ 'file://other-server/share/Repo' = 'unrelated-profile'; 'file://server/share/Repo' = 'named-profile' }
                Write-FixtureFile $state ($value | ConvertTo-Json -Depth 8)
                Assert-True ((Get-UserVSCodeMcpPath '\\server\share\Repo' '') -eq (Join-Path $user 'profiles\named-profile\mcp.json')) 'UNC profile matches its host and share'
                Assert-True ((Get-UserVSCodeMcpPath '\\unregistered-server\share\Repo' '') -eq (Join-Path $user 'mcp.json')) 'Different UNC host does not select another profile'
                $value.profileAssociations.workspaces = [ordered]@{}
                $value.profileAssociations.workspaces[$encodedUri] = 'named-profile'
                $value.userDataProfiles[0].useDefaultFlags.mcp = $true
                Write-FixtureFile $state ($value | ConvertTo-Json -Depth 8)
                Assert-True ((Get-UserVSCodeMcpPath $f.Repo '') -eq (Join-Path $user 'mcp.json')) 'Inherited MCP uses default'
                $override = Join-Path $f.Home 'custom\mcp.json'
                Assert-True ((Get-UserVSCodeMcpPath $f.Repo $override) -eq $override) 'Explicit profile override respected'
            }
            finally { $env:APPDATA = $oldAppData; $env:VSCODE_PORTABLE = $oldPortable }
        }

        Test-Case 'CLI-only setup does not depend on VS Code state' {
            $f = New-UserMcpFixture
            Write-FixtureFile (Join-Path $f.Home 'appdata\Code\User\globalStorage\storage.json') '{'
            $result = Invoke-UserMcpSetup $f @{ EnableCopilotCli = $true; VSCodeMcpPath = '' }
            Assert-Success $result
            Assert-True ((Read-SetupJson (Join-Path $f.Home 'mcp-config.json')).Value.mcpServers.xray.args[0] -eq 'serve-workspace') 'CLI/SDK configured independently'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'appdata\Code\User\mcp.json'))) 'VS Code not selected'
        }

        Test-Case 'personal relative paths use PowerShell location, not process CWD' {
            $f = New-UserMcpFixture
            $location = Split-Path $f.Install -Parent
            $result = Invoke-FixtureSetup $f @{
                UserMcp = $true; InstallDir = 'install with spaces'; VSCodeMcpPath = 'personal vscode\mcp.json'
            } -Location $location
            Assert-Success $result
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            Assert-True (Test-Path -LiteralPath $profile) 'Relative install resolved from shell location'
            Assert-True (Test-Path -LiteralPath (Join-Path $location 'personal vscode\mcp.json')) 'Relative MCP override resolved from shell location'
        }

        Test-Case 'legacy uninstall cannot remove an executable used by personal profiles' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-FixtureSetup $f)
            Assert-Success (Invoke-UserMcpSetup $f @{ MigrateLegacy = $true })
            Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true })
            Assert-True (Test-Path -LiteralPath (Join-Path $f.Install 'xray.exe')) 'Personal workspace executable retained'
            $profile = Get-WorkspaceProfilePath (Join-Path $f.Install 'workspaces') $f.Repo
            Assert-True (Test-Path -LiteralPath $profile) 'Personal workspace profile retained'
        }

        Test-Case 'personal uninstall preserves entries replaced by another owner' {
            $f = New-UserMcpFixture
            Assert-Success (Invoke-UserMcpSetup $f)
            $cli = Join-Path $f.Home 'mcp-config.json'
            $text = '{"mcpServers":{"xray":{"command":"foreign.exe"},"other":{"command":"other.exe"}}}'
            Write-FixtureFile $cli $text
            Assert-Success (Invoke-UserMcpSetup $f @{ Uninstall = $true })
            Assert-True ([IO.File]::ReadAllText($cli) -ceq $text) 'Unmanaged personal entry retained byte-for-byte'
        }

        Test-Case 'personal lifecycle uninstall pins ownership through entry removal' {
            $managedCheck = (Get-Item Function:\Test-ManagedUserMcpEntry).ScriptBlock
            foreach ($container in @('mcpServers', 'servers')) {
                foreach ($change in @('foreign-owner', 'other-server', 'deleted-config')) {
                    foreach ($dryRun in @($false, $true)) {
                        $f = New-Fixture
                        $binary = Join-Path $f.Install 'xray.exe'
                        $profiles = Join-Path $f.Install 'workspaces'
                        $profilePath = Get-WorkspaceProfilePath $profiles $f.Repo
                        Write-FixtureFile $profilePath (@{ version = 1; workspaceRoot = $f.Repo; args = @() } | ConvertTo-Json -Depth 8)
                        $path = Join-Path $f.Home 'personal-mcp.json'
                        $text = @{ $container = @{
                            xray = @{ command = $binary; args = @('serve-workspace', '--profiles', $profiles); _xrayUserMcp = 1 }
                            other = @{ command = 'other.exe' }
                        } } | ConvertTo-Json -Depth 8
                        Write-FixtureFile $path $text
                        $replacement = if ($change -eq 'foreign-owner') { $text.Replace('xray.exe', 'foreign.exe') } else { $text.Replace('other.exe', 'updated-other.exe') }
                        $race = @{ Path = $path; Text = $replacement; Delete = $change -eq 'deleted-config'; Validations = 0 }
                        $rejected = $false
                        try {
                            function Test-ManagedUserMcpEntry {
                                param($Entry, [string]$Binary, [string]$Profiles)
                                $managed = & $managedCheck $Entry $Binary $Profiles
                                if ($managed) {
                                    # Simulate another writer after the real ownership check succeeds.
                                    $race.Validations++
                                    if ($race.Delete) { Remove-Item -LiteralPath $race.Path -Force }
                                    else { Write-FixtureFile $race.Path $race.Text }
                                }
                                return $managed
                            }
                            $target = [PSCustomObject]@{ Path = $path; Container = $container; Client = 'Test personal client' }
                            try { Remove-UserMcpWorkspace -ProfilePath $profilePath -Profiles $profiles -Binary $binary -Targets @($target) -DryRun:$dryRun }
                            catch { $rejected = $_.Exception.Message -like '*changed during setup*' }
                        }
                        finally { Set-Item Function:\Test-ManagedUserMcpEntry -Value $managedCheck }
                        $label = "$container, $change, DryRun=$dryRun"
                        Assert-True ($race.Validations -eq 1) "Actual managed-entry validation succeeded before replacement: $label"
                        Assert-True $rejected "Concurrent personal config change rejected: $label"
                        if ($race.Delete) { Assert-True (-not (Test-Path $path)) "Deleted personal config not recreated: $label" }
                        else { Assert-True ([IO.File]::ReadAllText($path) -ceq $replacement) "Concurrent personal config bytes preserved: $label" }
                        Assert-True (-not (Test-Path "$path.bak")) "No backup of an unapproved replacement created: $label"
                    }
                }
            }
        }

        Test-Case 'personal files and profiles must remain outside the repository' {
            $f = New-UserMcpFixture
            foreach ($options in @(
                @{ InstallDir = (Join-Path $f.Repo 'install') },
                @{ VSCodeMcpPath = (Join-Path $f.Repo 'personal-mcp.json') }
            )) {
                $result = Invoke-UserMcpSetup $f $options
                Assert-True ($result.ExitCode -ne 0) 'Repository-local personal state refused'
                Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'mcp-config.json'))) 'No partial personal registration'
            }
        }
    }

    if (-not $UserMcpOnly) {
    Test-Case 'PowerShell filesystem drives resolve for workspace setup' {
        foreach ($inline in @($false, $true)) {
            foreach ($gitRepo in @($false, $true)) {
                $f = New-Fixture
                $driveRoot = Split-Path -Parent $f.Repo
                if (-not $gitRepo) {
                    $f.Repo = Join-Path $driveRoot 'plain repo'
                    Write-FixtureFile (Join-Path $f.Repo 'source.ts') 'export const value = 1;'
                }
                $vscode = Join-Path $f.Repo '.vscode\mcp.json'
                Write-FixtureFile $vscode '{"servers":{"other":{"command":"other.exe"}}}'
                $options = @{
                    RepoPath = 'XrayFixtureDrive:\' + (Split-Path -Leaf $f.Repo)
                    InstallDir = 'XrayFixtureDrive:\install with spaces'
                }
                Assert-Success (Invoke-FixtureSetup $f $options -Inline:$inline -DriveRoot $driveRoot)
                $config = Get-FixtureJson $vscode
                Assert-True ($config.servers.other.command -ceq 'other.exe') 'Existing server preserved through drive alias'
                Assert-True ($config.servers.xray.command -ceq (Join-Path $f.Install 'xray.exe')) 'Binary path uses backing filesystem'
                $args = @($config.servers.xray.args)
                Assert-True ($args[$args.IndexOf('--dir') + 1] -ceq $f.Repo) 'Repository path uses backing filesystem'
            }
        }
    }

    Test-Case 'PowerShell filesystem drives resolve for personal preflight' {
        foreach ($inline in @($false, $true)) {
            $f = New-Fixture
            $vscode = Join-Path $f.Repo '.vscode\mcp.json'
            $text = '{"servers":{"other":{"command":"other.exe"}}}'
            Write-FixtureFile $vscode $text
            $options = @{
                RepoPath = 'XrayFixtureDrive:\repo with spaces'
                InstallDir = 'XrayFixtureDrive:\install with spaces'
                UserMcp = $true
                PlanOnly = $true
                VSCodeMcpPath = Join-Path $f.Home 'vscode\mcp.json'
            }
            Assert-Success (Invoke-FixtureSetup $f $options -Inline:$inline -DriveRoot (Split-Path -Parent $f.Repo))
            Assert-True ([IO.File]::ReadAllText($vscode) -ceq $text) 'Personal preflight leaves workspace config unchanged'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Home 'mcp-config.json'))) 'Plan does not write personal config'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Install 'workspaces'))) 'Plan does not write workspace profile'
        }
    }

    Test-Case 'caller continuation for workspace setup restore and uninstall' {
        foreach ($inline in @($false, $true)) {
            $f = New-Fixture
            Assert-SetupReturned (Invoke-FixtureSetup $f -Inline:$inline)
            Assert-SetupReturned (Invoke-FixtureSetup $f @{ Extensions = 'rs' } -Inline:$inline)
            Assert-SetupReturned (Invoke-FixtureSetup $f @{ Restore = $true } -Inline:$inline)
            Assert-SetupReturned (Invoke-FixtureSetup $f @{ Uninstall = $true; DryRun = $true } -Inline:$inline)
            Assert-SetupReturned (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true } -Inline:$inline)
        }
    }

    Test-Case 'caller continuation after inline partial setup restore and uninstall failures' {
        $f = New-Fixture
        $cliPath = Join-Path $f.Repo '.github\mcp.json'
        $original = '{"mcpServers":{"corporate":{"command":"corporate"}}}'
        Write-FixtureFile $cliPath $original
        [void](Invoke-SetupGit -RepoRoot $f.Repo -Arguments @('add', '.github/mcp.json'))
        [void](Invoke-SetupGit -RepoRoot $f.Repo -Arguments @('commit', '--quiet', '-m', 'tracked client fixture'))
        foreach ($parameters in @(@{}, @{ Restore = $true }, @{ Uninstall = $true; KeepBinary = $true })) {
            $result = Invoke-FixtureSetup $f $parameters -Inline
            Assert-True ($result.ExitCode -eq 1) 'Incomplete operation must retain failure status'
            Assert-True ($result.Output -match '(?m)^XRAY_SETUP_CAUGHT_ERROR\r?$') "Caller could not catch the incomplete operation: $($result.Output)"
            Assert-True ([IO.File]::ReadAllText($cliPath) -ceq $original) 'Tracked config remains unchanged'
        }
    }

    Test-Case 'fresh setup, exact client formats, absolute paths, flags and exclusions' {
        $f = New-Fixture
        $result = Invoke-FixtureSetup $f
        Assert-Success $result
        $cli = Get-FixtureJson (Join-Path $f.Repo '.github\mcp.json')
        $vscode = Get-FixtureJson (Join-Path $f.Repo '.vscode\mcp.json')
        Assert-True ($cli.mcpServers.xray.type -eq 'stdio') 'CLI type'
        Assert-True ($cli.mcpServers.xray.tools -is [array] -and $cli.mcpServers.xray.tools.Count -eq 1 -and $cli.mcpServers.xray.tools[0] -eq '*') 'CLI tools must be ["*"]'
        Assert-True ($vscode.servers.xray.'//' -ceq $Script:XrayVisibleWarning) 'VS Code warning'
        Assert-True ($null -eq $vscode.servers.xray.PSObject.Properties['tools']) 'No tools field for VS Code'
        Assert-True ($vscode.servers.xray.type -eq 'stdio') 'VS Code type'
        Assert-True ($cli.mcpServers.xray.command -ceq (Join-Path $f.Install 'xray.exe')) 'Resolved executable path'
        $expected = @('serve', '--dir', $f.Repo, '--ext', 'cs,md', '--watch', '--definitions', '--debug-log')
        Assert-True (($cli.mcpServers.xray.args -join '|') -ceq ($expected -join '|')) 'Exact CLI args'
        Assert-True (($vscode.servers.xray.args -join '|') -ceq ($expected -join '|')) 'Shared VS Code args'
        Assert-XrayGitEnvironment $cli.mcpServers.xray.env
        Assert-XrayGitEnvironment $vscode.servers.xray.env
        foreach ($relative in @('.github/mcp.json', '.vscode/mcp.json', '.github/mcp.json.bak', '.vscode/mcp.json.bak')) {
            Assert-True (Test-GitIgnoresPath $f.Repo $relative) "Ignored: $relative"
        }
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.mcp.json'))) 'No root MCP creation'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'No filter installation'
        Assert-True ($result.Output -match 'No \(non-interactive\)') 'Non-interactive trust explicitly skipped'
        Assert-True (-not (Test-Path (Join-Path $f.Home 'config.json'))) 'No silent trust'

        $paths = @((Join-Path $f.Repo '.github\mcp.json'), (Join-Path $f.Repo '.vscode\mcp.json'), (Get-ResolvedGitPath $f.Repo 'info/exclude'))
        $times = @($paths | ForEach-Object { (Get-Item -LiteralPath $_ -Force).LastWriteTimeUtc })
        Assert-Success (Invoke-FixtureSetup $f)
        for ($i = 0; $i -lt $paths.Count; $i++) {
            Assert-True ((Get-Item -LiteralPath $paths[$i] -Force).LastWriteTimeUtc -eq $times[$i]) 'Unchanged rerun must not write'
        }
        Assert-True (-not (Test-Path "$($paths[0]).bak")) 'Unchanged rerun must not create backup'
    }

    Test-Case 'merge both clients, preserve other servers/settings, replace managed args' {
        $f = New-Fixture
        $cliPath = Join-Path $f.Repo '.github\mcp.json'
        $vsPath = Join-Path $f.Repo '.vscode\mcp.json'
        $old = '{"mcpServers":{"notes":{"command":"notes","args":["a"]},"xray":{"command":"stale","args":["serve","--metrics"]}},"custom":{"array":[1,2],"enabled":true}}'
        Write-FixtureFile $cliPath $old
        Write-FixtureFile $vsPath '{"servers":{"other":{"command":"other","env":{"key":"value"}}},"inputs":[{"id":"keep","type":"promptString"}]}'
        Assert-Success (Invoke-FixtureSetup $f)
        $cli = Get-FixtureJson $cliPath
        $vs = Get-FixtureJson $vsPath
        Assert-True ($cli.mcpServers.notes.args[0] -eq 'a' -and $cli.custom.array.Count -eq 2 -and $cli.custom.enabled) 'CLI unrelated definitions/settings'
        Assert-True ($vs.servers.other.env.key -eq 'value' -and $vs.inputs[0].id -eq 'keep') 'VS Code unrelated definitions/settings'
        Assert-True ($cli.mcpServers.xray.args -notcontains '--metrics') 'Installer owns args'
        Assert-True ([IO.File]::ReadAllText("$cliPath.bak") -ceq $old) 'Backup contains original bytes'
        Assert-True (Test-GitIgnoresPath $f.Repo '.github/mcp.json.bak') 'Backup protected'
        $time = (Get-Item "$cliPath.bak").LastWriteTimeUtc
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True ((Get-Item "$cliPath.bak").LastWriteTimeUtc -eq $time) 'Idempotent backup'
    }

    Test-Case 'workspace Git environment conflict stops before client or exclusion writes' {
        $f = New-Fixture
        $cli = Join-Path $f.Repo '.github\mcp.json'
        $text = '{"mcpServers":{"xray":{"command":"stale","env":{"GIT_CONFIG_COUNT":"1","GIT_CONFIG_KEY_0":"core.fsmonitor","GIT_CONFIG_VALUE_0":"private-setting-value"}}}}'
        Write-FixtureFile $cli $text
        $exclude = Get-ResolvedGitPath $f.Repo 'info/exclude'
        $before = [IO.File]::ReadAllText($exclude)
        $result = Invoke-FixtureSetup $f
        Assert-True ($result.ExitCode -ne 0 -and $result.Output -like '*Conflicting Xray Git setting*') 'Workspace conflict explicitly refused'
        Assert-True ($result.Output -notlike '*private-setting-value*') 'Private Git value not printed'
        Assert-True ([IO.File]::ReadAllText($cli) -ceq $text) 'CLI config unchanged'
        Assert-True ([IO.File]::ReadAllText($exclude) -ceq $before) 'Git excludes unchanged'
        Assert-True (-not (Test-Path -LiteralPath "$cli.bak")) 'No backup created'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $f.Repo '.vscode\mcp.json'))) 'No partial VS Code registration'
    }

    Test-Case 'tracked corporate root remains byte-identical' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.mcp.json'
        $text = "{`r`n  `"mcpServers`": { `"corporate`": { `"command`": `"shared`" } }`r`n}`r`n"
        Write-FixtureFile $path $text
        [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json'))
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Root config unchanged'
        Assert-True (-not (Test-Path "$path.bak")) 'No root backup during ordinary setup'
    }

    foreach ($relative in @('.github/mcp.json', '.vscode/mcp.json')) {
        Test-Case "tracked target skipped without changing its index: $relative" {
            $f = New-Fixture
            $text = if ($relative -match '^.github') { '{"mcpServers":{"shared":{"command":"shared"}}}' } else { '{"servers":{"shared":{"command":"shared"}}}' }
            $path = Join-Path $f.Repo $relative
            Write-FixtureFile $path $text
            [void](Invoke-SetupGit $f.Repo @('add', $relative))
            $index = (Invoke-SetupGit $f.Repo @('ls-files', '--stage', '--', $relative)).Lines -join ''
            $result = Invoke-FixtureSetup $f
            Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'tracked by Git') 'Tracked target reported, no success-shaped result'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Tracked file unchanged'
            Assert-True (((Invoke-SetupGit $f.Repo @('ls-files', '--stage', '--', $relative)).Lines -join '') -ceq $index) 'Index unchanged'
            Assert-True (-not (Test-Path "$path.bak")) 'No tracked-file backup'
        }
    }

    Test-Case 'invalid JSON and wrong container types are never overwritten' {
        foreach ($text in @('{broken', '[]', '{"mcpServers":null}', '{"mcpServers":[]}')) {
            $f = New-Fixture
            $path = Join-Path $f.Repo '.github\mcp.json'
            Write-FixtureFile $path $text
            $result = Invoke-FixtureSetup $f @{ EnableCopilotCli = $true }
            Assert-True ($result.ExitCode -eq 1) 'Invalid config refused'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Invalid config bytes preserved'
            Assert-True (-not (Test-Path "$path.bak")) 'No invalid backup/write'
        }
    }

    Test-Case 'missing container is added without discarding settings' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.vscode\mcp.json'
        Write-FixtureFile $path '{"inputs":[{"id":"keep"}]}'
        Assert-Success (Invoke-FixtureSetup $f @{ EnableVSCode = $true })
        Assert-True ((Get-FixtureJson $path).inputs[0].id -eq 'keep') 'Root settings preserved'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.github\mcp.json'))) 'Client switch selects only VS Code'
    }

    Test-Case 'Shared ignore precedence requires and preserves a local helper' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.gitignore'
        $text = "**/.vscode/**`n!**/.vscode/mcp.json`n"
        Write-FixtureFile $path $text
        [void](Invoke-SetupGit $f.Repo @('add', '.gitignore'))
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Tracked root ignore unchanged'
        Assert-True (Test-GitIgnoresPath $f.Repo '.vscode/mcp.json') 'VS Code config really ignored'
        Assert-True (Test-GitIgnoresPath $f.Repo '.vscode/.gitignore') 'Helper really ignored'
        Assert-True ([IO.File]::ReadAllText((Join-Path $f.Repo '.vscode\.gitignore')) -match 'mcp.json') 'Narrow helper present'
    }

    Test-Case 'tracked ignore helper prevents unsafe override' {
        $f = New-Fixture
        Write-FixtureFile (Join-Path $f.Repo '.gitignore') "!**/.vscode/mcp.json`n"
        Write-FixtureFile (Join-Path $f.Repo '.vscode\.gitignore') "# shared helper`n"
        [void](Invoke-SetupGit $f.Repo @('add', '.gitignore', '.vscode/.gitignore'))
        $result = Invoke-FixtureSetup $f @{ EnableVSCode = $true }
        Assert-True ($result.ExitCode -eq 1) 'Unsafe ignore override refused'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.vscode\mcp.json'))) 'No visible local config created'
    }

    Test-Case 'ignore helper only hides the intended directory-local file' {
        $f = New-Fixture
        Write-FixtureFile (Join-Path $f.Repo '.gitignore') "!**/.vscode/mcp.json`n"
        $nested = '.vscode/fixtures/mcp.json'
        Write-FixtureFile (Join-Path $f.Repo $nested) '{"fixture":true}'
        Assert-True (-not (Test-GitIgnoresPath $f.Repo $nested)) 'Nested config initially visible'
        $helper = Join-Path $f.Repo '.vscode\.gitignore'
        Write-FixtureFile $helper "# local settings`n/custom.txt`n"
        Assert-Success (Invoke-FixtureSetup $f @{ EnableVSCode = $true })
        Assert-True (Test-GitIgnoresPath $f.Repo '.vscode/mcp.json') 'Intended config ignored'
        Assert-True (-not (Test-GitIgnoresPath $f.Repo $nested)) 'Nested config remains visible'
        Assert-True ([IO.File]::ReadAllText($helper) -match '(?m)^/mcp\.json\r?$') 'Helper uses an anchored rule'
        Assert-True ([IO.File]::ReadAllText($helper).Contains('/custom.txt')) 'Existing helper rules preserved'
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        Assert-True (-not (Test-GitIgnoresPath $f.Repo $nested)) 'Nested config visible after uninstall'
        Assert-True ([IO.File]::ReadAllText($helper).Contains('/custom.txt')) 'Uninstall preserves unrelated helper rules'
    }

    Test-Case 'ordinary worktree setup uses common exclude and absolute worktree args' {
        $f = New-Fixture
        $worktree = Join-Path (Split-Path $f.Repo -Parent) 'linked worktree'
        [void](Invoke-SetupGit $f.Repo @('worktree', 'add', '--quiet', '--detach', $worktree))
        $f.Repo = $worktree
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True (Test-GitIgnoresPath $worktree '.github/mcp.json') 'Worktree CLI ignored'
        Assert-True (Test-GitIgnoresPath $worktree '.vscode/mcp.json') 'Worktree VS Code ignored'
        Assert-True ((Get-FixtureJson (Join-Path $worktree '.github\mcp.json')).mcpServers.xray.args[2] -ceq $worktree) 'Worktree --dir'
    }

    Test-Case 'in-memory setup requires no filter source files' {
        $f = New-Fixture
        Assert-Success (Invoke-FixtureSetup $f -Inline)
        Assert-True (Test-Path (Join-Path $f.Repo '.github\mcp.json')) 'Standalone CLI config'
    }

    Test-Case 'relative installation paths use the PowerShell location' {
        $f = New-Fixture
        Assert-Success (Invoke-FixtureSetup $f @{ InstallDir = '..\install with spaces' } -Location $f.Repo)
        Assert-True ((Get-FixtureJson (Join-Path $f.Repo '.github\mcp.json')).mcpServers.xray.command -ceq (Join-Path $f.Install 'xray.exe')) 'Relative executable resolved correctly'
    }

    Test-Case 'NonInteractive host without Force does not ask for trust' {
        $f = New-Fixture
        $result = Invoke-FixtureSetup $f @{ Force = $false }
        Assert-Success $result
        Assert-True ($result.Output -match 'No \(non-interactive\)') 'Host non-interactivity detected'
        Assert-True (-not (Test-Path (Join-Path $f.Home 'config.json'))) 'Host mode never grants trust'
    }

    Test-Case 'Force does not grant legacy cleanup permission' {
        $f = New-Fixture
        $null = Add-LegacyFixture $f
        $path = Join-Path $f.Repo '.mcp.json'
        $old = [IO.File]::ReadAllText($path)
        $result = Invoke-FixtureSetup $f
        Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'Legacy Xray state remains') 'Legacy conflict explicitly reported'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $old) 'Legacy file untouched'
        Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp\snapshot.txt')) 'Legacy metadata untouched'
        Assert-True (-not (Test-Path "$path.bak")) 'No unauthorized legacy backup'
    }

    Test-Case 'explicit migration preserves corporate bytes and clears both index flags' {
        $f = New-Fixture
        $canonical = Add-LegacyFixture $f
        $result = Invoke-FixtureSetup $f @{ MigrateLegacy = $true }
        Assert-Success $result
        $path = Join-Path $f.Repo '.mcp.json'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $canonical) 'Byte-exact snapshot removal'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'Filter directory removed'
        Assert-True ((Invoke-SetupGit $f.Repo @('config', '--get', 'filter.xray-mcp.clean') @(0, 1)).ExitCode -eq 1) 'Filter config removed'
        Assert-True (((Invoke-SetupGit $f.Repo @('ls-files', '-v', '--', '.mcp.json')).Lines -join '') -cmatch '^H ') 'Both index flags cleared'
        Assert-True ([IO.File]::ReadAllText((Join-Path $f.Repo '.git\info\attributes')) -match '\*\.txt eol=lf') 'Unrelated attributes preserved'
        Assert-True (Test-GitIgnoresPath $f.Repo '.mcp.json.bak') 'Legacy backup protected'
        Assert-Success (Invoke-FixtureSetup $f)
    }

    Test-Case 'unmarked manual root entry requires explicit consent and preserves other data' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.mcp.json'
        Write-FixtureFile $path '{"xray":{"command":"manual"},"notes":{"command":"notes"},"custom":[1,2]}'
        $result = Invoke-FixtureSetup $f @{ MigrateLegacy = $true; EnableCopilotCli = $true }
        Assert-Success $result
        $json = Get-FixtureJson $path
        Assert-True ($null -eq $json.PSObject.Properties['xray'] -and $json.notes.command -eq 'notes' -and $json.custom.Count -eq 2) 'Only root xray removed'
    }

    Test-Case 'VS Code filter migration keeps the new entry and unrelated attributes' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.vscode\mcp.json'
        Write-FixtureFile $path ('{"servers":{"notes":{"command":"notes"},"xray":{"command":"old","_xrayMcpMarker":"' + $Script:XrayMcpMarker + '"}}}')
        [void](Invoke-SetupGit $f.Repo @('config', 'filter.xray-vscode-mcp.clean', 'cat'))
        Write-FixtureFile (Join-Path $f.Repo '.git\info\attributes') ".vscode/mcp.json filter=xray-vscode-mcp text eol=lf`n"
        Write-FixtureFile (Join-Path $f.Repo '.git\xray-vscode-mcp\snapshot.txt') 'legacy'
        Assert-Success (Invoke-FixtureSetup $f @{ MigrateLegacy = $true; EnableVSCode = $true })
        $json = Get-FixtureJson $path
        Assert-True ($json.servers.xray.command -ceq (Join-Path $f.Install 'xray.exe')) 'New VS Code entry retained'
        Assert-True ($json.servers.notes.command -eq 'notes' -and $null -eq $json.servers.xray.PSObject.Properties['_xrayMcpMarker']) 'Other servers kept, sentinel retired'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-vscode-mcp'))) 'VS Code filter directory removed'
        $attributes = [IO.File]::ReadAllText((Join-Path $f.Repo '.git\info\attributes'))
        Assert-True ($attributes -match 'text eol=lf' -and $attributes -notmatch 'filter=') 'Other attributes on the same line preserved'
        Assert-Success (Invoke-FixtureSetup $f @{ EnableVSCode = $true })
    }

    Test-Case 'orphan legacy directory is detected without a root config or registration' {
        $f = New-Fixture
        Write-FixtureFile (Join-Path $f.Repo '.git\xray-mcp\snapshot.txt') 'orphan'
        Assert-Success (Invoke-FixtureSetup $f @{ MigrateLegacy = $true })
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.git\xray-mcp'))) 'Orphan metadata removed'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.mcp.json'))) 'Root config never created for orphan cleanup'
    }

    Test-Case 'legacy VS Code flags are not changed without consent' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.vscode\mcp.json'
        Write-FixtureFile $path '{"servers":{"xray":{"command":"old"}}}'
        [void](Invoke-SetupGit $f.Repo @('add', '.vscode/mcp.json'))
        [void](Invoke-SetupGit $f.Repo @('update-index', '--skip-worktree', '--', '.vscode/mcp.json'))
        $result = Invoke-FixtureSetup $f @{ EnableVSCode = $true }
        Assert-True ($result.ExitCode -eq 1) 'Legacy VS Code refused'
        Assert-True ([IO.File]::ReadAllText($path) -ceq '{"servers":{"xray":{"command":"old"}}}') 'Legacy VS Code untouched'
        Assert-True (((Invoke-SetupGit $f.Repo @('ls-files', '-v', '--', '.vscode/mcp.json')).Lines -join '') -cmatch '^S ') 'Index flag untouched'
    }

    Test-Case 'filtered migration refuses unsafe shared-worktree teardown' {
        $f = New-Fixture
        $null = Add-LegacyFixture $f
        $path = Join-Path $f.Repo '.mcp.json'
        $old = [IO.File]::ReadAllText($path)
        [void](Invoke-SetupGit $f.Repo @('worktree', 'add', '--quiet', '--detach', (Join-Path (Split-Path $f.Repo -Parent) 'second worktree')))
        $result = Invoke-FixtureSetup $f @{ MigrateLegacy = $true }
        Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'shared by multiple worktrees') 'Shared teardown failure reported'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $old) 'Legacy data untouched on refused shared teardown'
        Assert-True (Test-Path (Join-Path $f.Repo '.git\xray-mcp')) 'Shared directory preserved'
    }

    Test-Case 'uninstall DryRun, restore and repeated uninstall leave unrelated servers intact' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.github\mcp.json'
        $original = '{"mcpServers":{"notes":{"command":"notes"}}}'
        Write-FixtureFile $path $original
        Assert-Success (Invoke-FixtureSetup $f)
        $configured = [IO.File]::ReadAllText($path)
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; DryRun = $true; KeepBinary = $true })
        Assert-True ([IO.File]::ReadAllText($path) -ceq $configured) 'DryRun does not write'
        Assert-Success (Invoke-FixtureSetup $f @{ Restore = $true })
        Assert-True ([IO.File]::ReadAllText($path) -ceq $original) 'Restore exact prior bytes'
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        Assert-True ((Get-FixtureJson $path).mcpServers.notes.command -eq 'notes') 'Uninstall preserves notes'
        Assert-True (-not (Test-Path (Join-Path $f.Repo '.vscode\mcp.json'))) 'Empty generated config removed'
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        Assert-True (-not (Test-Path (Join-Path $f.Home 'config.json'))) 'Uninstall never grants/revokes trust'
    }

    Test-Case 'root CLI config creates an ignored launcher without changing tracked files' {
        $f = New-Fixture
        $rootPath = Join-Path $f.Repo '.mcp.json'
        $rootText = '{"mcpServers":{"notes":{"command":"notes"}}}'
        Write-FixtureFile $rootPath $rootText
        [void](Invoke-SetupGit $f.Repo @('add', '.mcp.json'))
        [void](Invoke-SetupGit $f.Repo @('commit', '--quiet', '-m', 'root MCP baseline'))
        $result = Invoke-FixtureSetup $f
        Assert-Success $result
        $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
        Assert-True (Test-Path $launcher) 'Root launcher created'
        Assert-True (Test-GitIgnoresPath $f.Repo 'copilot-xray.cmd') 'Launcher ignored'
        Assert-True ([IO.File]::ReadAllText($rootPath) -ceq $rootText) 'Tracked root config bytes unchanged'
        Assert-True (-not (Test-Path "$rootPath.bak")) 'Ordinary setup creates no root backup'
        Assert-True ($result.Output -match 'Start from this repository: \.\\copilot-xray\.cmd') 'Summary gives the launcher command'
        Assert-True ([IO.File]::ReadAllText($launcher) -notmatch [Regex]::Escape($f.Repo)) 'Launcher embeds no absolute repository path'
        $time = (Get-Item $launcher).LastWriteTimeUtc
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True ((Get-Item $launcher).LastWriteTimeUtc -eq $time) 'Unchanged launcher is not rewritten'
        [void](Invoke-SetupGit $f.Repo @('add', '--all'))
        Assert-True (-not (Test-IsTrackedFile $f.Repo 'copilot-xray.cmd')) 'Git add all cannot stage the launcher'
        Assert-True ((Invoke-SetupGit $f.Repo @('diff', '--cached', '--name-only')).Lines.Count -eq 0) 'Setup stages no local files'
    }

    Test-Case 'launcher is conditional on a root config and CLI selection' {
        foreach ($client in @('default', 'cli', 'vscode')) {
            $f = New-Fixture
            Write-FixtureFile (Join-Path $f.Repo '.mcp.json') '{"mcpServers":{"notes":{"command":"notes"}}}'
            $parameters = @{}
            if ($client -eq 'cli') { $parameters.EnableCopilotCli = $true }
            if ($client -eq 'vscode') { $parameters.EnableVSCode = $true }
            Assert-Success (Invoke-FixtureSetup $f $parameters)
            Assert-True ((Test-Path (Join-Path $f.Repo 'copilot-xray.cmd')) -eq ($client -ne 'vscode')) 'Only CLI selections create a launcher'
        }
        $f = New-Fixture
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True (-not (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd'))) 'No root MCP config needs no launcher'
        Write-FixtureFile (Join-Path $f.Repo '.mcp.json') '{"mcpServers":{"notes":{"command":"notes"}}}'
        Assert-Success (Invoke-FixtureSetup $f)
        $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
        Assert-Success (Invoke-FixtureSetup $f @{ EnableVSCode = $true })
        Assert-True (Test-Path $launcher) 'VS Code-only setup preserves an existing CLI launcher'
        Remove-Item -LiteralPath (Join-Path $f.Repo '.mcp.json')
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True (-not (Test-Path $launcher)) 'Removing the root config removes the obsolete launcher'
        $exclude = Get-ResolvedGitPath $f.Repo 'info/exclude'
        Assert-True ([IO.File]::ReadAllText($exclude) -notmatch '(?m)^/copilot-xray\.cmd\r?$') 'Obsolete launcher exclusion removed'
    }

    Test-Case 'tracked, foreign and directory launchers are never overwritten' {
        foreach ($kind in @('tracked', 'foreign', 'directory')) {
            $f = New-Fixture
            $rootPath = Join-Path $f.Repo '.mcp.json'
            $rootText = '{"mcpServers":{"notes":{"command":"notes"}}}'
            Write-FixtureFile $rootPath $rootText
            $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
            $original = if ($kind -eq 'tracked') { Get-CopilotLauncherText } else { '@echo user-managed launcher' }
            if ($kind -eq 'directory') { New-Item -ItemType Directory -Path $launcher | Out-Null }
            else { Write-FixtureFile $launcher $original }
            if ($kind -eq 'tracked') { [void](Invoke-SetupGit $f.Repo @('add', 'copilot-xray.cmd')) }
            $result = Invoke-FixtureSetup $f
            Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'CLI launcher:.*Failed') 'Launcher collision reports incomplete setup'
            if ($kind -eq 'directory') { Assert-True (Test-Path $launcher -PathType Container) 'Directory remains intact' }
            else { Assert-True ([IO.File]::ReadAllText($launcher) -ceq $original) 'Existing launcher bytes preserved' }
            Assert-True ([IO.File]::ReadAllText($rootPath) -ceq $rootText) 'Root config preserved on launcher failure'
            Assert-True ($result.Output -match 'Load Xray explicitly') 'Manual fallback is shown'
        }
    }

    Test-Case 'root ignore negation refuses an unprotected launcher without changing gitignore' {
        $f = New-Fixture
        Write-FixtureFile (Join-Path $f.Repo '.mcp.json') '{"mcpServers":{}}'
        $ignorePath = Join-Path $f.Repo '.gitignore'
        $ignoreText = "!/copilot-xray.cmd`n"
        Write-FixtureFile $ignorePath $ignoreText
        [void](Invoke-SetupGit $f.Repo @('add', '.gitignore'))
        $result = Invoke-FixtureSetup $f
        Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'will not change the root .gitignore') 'Ineffective root exclusion fails explicitly'
        Assert-True (-not (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd'))) 'Unprotected launcher is not written'
        Assert-True ([IO.File]::ReadAllText($ignorePath) -ceq $ignoreText) 'Tracked gitignore bytes preserved'
    }

    Test-Case 'launcher forwards quoted arguments and exit codes from a cmd-based Copilot' {
        $f = New-Fixture
        $renamed = $f.Repo + ' & special!'
        Move-Item -LiteralPath $f.Repo -Destination $renamed
        $f.Repo = $renamed
        Write-FixtureFile (Join-Path $f.Repo '.mcp.json') '{"mcpServers":{}}'
        Assert-Success (Invoke-FixtureSetup $f)
        Write-FixtureFile (Join-Path $f.Install 'copilot.cmd') (@(
            '@echo off'
            'echo CWD="%CD%"'
            'echo DIRECTORY_OPTION=%1'
            'echo DIRECTORY=%2'
            'echo OPTION=%3'
            'echo CONFIG=%4'
            'echo EXTRA=%5'
            'echo VALUE=%6'
            'exit /b 23'
        ) -join "`r`n")
        foreach ($value in @('hello world & tools!', '50% done', '100%', '%XRAY_LAUNCHER_PROBE%', 'a ^ b')) {
            $result = Invoke-FixtureLauncher $f ('-p "' + $value + '"')
            Assert-True ($result.ExitCode -eq 23) 'CLI exit code forwarded'
            Assert-True ($result.Output.Contains('CWD="' + $f.Install + '"')) 'Launcher does not change the process directory'
            Assert-True ($result.Output.Contains('DIRECTORY_OPTION=-C') -and $result.Output.Contains('DIRECTORY="' + $f.Repo + '\."')) 'CLI session directory is set to the repository root'
            Assert-True ($result.Output.Contains('OPTION=--additional-mcp-config')) 'Explicit config flag forwarded'
            Assert-True ($result.Output.Contains('CONFIG="@' + (Join-Path $f.Repo '.github\mcp.json') + '"')) 'Config path handles spaces and metacharacters'
            Assert-True ($result.Output.Contains('EXTRA=-p') -and $result.Output.Contains('VALUE="' + $value + '"')) 'User argument boundaries and literal metacharacters preserved'
        }
        Write-FixtureFile (Join-Path $f.Install 'outer.cmd') (@(
            '@echo off'
            'setlocal DisableDelayedExpansion'
            ('call "' + (Join-Path $f.Repo 'copilot-xray.cmd') + '"')
            'echo RETURNED_EXIT=%errorlevel%'
            'echo RETURNED_CWD="%CD%"'
        ) -join "`r`n")
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = $env:ComSpec
        $info.Arguments = '/d /s /c ""' + (Join-Path $f.Install 'outer.cmd') + '""'
        $info.WorkingDirectory = $f.Install
        $info.UseShellExecute = $false
        $info.RedirectStandardOutput = $true
        $info.RedirectStandardError = $true
        $info.EnvironmentVariables['PATH'] = $f.Install + ';' + (Join-Path $env:SystemRoot 'System32')
        $result = Invoke-FixtureProcess -Info $info -Label 'Caller directory check'
        Assert-True ($result.Output.Contains('RETURNED_EXIT=23')) 'Cmd caller receives the CLI exit code'
        Assert-True ($result.Output.Contains('RETURNED_CWD="' + $f.Install + '"')) 'Cmd caller keeps its working directory'
        Remove-Item -LiteralPath (Join-Path $f.Repo '.github\mcp.json')
        $result = Invoke-FixtureLauncher $f
        Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'configuration was not found') 'Missing local MCP config fails explicitly'
        Assert-True ($result.Output -notmatch 'OPTION=') 'Missing config does not launch CLI'
    }

    Test-Case 'launcher forwards literal arguments and exit codes from a native Copilot' {
        $f = New-Fixture
        Write-FixtureFile (Join-Path $f.Repo '.mcp.json') '{"mcpServers":{}}'
        Assert-Success (Invoke-FixtureSetup $f)
        $code = @'
$ErrorActionPreference = 'Stop'
Add-Type -OutputAssembly $env:XRAY_LAUNCHER_STUB -OutputType ConsoleApplication -TypeDefinition @"
using System;
public static class CliStub {
    public static int Main(string[] args) {
        Console.WriteLine("CWD=" + Environment.CurrentDirectory);
        for (int i = 0; i < args.Length; i++) Console.WriteLine("ARG{0}={1}", i, args[i]);
        return 23;
    }
}
"@
'@
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $info.Arguments = '-NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
        $info.UseShellExecute = $false
        $info.RedirectStandardOutput = $true
        $info.RedirectStandardError = $true
        $info.EnvironmentVariables['XRAY_LAUNCHER_STUB'] = Join-Path $f.Install 'copilot.exe'
        Assert-Success (Invoke-FixtureProcess -Info $info -Label 'Native CLI stub compilation')
        $result = Invoke-FixtureLauncher $f '-p "50% done ^ here & tools!"'
        Assert-True ($result.ExitCode -eq 23) 'Native CLI exit code forwarded'
        Assert-True ($result.Output.Contains('CWD=' + $f.Install)) 'Native CLI process does not change the caller directory'
        Assert-True ($result.Output.Contains('ARG0=-C') -and $result.Output.Contains('ARG1=' + $f.Repo + '\.')) 'Native CLI receives the repository session directory'
        Assert-True ($result.Output.Contains('ARG2=--additional-mcp-config') -and $result.Output.Contains('ARG3=@' + (Join-Path $f.Repo '.github\mcp.json'))) 'Native CLI receives the local MCP config'
        Assert-True ($result.Output.Contains('ARG4=-p') -and $result.Output.Contains('ARG5=50% done ^ here & tools!')) 'Native CLI receives exact literal prompt characters'
    }

    Test-Case 'launcher restore, dry-run and uninstall preserve the root config' {
        $f = New-Fixture
        $rootPath = Join-Path $f.Repo '.mcp.json'
        $rootText = '{"mcpServers":{"notes":{"command":"notes"}}}'
        Write-FixtureFile $rootPath $rootText
        Write-FixtureFile (Join-Path $f.Repo '.github\mcp.json') '{"mcpServers":{"local-notes":{"command":"local-notes"}}}'
        Assert-Success (Invoke-FixtureSetup $f)
        $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
        $launcherText = [IO.File]::ReadAllText($launcher)
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; DryRun = $true; KeepBinary = $true })
        Assert-True ([IO.File]::ReadAllText($launcher) -ceq $launcherText) 'Uninstall dry run preserves launcher bytes'
        Assert-Success (Invoke-FixtureSetup $f @{ Restore = $true })
        Assert-True (-not (Test-Path $launcher)) 'Restore without Xray removes the launcher'
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        Assert-True (-not (Test-Path $launcher)) 'Uninstall removes managed launcher'
        Assert-True ([IO.File]::ReadAllText($rootPath) -ceq $rootText) 'Lifecycle never changes root config bytes'
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
    }

    Test-Case 'restore continues both clients when a tracked launcher cannot be removed' {
        $f = New-Fixture
        $rootPath = Join-Path $f.Repo '.mcp.json'
        $rootText = '{"mcpServers":{"notes":{"command":"notes"}}}'
        Write-FixtureFile $rootPath $rootText
        $cliPath = Join-Path $f.Repo '.github\mcp.json'
        $vscodePath = Join-Path $f.Repo '.vscode\mcp.json'
        $cliText = '{"mcpServers":{"cli-notes":{"command":"cli-notes"}}}'
        $vscodeText = '{"servers":{"editor-notes":{"command":"editor-notes"}}}'
        Write-FixtureFile $cliPath $cliText
        Write-FixtureFile $vscodePath $vscodeText
        Assert-Success (Invoke-FixtureSetup $f)
        $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
        $launcherText = [IO.File]::ReadAllText($launcher)
        [void](Invoke-SetupGit $f.Repo @('add', '--force', 'copilot-xray.cmd'))
        $result = Invoke-FixtureSetup $f @{ Restore = $true }
        Assert-True ($result.ExitCode -eq 1) 'Launcher cleanup failure returns nonzero'
        Assert-True ($result.Output -match 'Could not clean up Copilot CLI launcher during restore:' -and $result.Output -match 'Not removing a tracked launcher') 'Tracked launcher failure is reported explicitly'
        Assert-True ($result.Output -match 'Restore incomplete') 'Partial result is reported in the summary'
        Assert-True ([IO.File]::ReadAllText($cliPath) -ceq $cliText) 'CLI backup restored exactly'
        Assert-True ([IO.File]::ReadAllText($vscodePath) -ceq $vscodeText) 'VS Code backup restored despite launcher failure'
        Assert-True (-not (Test-Path "$cliPath.bak") -and -not (Test-Path "$vscodePath.bak")) 'Both consumed backups removed'
        Assert-True ([IO.File]::ReadAllText($launcher) -ceq $launcherText) 'Tracked launcher bytes preserved'
        Assert-True (Test-IsTrackedFile $f.Repo 'copilot-xray.cmd') 'Launcher index state preserved'
        Assert-True ([IO.File]::ReadAllText($rootPath) -ceq $rootText) 'Root config unchanged'
    }

    Test-Case 'restore removes the launcher when the root config is gone even if Xray remains' {
        $f = New-Fixture
        $rootPath = Join-Path $f.Repo '.mcp.json'
        Write-FixtureFile $rootPath '{"mcpServers":{}}'
        $cliPath = Join-Path $f.Repo '.github\mcp.json'
        $cliText = '{"mcpServers":{"xray":{"command":"prior-xray"}}}'
        Write-FixtureFile $cliPath $cliText
        Assert-Success (Invoke-FixtureSetup $f)
        Remove-Item -LiteralPath $rootPath
        Assert-Success (Invoke-FixtureSetup $f @{ Restore = $true })
        Assert-True ([IO.File]::ReadAllText($cliPath) -ceq $cliText) 'Restored config retains its original Xray entry'
        Assert-True (-not (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd'))) 'No root config needs no launcher after restore'
        $exclude = Get-ResolvedGitPath $f.Repo 'info/exclude'
        Assert-True ([IO.File]::ReadAllText($exclude) -notmatch '(?m)^/copilot-xray\.cmd\r?$') 'Obsolete launcher exclusion removed'
    }

    Test-Case 'launcher exclude cleanup skips Git when no exact launcher rule exists' {
        $f = New-Fixture
        $exclude = Join-Path $f.Home 'exclude'
        function Invoke-SetupGit {
            param([string]$RepoRoot, [string[]]$Arguments)
            throw "Unexpected Git query during launcher exclusion cleanup: $RepoRoot $Arguments"
        }
        Remove-CopilotLauncherExclude -RepoRoot $f.Repo -ExcludePath $exclude
        Assert-True (-not (Test-Path $exclude)) 'Missing exclude file is not created'
        $text = "# /copilot-xray.cmd`r`n/unrelated-local-file`r`n"
        Write-FixtureFile $exclude $text
        Remove-CopilotLauncherExclude -RepoRoot $f.Repo -ExcludePath $exclude
        Assert-True ([IO.File]::ReadAllText($exclude) -ceq $text) 'Unrelated exclude content is untouched without querying Git'
    }

    Test-Case 'uninstall preserves tracked and foreign launchers' {
        foreach ($tracked in @($false, $true)) {
            $f = New-Fixture
            $launcher = Join-Path $f.Repo 'copilot-xray.cmd'
            $text = if ($tracked) { Get-CopilotLauncherText } else { '@echo foreign launcher' }
            Write-FixtureFile $launcher $text
            if ($tracked) { [void](Invoke-SetupGit $f.Repo @('add', 'copilot-xray.cmd')) }
            $result = Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true }
            Assert-True ($result.ExitCode -eq $(if ($tracked) { 1 } else { 0 })) 'Tracked cleanup fails; foreign launcher is not owned'
            Assert-True ([IO.File]::ReadAllText($launcher) -ceq $text) 'Uninstall preserves protected launcher bytes'
        }
    }

    Test-Case 'worktree launcher cleanup preserves shared exclusions until no launcher remains' {
        $f = New-Fixture
        $mainRepo = $f.Repo
        $rootText = '{"mcpServers":{}}'
        Write-FixtureFile (Join-Path $mainRepo '.mcp.json') $rootText
        Write-FixtureFile (Join-Path $mainRepo '.github\mcp.json') '{"mcpServers":{"notes":{"command":"notes"}}}'
        Assert-Success (Invoke-FixtureSetup $f)
        $mainLauncher = Join-Path $mainRepo 'copilot-xray.cmd'
        $worktree = Join-Path (Split-Path $mainRepo -Parent) 'linked worktree'
        [void](Invoke-SetupGit $mainRepo @('worktree', 'add', '--quiet', '--detach', $worktree))
        $f.Repo = $worktree
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True (Test-Path $mainLauncher) 'Setup without a root config preserves the other launcher'
        Assert-True (Test-GitIgnoresPath $mainRepo 'copilot-xray.cmd') 'Absent launcher cleanup keeps the other worktree protected'
        Write-FixtureFile (Join-Path $worktree '.mcp.json') $rootText
        Assert-Success (Invoke-FixtureSetup $f)
        $linkedLauncher = Join-Path $worktree 'copilot-xray.cmd'
        Assert-True (Test-Path $linkedLauncher) 'Linked worktree launcher created'
        $f.Repo = $mainRepo
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        Assert-True (-not (Test-Path $mainLauncher)) 'Main worktree launcher removed'
        Assert-True (Test-GitIgnoresPath $worktree 'copilot-xray.cmd') 'Existing launcher keeps its shared exclude rule'
        Remove-Item -LiteralPath $linkedLauncher
        $f.Repo = $worktree
        Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true })
        $exclude = Get-ResolvedGitPath $worktree 'info/exclude'
        Assert-True ([IO.File]::ReadAllText($exclude) -notmatch '(?m)^/copilot-xray\.cmd\r?$') 'Shared exclude removed when no launcher remains'
        Assert-True ([IO.File]::ReadAllText((Join-Path $mainRepo '.mcp.json')) -ceq $rootText) 'Main root config unchanged'
        Assert-True ([IO.File]::ReadAllText((Join-Path $worktree '.mcp.json')) -ceq $rootText) 'Linked root config unchanged'
    }

    Test-Case 'missing launcher cleanup removes only its exclude rule outside dry runs' {
        foreach ($mode in @('setup', 'uninstall', 'restore')) {
            $f = New-Fixture
            $rootPath = Join-Path $f.Repo '.mcp.json'
            Write-FixtureFile $rootPath '{"mcpServers":{}}'
            Write-FixtureFile (Join-Path $f.Repo '.github\mcp.json') '{"mcpServers":{"notes":{"command":"notes"}}}'
            Assert-Success (Invoke-FixtureSetup $f)
            Remove-Item -LiteralPath (Join-Path $f.Repo 'copilot-xray.cmd')
            $exclude = Get-ResolvedGitPath $f.Repo 'info/exclude'
            [void](Add-TextLineNoBom -Path $exclude -Line '/unrelated-local-file')
            $original = [IO.File]::ReadAllText($exclude)
            Assert-True ($original -match '(?m)^/copilot-xray\.cmd\r?$') 'Manual deletion leaves the launcher exclude rule'
            Assert-Success (Invoke-FixtureSetup $f @{ Uninstall = $true; DryRun = $true; KeepBinary = $true })
            Assert-True ([IO.File]::ReadAllText($exclude) -ceq $original) 'Dry run preserves stale exclude rules'
            $parameters = @{}
            if ($mode -eq 'setup') { Remove-Item -LiteralPath $rootPath }
            if ($mode -eq 'uninstall') { $parameters = @{ Uninstall = $true; KeepBinary = $true } }
            if ($mode -eq 'restore') { $parameters = @{ Restore = $true } }
            Assert-Success (Invoke-FixtureSetup $f $parameters)
            $text = [IO.File]::ReadAllText($exclude)
            Assert-True ($text -notmatch '(?m)^/copilot-xray\.cmd\r?$') 'Missing launcher exclude rule removed'
            Assert-True ($text -match '(?m)^/unrelated-local-file\r?$') 'Unrelated exclude rule preserved'
            Assert-True (-not (Test-Path (Join-Path $f.Repo 'copilot-xray.cmd'))) 'Cleanup does not recreate the launcher'
        }
    }

    Test-Case 'backup failure leaves the existing MCP config unchanged' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.github\mcp.json'
        $old = '{"mcpServers":{"notes":{"command":"notes"}}}'
        Write-FixtureFile $path $old
        New-Item -ItemType Directory -Path "$path.bak" | Out-Null
        $result = Invoke-FixtureSetup $f @{ EnableCopilotCli = $true }
        Assert-True ($result.ExitCode -eq 1) 'Backup failure reported'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $old) 'No overwrite after backup failure'
    }

    Test-Case 'atomic writer rejects concurrent config changes' {
        $f = New-Fixture
        $path = Join-Path $f.Home 'config.json'
        Write-FixtureFile $path '{"trustedFolders":[]}'
        $document = Read-SetupJson $path
        $document.Value | Add-Member -NotePropertyName 'new' -NotePropertyValue $true
        Write-FixtureFile $path '{"trustedFolders":[],"concurrent":true}'
        $failed = $false
        try { [void](Write-SetupJson $document -Backup) }
        catch { $failed = $_.Exception.Message -match 'changed during setup' }
        Assert-True $failed 'Concurrent modification refused'
        Assert-True ((Get-FixtureJson $path).concurrent) 'Concurrent data preserved'
    }

    Test-Case 'uninstall refuses concurrent edits before deletion or rewrite and keeps the backup' {
        foreach ($original in @('{"mcpServers":{"xray":{"command":"xray"}}}', '{"mcpServers":{"xray":{"command":"xray"},"notes":{"command":"old-notes"}}}')) {
            $f = New-Fixture
            $path = Join-Path $f.Repo '.github\mcp.json'
            Write-FixtureFile $path $original
            $concurrent = '{"mcpServers":{"xray":{"command":"xray"},"notes":{"command":"new-notes"}}}'
            $result = Invoke-FixtureSetup $f @{ Uninstall = $true; KeepBinary = $true } -BackupReplacement $concurrent
            Assert-True ($result.ExitCode -eq 1 -and $result.Output -match 'changed during setup') 'Concurrent uninstall fails explicitly'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $concurrent) 'Concurrent server preserved'
            Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq $concurrent) 'Backup retained after failed uninstall'
        }
    }

    Test-Case 'depth-100 JSON is preserved and deeper input is refused without changing the config or backup' {
        foreach ($depth in @(100, 101, 110)) {
            $f = New-Fixture
            $path = Join-Path $f.Repo '.github\mcp.json'
            $text = '{"mcpServers":{"notes":{"command":"notes"}},"custom":' + ('{"nested":' * $depth) + '1' + ('}' * $depth) + '}'
            Write-FixtureFile $path $text
            Write-FixtureFile "$path.bak" '{"existingBackup":true}'
            $result = Invoke-FixtureSetup $f @{ EnableCopilotCli = $true }
            if ($depth -eq 100) {
                Assert-Success $result
                $node = (Get-FixtureJson $path).custom
                $levels = 0
                while ($null -ne $node.PSObject.Properties['nested']) { $levels++; $node = $node.nested }
                Assert-True ($levels -eq 100 -and $node -eq 1) 'Depth boundary retains every nested object and scalar'
                Assert-True ((Get-FixtureJson $path).mcpServers.notes.command -eq 'notes') 'Other server preserved at depth boundary'
                Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq $text) 'Accepted config backed up without truncation'
            }
            else {
                Assert-True ($result.ExitCode -eq 1) 'Unsupported depth fails explicitly'
                Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Deep config bytes preserved'
                Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq '{"existingBackup":true}') 'Existing backup not overwritten on serialization failure'
            }
        }
    }

    Test-Case 'deep JSON refuses legacy removal without touching the root or its existing backup' {
        $f = New-Fixture
        $path = Join-Path $f.Repo '.mcp.json'
        $text = '{"mcpServers":{"xray":{"command":"old"},"notes":{"command":"notes"}},"custom":' + ('{"nested":' * 110) + '1' + ('}' * 110) + '}'
        Write-FixtureFile $path $text
        Write-FixtureFile "$path.bak" '{"existingBackup":true}'
        $result = Invoke-FixtureSetup $f @{ MigrateLegacy = $true; EnableCopilotCli = $true }
        Assert-True ($result.ExitCode -eq 1) 'Unsupported legacy depth fails explicitly'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Legacy root bytes preserved'
        Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq '{"existingBackup":true}') 'Legacy backup preserved on failed serialization'
    }

    function Read-Host {
        param([string]$Prompt)
        $script:trustPrompts++
        return $script:trustAnswer
    }
    Test-Case 'interactive legacy migration defaults to No without modifying anything' {
        $f = New-Fixture
        $null = Add-LegacyFixture $f
        $path = Join-Path $f.Repo '.mcp.json'
        $old = [IO.File]::ReadAllText($path)
        $states = @(Get-LegacyMcpState $f.Repo $true | Where-Object { $_.Found })
        $script:trustAnswer = ''
        $script:trustPrompts = 0
        Assert-True (-not (Request-LegacyMigration $states $true)) 'Enter does not authorize migration'
        Assert-True ($script:trustPrompts -eq 1) 'Migration prompt shown once'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $old) 'Consent prompt does not modify legacy state'
    }

    Test-Case 'trust default Enter, existing settings, duplicate paths and backups' {
        $f = New-Fixture
        $env:COPILOT_HOME = $f.Home
        $path = Join-Path $f.Home 'config.json'
        $old = '{"trustedFolders":["C:\\Other"],"token":"fixture-secret","custom":{"values":[1,2]},"time":"2026-10-11T08:00:00Z"}'
        Write-FixtureFile $path $old
        $script:trustAnswer = ''
        $script:trustPrompts = 0
        $status = Set-CopilotWorkspaceTrust $f.Repo $true
        Assert-True ($status -eq 'Yes' -and $script:trustPrompts -eq 1) 'Enter accepts trust'
        $json = Get-FixtureJson $path
        Assert-True ($json.trustedFolders.Count -eq 2 -and $json.custom.values.Count -eq 2 -and $json.token -eq 'fixture-secret') 'Existing folders/settings preserved'
        Assert-True ($json.time -is [string] -and $json.time -ceq '2026-10-11T08:00:00Z') 'Unrelated date-looking string preserved'
        Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq $old) 'Trust backup exact'
        $time = (Get-Item $path).LastWriteTimeUtc
        $status = Set-CopilotWorkspaceTrust ($f.Repo.ToUpperInvariant() + '\') $true
        Assert-True ($status -eq 'Yes (already trusted)' -and $script:trustPrompts -eq 1) 'Case/trailing slash duplicate avoided'
        Assert-True ((Get-Item $path).LastWriteTimeUtc -eq $time) 'Trusted rerun does not write'
    }

    Test-Case 'Copilot-generated header comments preserve trust settings and backups' {
        foreach ($newline in @("`n", "`r`n", "`r")) {
            $f = New-Fixture
            $env:COPILOT_HOME = $f.Home
            $path = Join-Path $f.Home 'config.json'
            $old = @(
                ''
                '  // User settings belong in settings.json.'
                '// This file is managed automatically.'
                '{"firstLaunchAt":"2026-07-19T15:03:21.567Z","custom":"https://example.invalid/*literal*/\"//literal","staff":true}'
            ) -join $newline
            Write-FixtureFile $path $old
            $script:trustAnswer = 'n'
            $script:trustPrompts = 0
            Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'No (declined)') 'Header does not block decline'
            Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $false) -eq 'No (non-interactive)') 'Header does not block non-interactive skip'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $old) 'No consent preserves original comments and bytes'
            Assert-True (-not (Test-Path "$path.bak")) 'No consent creates no backup'
            $script:trustAnswer = ''
            Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'Yes') 'Commented Copilot config accepts trust'
            $json = Get-FixtureJson $path
            Assert-True ($json.trustedFolders -is [array] -and $json.trustedFolders.Count -eq 1 -and $json.trustedFolders[0] -eq $f.Repo) 'Repository added as a single trusted folder'
            Assert-True ($json.firstLaunchAt -is [string] -and $json.firstLaunchAt -ceq '2026-07-19T15:03:21.567Z') 'First-launch timestamp preserved as a string'
            Assert-True ($json.staff -and $json.custom -ceq 'https://example.invalid/*literal*/"//literal') 'Unrelated settings and comment-like strings preserved'
            Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq $old) 'Backup retains original comment header and bytes'
            Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'Yes (already trusted)') 'Commented config trust is idempotent'
            Assert-True ($script:trustPrompts -eq 2) 'Only decline and consent prompt'
        }
    }

    Test-Case 'PowerShell 7 token reader accepts Copilot-generated header comments' {
        if ($PSVersionTable.PSVersion.Major -lt 7) { return }
        $realGetCommand = $ExecutionContext.InvokeCommand.GetCommand('Get-Command', [Management.Automation.CommandTypes]::Cmdlet)
        function Get-Command {
            param([string]$Name)
            if ($Name -eq 'ConvertFrom-Json') {
                return [PSCustomObject]@{ Parameters = @{} }
            }
            return & $realGetCommand -Name $Name
        }
        $text = "// User settings belong in settings.json.`n// This file is managed automatically.`n" +
            '{"firstLaunchAt":"2026-07-19T15:03:21.567Z","trustedFolders":[]}'
        $json = ConvertFrom-SetupJson -Text $text -Path 'fixture.json'
        Assert-True ($json.firstLaunchAt -is [string] -and $json.firstLaunchAt -ceq '2026-07-19T15:03:21.567Z') 'Legacy PowerShell 7 parser retains the timestamp'
        Assert-True ($json.trustedFolders -is [array] -and $json.trustedFolders.Count -eq 0) 'Legacy PowerShell 7 parser retains an empty trust array'
        $failed = $false
        try { $null = ConvertFrom-SetupJson -Text ($text + '{}') -Path 'fixture.json' }
        catch { $failed = $_.Exception.Message -match 'Invalid JSON' }
        Assert-True $failed 'Legacy PowerShell 7 parser still rejects trailing JSON'
    }

    Test-Case 'trust missing config/property, decline and non-interactive paths' {
        $f = New-Fixture
        $env:COPILOT_HOME = $f.Home
        $path = Join-Path $f.Home 'config.json'
        $script:trustAnswer = 'n'
        $script:trustPrompts = 0
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'No (declined)') 'Decline respected'
        Assert-True (-not (Test-Path $path)) 'Decline creates no config'
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $false) -eq 'No (non-interactive)') 'Non-interactive skip'
        Assert-True ($script:trustPrompts -eq 1 -and -not (Test-Path $path)) 'Non-interactive does not prompt/write'
        $script:trustAnswer = ''
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'Yes') 'Missing config created'
        Assert-True ((Get-FixtureJson $path).trustedFolders -is [array]) 'Single trusted folder remains an array'
        Write-FixtureFile $path '{"custom":true}'
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'Yes') 'Missing property added'
        Assert-True ((Get-FixtureJson $path).custom) 'Unrelated property retained'
    }

    Test-Case 'invalid trust JSON/schema preserves bytes and does not log credentials' {
        foreach ($text in @(
            '{"token":"fixture-secret",bad'
            '{"trustedFolders":null}'
            '{"trustedFolders":"wrong"}'
            '{"trustedFolders":[null]}'
            "// Copilot header`n" + '{"token":"fixture-secret",bad'
            "// Copilot header`n" + '{"trustedFolders":[]} {}'
            "// Copilot header`n"
            '{"custom":"// not a header","token":"fixture-secret",bad'
        )) {
            $f = New-Fixture
            $env:COPILOT_HOME = $f.Home
            $path = Join-Path $f.Home 'config.json'
            Write-FixtureFile $path $text
            $warnings = @()
            $status = Set-CopilotWorkspaceTrust $f.Repo $true -WarningVariable warnings -WarningAction SilentlyContinue
            Assert-True ($status -eq 'No (error)') 'Invalid trust schema reported'
            Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Invalid trust config preserved'
            Assert-True (($warnings -join '') -notmatch 'fixture-secret') 'Credentials absent from diagnostics'
        }
    }

    Test-Case 'trust backup failure is isolated from MCP installation' {
        $f = New-Fixture
        $env:COPILOT_HOME = $f.Home
        $path = Join-Path $f.Home 'config.json'
        Write-FixtureFile $path '{"trustedFolders":[],"custom":true}'
        New-Item -ItemType Directory -Path "$path.bak" | Out-Null
        $script:trustAnswer = ''
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'No (error)') 'Trust backup failure reported'
        Assert-True ((Get-FixtureJson $path).trustedFolders.Count -eq 0) 'Trust config not overwritten'
        Assert-Success (Invoke-FixtureSetup $f)
        Assert-True (Test-Path (Join-Path $f.Repo '.github\mcp.json')) 'MCP setup is independent of trust failure'
    }

    Test-Case 'malformed Copilot config does not fail the MCP installation' {
        $f = New-Fixture
        $path = Join-Path $f.Home 'config.json'
        Write-FixtureFile $path '{"trustedFolders":"wrong"}'
        $result = Invoke-FixtureSetup $f
        Assert-Success $result
        Assert-True ($result.Output -match 'No \(error\)') 'Trust error reported separately'
        Assert-True ([IO.File]::ReadAllText($path) -ceq '{"trustedFolders":"wrong"}') 'Bad Copilot config left untouched'
        Assert-True (Test-Path (Join-Path $f.Repo '.github\mcp.json')) 'CLI MCP config still written'
    }

    Test-Case 'deep trust settings are refused without overwriting settings or backups' {
        $f = New-Fixture
        $env:COPILOT_HOME = $f.Home
        $path = Join-Path $f.Home 'config.json'
        $text = '{"trustedFolders":[],"token":"fixture-secret","custom":' + ('{"nested":' * 110) + '1' + ('}' * 110) + '}'
        Write-FixtureFile $path $text
        Write-FixtureFile "$path.bak" '{"existingBackup":true}'
        $script:trustAnswer = ''
        $warnings = @()
        $status = Set-CopilotWorkspaceTrust $f.Repo $true -WarningVariable warnings -WarningAction SilentlyContinue
        Assert-True ($status -eq 'No (error)') 'Unsupported trust depth reported separately'
        Assert-True ([IO.File]::ReadAllText($path) -ceq $text) 'Deep trust settings bytes preserved'
        Assert-True ([IO.File]::ReadAllText("$path.bak") -ceq '{"existingBackup":true}') 'Trust backup retained unchanged'
        Assert-True (($warnings -join '') -notmatch 'fixture-secret') 'Serialization error does not log credentials'
    }

    if ($PSVersionTable.PSVersion.Major -ge 7) {
        Test-Case 'PowerShell 7.0-7.4 JSON reader preserves string dates, nulls and arrays' {
            function Get-Command {
                param([string]$Name)
                return [PSCustomObject]@{ Parameters = @{} }
            }
            $json = ConvertFrom-SetupJson -Path 'fixture.json' -Text '{"date":"2026-10-11T08:00:00Z","items":[null,[],{"date":"2026-10-11T08:00:00Z"}]}'
            Assert-True ($json.date -is [string] -and $json.date -ceq '2026-10-11T08:00:00Z') 'String date type preserved'
            Assert-True ($json.items.Count -eq 3 -and $null -eq $json.items[0]) 'Array null element preserved'
            Assert-True ($json.items[1] -is [array] -and $json.items[1].Count -eq 0) 'Empty array preserved'
            Assert-True ($json.items[2].date -is [string]) 'Nested string date preserved'
            $boundary = '{"custom":' + ('{"nested":' * 100) + '1' + ('}' * 100) + '}'
            $parsedBoundary = ConvertFrom-SetupJson -Path 'fixture.json' -Text $boundary
            Assert-True ((ConvertTo-SetupJson -Value $parsedBoundary -Path 'fixture.json' -Compress) -ceq $boundary) 'Fallback preserves the supported depth boundary'
            $tooDeep = '{"custom":' + ('{"nested":' * 101) + '1' + ('}' * 101) + '}'
            foreach ($invalid in @('{}{}', '{} trailing', '[]', $tooDeep)) {
                $rejected = $false
                try { $null = ConvertFrom-SetupJson -Path 'fixture.json' -Text $invalid }
                catch { $rejected = $true }
                Assert-True $rejected 'Fallback parser rejects invalid or non-object input'
            }
        }
    }

    Test-Case 'trusted parent suppresses the trust prompt' {
        $f = New-Fixture
        $env:COPILOT_HOME = $f.Home
        Write-FixtureFile (Join-Path $f.Home 'config.json') (@{ trustedFolders = @((Split-Path $f.Repo -Parent)) } | ConvertTo-Json)
        $script:trustPrompts = 0
        Assert-True ((Set-CopilotWorkspaceTrust $f.Repo $true) -eq 'Yes (already trusted)') 'Parent directory trust respected'
        Assert-True ($script:trustPrompts -eq 0) 'No duplicate child prompt'
    }
    }
}
finally {
    $env:GIT_CONFIG_GLOBAL = $oldGlobal
    $env:GIT_CONFIG_NOSYSTEM = $oldSystem
    $env:COPILOT_HOME = $oldCopilotHome
    Remove-Item -LiteralPath $root -Recurse -Force
}
Write-Host "MCP setup assertions passed: $script:passes; cases failed: $script:failures"
if ($script:passes -eq 0 -and $script:failures -eq 0) { Write-Error "No test cases matched: $CasePattern"; exit 1 }
if ($script:failures -gt 0) { exit 1 }
exit 0
