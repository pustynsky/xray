#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Isolated regression tests for local MCP setup and explicit legacy migration.
#>
param()

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
    param($Fixture, [hashtable]$Parameters = @{}, [switch]$Inline, [string]$Location, [string]$BackupReplacement)
    $arguments = @{
        RepoPath = $Fixture.Repo; InstallDir = $Fixture.Install
        SkipDownload = $true; Extensions = ' .CS, md,cs '; Force = $true
    }
    foreach ($key in $Parameters.Keys) { $arguments[$key] = $Parameters[$key] }
    $payload = @{ Path = $scriptPath; Arguments = $arguments; Inline = [bool]$Inline; Location = $Location; BackupReplacement = $BackupReplacement } | ConvertTo-Json -Depth 10 -Compress
    $encodedPayload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $code = @'
$ErrorActionPreference = 'Stop'
$payload = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
$parameters = @{}
foreach ($property in $payload.Arguments.PSObject.Properties) { $parameters[$property.Name] = $property.Value }
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
if ($payload.Inline) {
    & ([scriptblock]::Create([IO.File]::ReadAllText($payload.Path))) @parameters
}
else {
    & $payload.Path @parameters
}
exit $LASTEXITCODE
'@.Replace('__PAYLOAD__', $encodedPayload)
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $powerShellExe
    $info.Arguments = '-NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['COPILOT_HOME'] = $Fixture.Home
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    try {
        [void]$process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) { throw 'Setup timed out' }
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

function Get-FixtureJson {
    param([string]$Path)
    return (Read-SetupJson -Path $Path).Value
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
        foreach ($text in @('{"token":"fixture-secret",bad', '{"trustedFolders":null}', '{"trustedFolders":"wrong"}', '{"trustedFolders":[null]}')) {
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
finally {
    $env:GIT_CONFIG_GLOBAL = $oldGlobal
    $env:GIT_CONFIG_NOSYSTEM = $oldSystem
    $env:COPILOT_HOME = $oldCopilotHome
    Remove-Item -LiteralPath $root -Recurse -Force
}
Write-Host "MCP setup assertions passed: $script:passes; cases failed: $script:failures"
if ($script:failures -gt 0) { exit 1 }
exit 0
