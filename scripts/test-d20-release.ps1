#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary,
    [switch]$BuildRelease,
    [switch]$IncludeBaselineControl,
    [switch]$KeepArtifacts,
    [string]$OutputPath,
    [ValidateRange(5, 600)]
    [int]$TimeoutSeconds = 120,
    [ValidateRange(60, 7200)]
    [int]$BuildTimeoutSeconds = 1800
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Checks = [System.Collections.Generic.List[object]]::new()
$script:QueryTimings = [ordered]@{}
$script:McpSession = $null
$script:Failure = $null
$script:CleanupStatus = 'notStarted'
$script:ArtifactCleanupStatus = 'notStarted'
$script:FixtureDirectory = $null
$script:CandidateBinary = $null
$script:CandidateCommit = $null
$script:FixtureManifestSha256 = $null
$script:FixtureManifest = @()
$script:CompilerOracle = [ordered]@{ status = 'notRun' }
$script:Timings = [ordered]@{}
$script:BaselineWorktree = $null
$script:BaselineControl = [ordered]@{ status = 'notRun' }
$script:MigrationSmoke = [ordered]@{ status = 'notRun' }
$script:LifecycleSmoke = [ordered]@{ status = 'notRun' }
$script:CargoLockSha256 = $null
$scriptStartUtc = [DateTime]::UtcNow.ToString('o')

$RepoRoot = Split-Path -Parent $PSScriptRoot
$BaselineCommit = 'ef14a4875914fb8a5323313001f06c246688513c'
$RunId = [Guid]::NewGuid().ToString('N')
$ArtifactRoot = Join-Path $RepoRoot "target/d20-release/$RunId"
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $RepoRoot 'target/d20-release-result.json'
}
elseif (-not [IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path $RepoRoot $OutputPath
}

function Add-Check {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [bool]$Passed,
        [Parameter(Mandatory)]
        [string]$Details,
        [long]$DurationMs = 0
    )

    $script:Checks.Add([ordered]@{
        name = $Name
        status = if ($Passed) { 'passed' } else { 'failed' }
        passed = $Passed
        details = $Details
        durationMs = $DurationMs
    })
}

function Assert-D20 {
    param(
        [Parameter(Mandatory)]
        [bool]$Condition,
        [Parameter(Mandatory)]
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$Path)

    $bytes = [IO.File]::ReadAllBytes($Path)
    return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = $RepoRoot,
        [int]$ProcessTimeoutSeconds = $TimeoutSeconds
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $ArgumentList) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    try {
        if (-not $process.Start()) {
            throw "Failed to start $FilePath"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($ProcessTimeoutSeconds * 1000)) {
            $process.Kill($true)
            $process.WaitForExit()
            throw "Process timed out after $ProcessTimeoutSeconds seconds: $FilePath $($ArgumentList -join ' ')"
        }
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        return [pscustomobject]@{
            exitCode = $process.ExitCode
            stdout = $stdout
            stderr = $stderr
            durationMs = $stopwatch.ElapsedMilliseconds
        }
    }
    finally {
        $stopwatch.Stop()
        $process.Dispose()
    }
}

function Invoke-RequiredProcess {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = $RepoRoot,
        [int]$ProcessTimeoutSeconds = $TimeoutSeconds,
        [string]$Description = $FilePath
    )

    $result = Invoke-CapturedProcess -FilePath $FilePath -ArgumentList $ArgumentList -WorkingDirectory $WorkingDirectory -ProcessTimeoutSeconds $ProcessTimeoutSeconds
    if ($result.exitCode -ne 0) {
        $output = ($result.stdout + [Environment]::NewLine + $result.stderr).Trim()
        throw "$Description failed with exit code $($result.exitCode): $output"
    }
    return $result
}

function Write-Utf8File {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Content
    )

    [IO.File]::WriteAllText($Path, $Content.Replace("`r`n", "`n"), [Text.UTF8Encoding]::new($false))
}

function Initialize-D20Fixture {
    param([Parameter(Mandatory)][string]$Directory)

    [IO.Directory]::CreateDirectory($Directory) | Out-Null

    $files = [ordered]@{
        'D20Fixture.csproj' = @'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
    <LangVersion>12.0</LangVersion>
    <Nullable>enable</Nullable>
    <ImplicitUsings>disable</ImplicitUsings>
    <Deterministic>true</Deterministic>
  </PropertyGroup>
</Project>
'@
        'One.Router.cs' = @'
namespace One
{
    public static class Counters
    {
        public static int IntRoute;
        public static int StringRoute;
        public static int IntTarget;
        public static int StringTarget;
        public static int TwoTarget;
    }

    public partial class Router
    {
        public void Route(int value) { Counters.IntRoute++; IntTarget(); }
        public void Route(string value) { Counters.StringRoute++; StringTarget(); }
        public void CallInt() => Route(1);
        public void CallString() => Route("x");
        public void CallUnknown(dynamic value) => Route(value);
        public void CallSameLine() { Route(2); Route("y"); }
        private void IntTarget() { Counters.IntTarget++; }
        private void StringTarget() { Counters.StringTarget++; }
    }

    public sealed class NamespaceCaller
    {
        public void CallTwo() => new Two.Router().Route(7);
    }
}
'@
        'One.Router.Partial.cs' = @'
namespace One
{
    public partial class Router
    {
        public void PartialControl() => PartialTarget();
        private void PartialTarget() { }
    }
}
'@
        'Two.Router.cs' = @'
namespace Two
{
    public sealed class Router
    {
        public void Route(int value) => TwoTarget();
        private void TwoTarget() { One.Counters.TwoTarget++; }
    }
}
'@
        'Program.cs' = @'
using System;
using System.Text.Json;

var router = new One.Router();
router.CallInt();
router.CallString();
Console.WriteLine(JsonSerializer.Serialize(new
{
    intRoute = One.Counters.IntRoute,
    stringRoute = One.Counters.StringRoute,
    intTarget = One.Counters.IntTarget,
    stringTarget = One.Counters.StringTarget,
    twoTarget = One.Counters.TwoTarget
}));
'@
    }

    foreach ($entry in $files.GetEnumerator()) {
        Write-Utf8File -Path (Join-Path $Directory $entry.Key) -Content $entry.Value
    }

    $manifestLines = [System.Collections.Generic.List[string]]::new()
    $manifest = [System.Collections.Generic.List[object]]::new()
    foreach ($name in ($files.Keys | Sort-Object)) {
        $hash = Get-Sha256 -Path (Join-Path $Directory $name)
        $manifestLines.Add("$name`t$hash")
        $manifest.Add([ordered]@{ path = $name; sha256 = $hash })
    }
    $manifestText = ($manifestLines -join "`n") + "`n"
    $manifestPath = Join-Path $Directory 'fixture-manifest.txt'
    Write-Utf8File -Path $manifestPath -Content $manifestText

    return [pscustomobject]@{
        entries = @($manifest)
        sha256 = Get-Sha256 -Path $manifestPath
    }
}

function Open-McpSession {
    param(
        [Parameter(Mandatory)]
        [string]$BinaryPath,
        [Parameter(Mandatory)]
        [string]$Directory,
        [switch]$Watch
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $BinaryPath
    $startInfo.WorkingDirectory = $Directory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $serverArguments = [System.Collections.Generic.List[string]]::new()
    foreach ($argument in @('serve', '--dir', $Directory, '--ext', 'cs', '--definitions', '--log-level', 'warn')) {
        $serverArguments.Add($argument)
    }
    if ($Watch) {
        foreach ($argument in @('--watch', '--debounce-ms', '50')) {
            $serverArguments.Add($argument)
        }
    }
    foreach ($argument in $serverArguments) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw 'Failed to start the Xray MCP server'
    }
    $stderrTask = $process.StandardError.ReadToEndAsync()
    return [pscustomobject]@{
        Process = $process
        StderrTask = $stderrTask
        NextId = 1
        Responses = [System.Collections.Generic.List[string]]::new()
    }
}

function Send-McpNotification {
    param(
        [Parameter(Mandatory)]
        [object]$Session,
        [Parameter(Mandatory)]
        [string]$Method,
        [object]$Params = @{}
    )

    $payload = [ordered]@{
        jsonrpc = '2.0'
        method = $Method
        params = $Params
    } | ConvertTo-Json -Compress -Depth 50
    $Session.Process.StandardInput.WriteLine($payload)
    $Session.Process.StandardInput.Flush()
}

function Send-McpRequest {
    param(
        [Parameter(Mandatory)]
        [object]$Session,
        [Parameter(Mandatory)]
        [string]$Method,
        [object]$Params = @{}
    )

    $id = $Session.NextId
    $Session.NextId++
    $payload = [ordered]@{
        jsonrpc = '2.0'
        id = $id
        method = $Method
        params = $Params
    } | ConvertTo-Json -Compress -Depth 50
    $Session.Process.StandardInput.WriteLine($payload)
    $Session.Process.StandardInput.Flush()

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remaining = [int][Math]::Max(1, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        $readTask = $Session.Process.StandardOutput.ReadLineAsync()
        if (-not $readTask.Wait($remaining)) {
            throw "Timed out waiting for MCP response id $id ($Method)"
        }
        $line = $readTask.GetAwaiter().GetResult()
        if ($null -eq $line) {
            $stderr = if ($Session.StderrTask.IsCompleted) { $Session.StderrTask.GetAwaiter().GetResult() } else { '' }
            throw "MCP server closed stdout while waiting for id $id ($Method): $stderr"
        }
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        $Session.Responses.Add($line)
        try {
            $response = $line | ConvertFrom-Json -Depth 100
        }
        catch {
            continue
        }
        if ([string]$response.id -ne [string]$id) {
            continue
        }
        if ($null -ne $response.PSObject.Properties['error']) {
            throw "MCP $Method returned error: $($response.error | ConvertTo-Json -Compress -Depth 20)"
        }
        return $response
    }
    throw "Timed out waiting for MCP response id $id ($Method)"
}

function ConvertFrom-XrayToolText {
    param([Parameter(Mandatory)][string]$Text)

    try {
        return $Text | ConvertFrom-Json -Depth 100
    }
    catch {
        $jsonStart = $Text.IndexOf('{')
        if ($jsonStart -lt 0) {
            throw "Xray tool text contains no JSON object: $Text"
        }
        return $Text.Substring($jsonStart) | ConvertFrom-Json -Depth 100
    }
}

function Invoke-McpTool {
    param(
        [Parameter(Mandatory)]
        [object]$Session,
        [Parameter(Mandatory)]
        [string]$ToolName,
        [Parameter(Mandatory)]
        [hashtable]$Arguments,
        [Parameter(Mandatory)]
        [string]$MeasurementName
    )

    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $response = Send-McpRequest -Session $Session -Method 'tools/call' -Params @{
            name = $ToolName
            arguments = $Arguments
        }
    }
    finally {
        $stopwatch.Stop()
        $script:QueryTimings[$MeasurementName] = $stopwatch.ElapsedMilliseconds
    }

    $textItem = @($response.result.content) | Where-Object { $_.type -eq 'text' } | Select-Object -First 1
    if ($null -ne $response.result.PSObject.Properties['isError'] -and $response.result.isError -eq $true) {
        $errorText = if ($null -ne $textItem) { $textItem.text } else { 'no text content' }
        throw "$ToolName returned isError=true: $errorText"
    }
    if ($null -eq $textItem) {
        throw "$ToolName returned no text content"
    }
    return ConvertFrom-XrayToolText -Text $textItem.text
}

function Close-McpSession {
    param([object]$Session)

    if ($null -eq $Session) {
        return [pscustomobject]@{ passed = $true; details = 'notStarted'; stderr = '' }
    }
    $process = $Session.Process
    try {
        if (-not $process.HasExited) {
            $process.StandardInput.Close()
            if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
                $process.Kill($true)
                $process.WaitForExit()
                $stderr = $Session.StderrTask.GetAwaiter().GetResult()
                return [pscustomobject]@{ passed = $false; details = 'forcedTerminationAfterTimeout'; stderr = $stderr }
            }
        }
        $stderr = $Session.StderrTask.GetAwaiter().GetResult()
        return [pscustomobject]@{
            passed = ($process.ExitCode -eq 0)
            details = if ($process.ExitCode -eq 0) { 'stdinClosedAndExited' } else { "exitCode=$($process.ExitCode); stderr=$stderr" }
            stderr = $stderr
        }
    }
    catch {
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        return [pscustomobject]@{ passed = $false; details = $_.Exception.Message; stderr = '' }
    }
    finally {
        $process.Dispose()
    }
}

function Get-DefinitionStartLine {
    param([Parameter(Mandatory)][object]$Definition)

    if ($null -ne $Definition.PSObject.Properties['line']) {
        return [int]$Definition.line
    }
    if ($null -ne $Definition.PSObject.Properties['bodyStartLine']) {
        return [int]$Definition.bodyStartLine
    }
    if ($null -ne $Definition.PSObject.Properties['lines'] -and [string]$Definition.lines -match '^(\d+)') {
        return [int]$Matches[1]
    }
    throw "Definition has no source line metadata: $($Definition | ConvertTo-Json -Compress -Depth 20)"
}

function Get-CallNode {
    param(
        [Parameter(Mandatory)]
        [object]$Output,
        [Parameter(Mandatory)]
        [string]$MethodName
    )

    $matchingNodes = [System.Collections.Generic.List[object]]::new()
    foreach ($node in @($Output.callTree)) {
        if ($node.method -eq $MethodName) {
            $matchingNodes.Add($node)
        }
    }
    return @($matchingNodes)
}

function Wait-D20Observation {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][scriptblock]$Probe
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $attempts = 0
    $lastError = $null
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    while ([DateTime]::UtcNow -lt $deadline) {
        $attempts++
        try {
            $value = & $Probe
            if ($null -ne $value) {
                $stopwatch.Stop()
                return [pscustomobject]@{
                    value = $value
                    attempts = $attempts
                    durationMs = $stopwatch.ElapsedMilliseconds
                }
            }
        }
        catch {
            $lastError = $_.Exception.Message
        }
        [Threading.Tasks.Task]::Delay(25).GetAwaiter().GetResult()
    }
    $stopwatch.Stop()
    throw "Timed out waiting for observable lifecycle outcome '$Name' after $attempts attempts. Last error: $lastError"
}

function Wait-McpIndexReady {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][ValidateSet('content', 'definition')][string]$IndexType,
        [Parameter(Mandatory)][string]$MeasurementName
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $attempts = 0
    $lastState = "no '$IndexType' index entry"
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    while ([DateTime]::UtcNow -lt $deadline) {
        $attempts++
        try {
            $info = Invoke-McpTool -Session $Session -ToolName 'xray_info' -MeasurementName $MeasurementName -Arguments @{}
        }
        catch {
            $stopwatch.Stop()
            $script:QueryTimings[$MeasurementName] = $stopwatch.ElapsedMilliseconds
            throw "Failed while waiting for '$IndexType' index readiness after $attempts attempts: $($_.Exception.Message)"
        }

        $matchingIndexes = @($info.indexes | Where-Object { $_.type -eq $IndexType })
        if ($matchingIndexes.Count -eq 1) {
            $status = $matchingIndexes[0].PSObject.Properties['status']
            if ($null -eq $status) {
                $stopwatch.Stop()
                $script:QueryTimings[$MeasurementName] = $stopwatch.ElapsedMilliseconds
                return [pscustomobject]@{
                    value = $matchingIndexes[0]
                    attempts = $attempts
                    durationMs = $stopwatch.ElapsedMilliseconds
                }
            }
            $lastState = "status=$($status.Value)"
        }
        else {
            $lastState = "expected one '$IndexType' index entry, found $($matchingIndexes.Count)"
        }
        [Threading.Tasks.Task]::Delay(25).GetAwaiter().GetResult()
    }

    $stopwatch.Stop()
    $script:QueryTimings[$MeasurementName] = $stopwatch.ElapsedMilliseconds
    throw "Timed out waiting for '$IndexType' index readiness after $attempts attempts. Last index state: $lastState"
}

function Invoke-IncrementalLifecycleSmoke {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$Directory
    )

    [IO.Directory]::CreateDirectory($Directory) | Out-Null
    $oneOverloadSource = @'
namespace Lifecycle
{
    public class Router
    {
        public void Route(int value) => IntTarget();
        public void CallUnknown(dynamic value) => Route(value);
        public void UseExtension(Router router) => router.ExtensionPulse();
        private void IntTarget() { }
    }
}
'@
    $twoOverloadSource = @'
namespace Lifecycle
{
    public class Router
    {
        public void Route(int value) => IntTarget();
        public void Route(string value) => StringTarget();
        public void CallUnknown(dynamic value) => Route(value);
        public void UseExtension(Router router) => router.ExtensionPulse();
        private void IntTarget() { }
        private void StringTarget() { }
    }
}
'@
    $renamedSource = @'
namespace LifecycleRenamed
{
    public class RouterRenamed
    {
        public void Route(int value) => IntTarget();
        private void IntTarget() { }
    }
}
'@
    $extensionSource = @'
namespace Lifecycle
{
    public static class RouterExtensions
    {
        public static void ExtensionPulse(this Router router) { }
    }
}
'@
    $replacementExtensionSource = @'
namespace Lifecycle
{
    public static class RouterExtensions
    {
        public static void Replacement(this Router router) { }
    }
}
'@

    $routerPath = Join-Path $Directory 'Router.cs'
    $extensionPath = Join-Path $Directory 'Extensions.cs'
    Write-Utf8File -Path $routerPath -Content $oneOverloadSource
    Write-Utf8File -Path $extensionPath -Content $extensionSource

    [void](Invoke-CapturedProcess -FilePath $BinaryPath -ArgumentList @('cleanup', '--dir', $Directory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds)
    [void](Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @('content-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'lifecycle content-index')
    [void](Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @('def-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'lifecycle def-index')

    $session = $null
    try {
        $session = Open-McpSession -BinaryPath $BinaryPath -Directory $Directory -Watch
        $initialize = Send-McpRequest -Session $session -Method 'initialize' -Params @{
            protocolVersion = '2025-03-26'
            capabilities = @{}
            clientInfo = @{ name = 'xray-d20-lifecycle-smoke'; version = '1.0' }
        }
        Assert-D20 -Condition ($initialize.result.protocolVersion -eq '2025-03-26') -Message 'Lifecycle MCP initialize failed'
        Send-McpNotification -Session $session -Method 'notifications/initialized'

        $initial = Wait-D20Observation -Name 'initial single overload' -Probe {
            $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleInitialDefinitionsMs' -Arguments @{
                name = @('Route')
                exactNameOnly = $true
            }
            $routes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' })
            if ($routes.Count -eq 1 -and $routes[0].signature -match '\bint\s+value\b') { return $routes[0] }
            return $null
        }
        $stableIntSymbolId = $initial.value.symbolId
        Assert-D20 -Condition ($stableIntSymbolId -match '^cs:v1:[0-9a-f]{64}$') -Message 'Initial lifecycle SymbolId is invalid'

        $initialCall = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'lifecycleInitialCallMs' -Arguments @{
            method = @('CallUnknown')
            class = 'Router'
            direction = 'down'
            depth = 1
        }
        $initialRouteNodes = @(Get-CallNode -Output $initialCall -MethodName 'Route')
        Assert-D20 -Condition ($initialRouteNodes.Count -eq 1 -and (Get-OptionalProperty -Object $initialRouteNodes[0] -Name 'nodeKind') -eq 'callee') -Message 'Single-overload lifecycle state was not exact'

        $initialExtension = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleInitialExtensionMs' -Arguments @{
            name = @('ExtensionPulse')
            exactNameOnly = $true
        }
        $initialExtensionDefinitions = @(Get-OptionalProperty -Object $initialExtension -Name 'definitions')
        Assert-D20 -Condition ($initialExtensionDefinitions.Count -eq 1 -and $initialExtensionDefinitions[0].signature -match '\bthis\s+Router\b') -Message 'Initial extension declaration was not indexed with its this receiver'
        $initialExtensionSymbolId = $initialExtensionDefinitions[0].symbolId

        Write-Utf8File -Path $routerPath -Content $twoOverloadSource
        $added = Wait-D20Observation -Name 'second overload added' -Probe {
            $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleAddedDefinitionsMs' -Arguments @{
                name = @('Route')
                exactNameOnly = $true
            }
            $routes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' })
            if ($routes.Count -ne 2) { return $null }
            $intRoute = @($routes | Where-Object { $_.signature -match '\bint\s+value\b' })
            if ($intRoute.Count -eq 1 -and $intRoute[0].symbolId -eq $stableIntSymbolId) { return $routes }
            return $null
        }
        $ambiguous = Wait-D20Observation -Name 'unknown call becomes ambiguous' -Probe {
            $output = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'lifecycleAddedAmbiguityMs' -Arguments @{
                method = @('CallUnknown')
                class = 'Router'
                direction = 'down'
                depth = 1
            }
            $ambiguousNodes = @($output.callTree | Where-Object { (Get-OptionalProperty -Object $_ -Name 'nodeKind') -eq 'ambiguousCall' -and $_.method -eq 'Route' })
            $exactNodes = @($output.callTree | Where-Object { (Get-OptionalProperty -Object $_ -Name 'nodeKind') -eq 'callee' -and $_.method -eq 'Route' })
            if ($ambiguousNodes.Count -eq 1 -and $exactNodes.Count -eq 0) { return $output }
            return $null
        }

        Write-Utf8File -Path $routerPath -Content $oneOverloadSource
        $removed = Wait-D20Observation -Name 'second overload removed' -Probe {
            $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleRemovedDefinitionsMs' -Arguments @{
                name = @('Route')
                exactNameOnly = $true
            }
            $routes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' })
            if ($routes.Count -eq 1 -and $routes[0].symbolId -eq $stableIntSymbolId) { return $routes[0] }
            return $null
        }
        $exactAgain = Wait-D20Observation -Name 'unknown call becomes exact again' -Probe {
            $output = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'lifecycleRemovedExactMs' -Arguments @{
                method = @('CallUnknown')
                class = 'Router'
                direction = 'down'
                depth = 1
            }
            $routes = @($output.callTree | Where-Object { (Get-OptionalProperty -Object $_ -Name 'nodeKind') -eq 'callee' -and $_.method -eq 'Route' })
            if ($routes.Count -eq 1) { return $output }
            return $null
        }

        Write-Utf8File -Path $routerPath -Content $renamedSource
        $renamed = Wait-D20Observation -Name 'namespace and type rename' -Probe {
            $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleRenamedDefinitionsMs' -Arguments @{
                name = @('Route')
                exactNameOnly = $true
            }
            $oldRoutes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' })
            $newRoutes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'LifecycleRenamed.RouterRenamed' })
            if ($oldRoutes.Count -eq 0 -and $newRoutes.Count -eq 1) { return $newRoutes[0] }
            return $null
        }
        Assert-D20 -Condition ($renamed.value.symbolId -ne $stableIntSymbolId) -Message 'Renamed namespace/type retained the old identity'
        $oldIdentityRejected = $false
        try {
            $oldIdentityOutput = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'lifecycleOldIdentityMs' -Arguments @{
                targets = @(@{ symbolId = $stableIntSymbolId })
                direction = 'down'
                depth = 1
            }
            $oldIdentityRejected = $null -eq (Get-OptionalProperty -Object $oldIdentityOutput -Name 'rootResolution') -or $oldIdentityOutput.rootResolution.status -ne 'exact'
        }
        catch {
            $oldIdentityRejected = $true
        }
        Assert-D20 -Condition $oldIdentityRejected -Message 'Old identity remained queryable after namespace/type rename'

        Write-Utf8File -Path $routerPath -Content $oneOverloadSource
        $restored = Wait-D20Observation -Name 'original identity restored' -Probe {
            $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleRestoredDefinitionsMs' -Arguments @{
                name = @('Route')
                exactNameOnly = $true
            }
            $routes = @($definitions.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' })
            if ($routes.Count -eq 1 -and $routes[0].symbolId -eq $stableIntSymbolId) { return $routes[0] }
            return $null
        }

        $reindex = Invoke-McpTool -Session $session -ToolName 'xray_reindex_definitions' -MeasurementName 'lifecycleReindexDefinitionsMs' -Arguments @{}
        $postReindex = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecyclePostReindexDefinitionsMs' -Arguments @{
            name = @('Route')
            exactNameOnly = $true
        }
        $postReindexRoutes = @($postReindex.definitions | Where-Object { $_.qualifiedType -eq 'Lifecycle.Router' -and $_.signature -match '\bint\s+value\b' })
        Assert-D20 -Condition ($postReindexRoutes.Count -eq 1 -and $postReindexRoutes[0].symbolId -eq $stableIntSymbolId) -Message 'Stable SymbolId changed after xray_reindex_definitions'

        Write-Utf8File -Path $extensionPath -Content $replacementExtensionSource
        $extensionReparsed = Wait-D20Observation -Name 'extension declaration replaced' -Probe {
            $oldDefinitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleOldExtensionDefinitionsMs' -Arguments @{
                name = @('ExtensionPulse')
                exactNameOnly = $true
            }
            $newDefinitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleNewExtensionDefinitionsMs' -Arguments @{
                name = @('Replacement')
                exactNameOnly = $true
            }
            $oldItems = @(Get-OptionalProperty -Object $oldDefinitions -Name 'definitions')
            $newItems = @(Get-OptionalProperty -Object $newDefinitions -Name 'definitions')
            if ($oldItems.Count -eq 0 -and $newItems.Count -eq 1 -and $newItems[0].signature -match '\bthis\s+Router\b') { return $newItems[0] }
            return $null
        }
        $oldExtensionRejected = $false
        try {
            $oldExtensionOutput = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'lifecycleOldExtensionIdentityMs' -Arguments @{
                targets = @(@{ symbolId = $initialExtensionSymbolId })
                direction = 'down'
                depth = 1
            }
            $oldExtensionRejected = $null -eq (Get-OptionalProperty -Object $oldExtensionOutput -Name 'rootResolution') -or $oldExtensionOutput.rootResolution.status -ne 'exact'
        }
        catch {
            $oldExtensionRejected = $true
        }
        Assert-D20 -Condition $oldExtensionRejected -Message 'Reparse left the removed extension SymbolId queryable'

        [void](Invoke-McpTool -Session $session -ToolName 'xray_reindex_definitions' -MeasurementName 'lifecycleExtensionReindexMs' -Arguments @{})
        $oldExtensionAfterReindex = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'lifecycleOldExtensionAfterReindexMs' -Arguments @{
            name = @('ExtensionPulse')
            exactNameOnly = $true
        }
        Assert-D20 -Condition (@(Get-OptionalProperty -Object $oldExtensionAfterReindex -Name 'definitions').Count -eq 0) -Message 'Definition reindex resurrected a removed extension declaration'

        $shutdown = Close-McpSession -Session $session
        $session = $null
        Assert-D20 -Condition ($shutdown.passed) -Message "Lifecycle MCP did not shut down cleanly: $($shutdown.details)"
        return [ordered]@{
            status = 'passed'
            passed = $true
            stableSymbolId = $stableIntSymbolId
            renamedSymbolId = $renamed.value.symbolId
            addPollAttempts = $added.attempts
            ambiguityPollAttempts = $ambiguous.attempts
            removePollAttempts = $removed.attempts
            exactAgainPollAttempts = $exactAgain.attempts
            restorePollAttempts = $restored.attempts
            renamePollAttempts = $renamed.attempts
            extensionReparsePollAttempts = $extensionReparsed.attempts
            reindexStatus = Get-OptionalProperty -Object $reindex -Name 'status'
            staleExtensionContributionRemoved = $true
            shutdown = $shutdown.details
        }
    }
    finally {
        if ($null -ne $session) {
            [void](Close-McpSession -Session $session)
        }
        [void](Invoke-CapturedProcess -FilePath $BinaryPath -ArgumentList @('cleanup', '--dir', $Directory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds)
    }
}


function Invoke-MigrationSmoke {
    param(
        [Parameter(Mandatory)][string]$BaselineBinaryPath,
        [Parameter(Mandatory)][string]$CandidateBinaryPath,
        [Parameter(Mandatory)][string]$Directory,
        [Parameter(Mandatory)][string]$ExpectedIntSymbolId
    )

    $indexDirectory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'xray'
    [IO.Directory]::CreateDirectory($indexDirectory) | Out-Null
    $firstSession = $null
    $secondSession = $null
    $firstShutdown = $null
    $secondShutdown = $null
    try {
        [void](Invoke-RequiredProcess -FilePath $BaselineBinaryPath -ArgumentList @('cleanup', '--dir', $Directory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds -Description 'pre-migration baseline cleanup')
        $beforePaths = @{}
        foreach ($file in @(Get-ChildItem -Path $indexDirectory -Filter '*.code-structure' -File -ErrorAction SilentlyContinue)) {
            $beforePaths[$file.FullName] = $true
        }

        [void](Invoke-RequiredProcess -FilePath $BaselineBinaryPath -ArgumentList @('content-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'v6 content-index build')
        $v6Build = Invoke-RequiredProcess -FilePath $BaselineBinaryPath -ArgumentList @('def-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'v6 definition-index build'
        $newDefinitionIndexes = @(Get-ChildItem -Path $indexDirectory -Filter '*.code-structure' -File | Where-Object { -not $beforePaths.ContainsKey($_.FullName) })
        Assert-D20 -Condition ($newDefinitionIndexes.Count -eq 1) -Message "Expected one genuine v6 definition index, found $($newDefinitionIndexes.Count)"
        $indexPath = $newDefinitionIndexes[0].FullName
        $v6Bytes = $newDefinitionIndexes[0].Length
        $v6Sha256 = Get-Sha256 -Path $indexPath

        $firstSession = Open-McpSession -BinaryPath $CandidateBinaryPath -Directory $Directory
        $initialize = Send-McpRequest -Session $firstSession -Method 'initialize' -Params @{
            protocolVersion = '2025-03-26'
            capabilities = @{}
            clientInfo = @{ name = 'xray-d20-migration-smoke'; version = '1.0' }
        }
        Assert-D20 -Condition ($initialize.result.protocolVersion -eq '2025-03-26') -Message 'Migration MCP initialize failed'
        Send-McpNotification -Session $firstSession -Method 'notifications/initialized'
        $firstDefinitionReady = Wait-McpIndexReady -Session $firstSession -IndexType 'definition' -MeasurementName 'migrationDefinitionReadyMs'

        $definitions = Invoke-McpTool -Session $firstSession -ToolName 'xray_definitions' -MeasurementName 'migrationDefinitionsMs' -Arguments @{
            name = @('Route')
            exactNameOnly = $true
        }
        $rebuiltIntRoutes = @($definitions.definitions | Where-Object {
            $_.qualifiedType -eq 'One.Router' -and $_.signature -match '\bint\s+value\b'
        })
        Assert-D20 -Condition ($rebuiltIntRoutes.Count -eq 1) -Message 'v6 to v7 rebuild did not restore the int overload definition'
        Assert-D20 -Condition ($rebuiltIntRoutes[0].symbolId -eq $ExpectedIntSymbolId) -Message 'Stable SymbolId changed across v6 to v7 rebuild'

        $exactDown = Invoke-McpTool -Session $firstSession -ToolName 'xray_callers' -MeasurementName 'migrationExactDownMs' -Arguments @{
            targets = @(@{ symbolId = $ExpectedIntSymbolId })
            direction = 'down'
            depth = 1
        }
        $exactMethods = @($exactDown.callTree | ForEach-Object { $_.method })
        Assert-D20 -Condition ($exactDown.rootResolution.status -eq 'exact' -and $exactMethods.Count -eq 1 -and $exactMethods[0] -eq 'IntTarget') -Message 'Exact D20 query failed after v6 to v7 rebuild'

        $firstShutdown = Close-McpSession -Session $firstSession
        $firstSession = $null
        Assert-D20 -Condition ($firstShutdown.passed) -Message "Migration server did not shut down cleanly: $($firstShutdown.details)"
        Assert-D20 -Condition ($firstShutdown.stderr -match 'Format version mismatch \(found 6, expected 7\)') -Message "Current release did not report the v6 pre-decode mismatch: $($firstShutdown.stderr)"
        Assert-D20 -Condition (Test-Path $indexPath -PathType Leaf) -Message 'Rebuilt v7 definition index was not persisted'
        $v7Bytes = (Get-Item $indexPath).Length
        $v7Sha256 = Get-Sha256 -Path $indexPath
        Assert-D20 -Condition ($v7Sha256 -ne $v6Sha256) -Message 'Persisted definition index did not change after the v6 rejection'

        $secondSession = Open-McpSession -BinaryPath $CandidateBinaryPath -Directory $Directory
        $initializeSecond = Send-McpRequest -Session $secondSession -Method 'initialize' -Params @{
            protocolVersion = '2025-03-26'
            capabilities = @{}
            clientInfo = @{ name = 'xray-d20-migration-reload'; version = '1.0' }
        }
        Assert-D20 -Condition ($initializeSecond.result.protocolVersion -eq '2025-03-26') -Message 'Post-migration MCP initialize failed'
        Send-McpNotification -Session $secondSession -Method 'notifications/initialized'
        $reloadDefinitionReady = Wait-McpIndexReady -Session $secondSession -IndexType 'definition' -MeasurementName 'migrationReloadDefinitionReadyMs'
        $reloadExact = Invoke-McpTool -Session $secondSession -ToolName 'xray_callers' -MeasurementName 'migrationReloadExactMs' -Arguments @{
            targets = @(@{ symbolId = $ExpectedIntSymbolId })
            direction = 'down'
            depth = 1
        }
        $reloadMethods = @($reloadExact.callTree | ForEach-Object { $_.method })
        Assert-D20 -Condition ($reloadExact.rootResolution.status -eq 'exact' -and $reloadMethods.Count -eq 1 -and $reloadMethods[0] -eq 'IntTarget') -Message 'Persisted v7 reload failed the exact query'
        $secondShutdown = Close-McpSession -Session $secondSession
        $secondSession = $null
        Assert-D20 -Condition ($secondShutdown.passed) -Message "Post-migration server did not shut down cleanly: $($secondShutdown.details)"
        Assert-D20 -Condition ($secondShutdown.stderr -notmatch 'Format version mismatch') -Message 'Persisted v7 index was rejected on the second load'

        return [ordered]@{
            status = 'passed'
            passed = $true
            source = 'genuineBaselineV6'
            preDecodeGuardObserved = $true
            v6BuildDurationMs = $v6Build.durationMs
            v6Bytes = $v6Bytes
            v6Sha256 = $v6Sha256
            v7Bytes = $v7Bytes
            v7Sha256 = $v7Sha256
            stableSymbolId = $ExpectedIntSymbolId
            firstDefinitionReadyAttempts = $firstDefinitionReady.attempts
            firstDefinitionReadyMs = $firstDefinitionReady.durationMs
            reloadDefinitionReadyAttempts = $reloadDefinitionReady.attempts
            reloadDefinitionReadyMs = $reloadDefinitionReady.durationMs
            reloadPassed = $true
        }
    }
    finally {
        if ($null -ne $firstSession) {
            [void](Close-McpSession -Session $firstSession)
        }
        if ($null -ne $secondSession) {
            [void](Close-McpSession -Session $secondSession)
        }
    }
}


function Get-OptionalProperty {
    param(
        [Parameter(Mandatory)][object]$Object,
        [Parameter(Mandatory)][string]$Name
    )

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Invoke-BaselineSemanticControl {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$Directory,
        [Parameter(Mandatory)][int]$IntRouteLine,
        [Parameter(Mandatory)][int]$StringRouteLine,
        [Parameter(Mandatory)][int]$TwoRouteLine
    )

    $failedAssertions = [System.Collections.Generic.List[string]]::new()
    $observations = [ordered]@{}
    $session = $null
    $shutdown = $null
    try {
        [void](Invoke-CapturedProcess -FilePath $BinaryPath -ArgumentList @('cleanup', '--dir', $Directory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds)
        $contentIndex = Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @('content-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'baseline content-index'
        $definitionIndex = Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @('def-index', '--dir', $Directory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'baseline def-index'
        $observations.contentIndexBuildMs = $contentIndex.durationMs
        $observations.definitionIndexBuildMs = $definitionIndex.durationMs

        $session = Open-McpSession -BinaryPath $BinaryPath -Directory $Directory
        $initialize = Send-McpRequest -Session $session -Method 'initialize' -Params @{
            protocolVersion = '2025-03-26'
            capabilities = @{}
            clientInfo = @{ name = 'xray-d20-baseline-control'; version = '1.0' }
        }
        Assert-D20 -Condition ($initialize.result.protocolVersion -eq '2025-03-26') -Message 'Baseline MCP initialize returned an unexpected protocol version'
        Send-McpNotification -Session $session -Method 'notifications/initialized'

        $definitions = Invoke-McpTool -Session $session -ToolName 'xray_definitions' -MeasurementName 'baselineDefinitionsRouteMs' -Arguments @{
            name = @('Route')
            exactNameOnly = $true
        }
        $routeDefinitions = @($definitions.definitions) | Where-Object { $_.name -eq 'Route' }
        $validIds = @($routeDefinitions | Where-Object {
            $symbolId = Get-OptionalProperty -Object $_ -Name 'symbolId'
            $symbolId -is [string] -and $symbolId -match '^cs:v1:[0-9a-f]{64}$'
        })
        $qualifiedTypes = @($routeDefinitions | ForEach-Object { Get-OptionalProperty -Object $_ -Name 'qualifiedType' })
        $identityPassed = $validIds.Count -eq 3 -and @($validIds.symbolId | Sort-Object -Unique).Count -eq 3 -and @($qualifiedTypes | Where-Object { $_ -eq 'One.Router' }).Count -eq 2 -and @($qualifiedTypes | Where-Object { $_ -eq 'Two.Router' }).Count -eq 1
        if (-not $identityPassed) {
            $failedAssertions.Add('symbolIdentity')
        }

        $callInt = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'baselineCallIntDownMs' -Arguments @{
            method = @('CallInt')
            class = 'Router'
            direction = 'down'
            depth = 1
        }
        $callString = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'baselineCallStringDownMs' -Arguments @{
            method = @('CallString')
            class = 'Router'
            direction = 'down'
            depth = 1
        }
        $callIntRoutes = @(Get-CallNode -Output $callInt -MethodName 'Route')
        $callStringRoutes = @(Get-CallNode -Output $callString -MethodName 'Route')
        $overloadPassed = $callIntRoutes.Count -eq 1 -and $callIntRoutes[0].line -eq $IntRouteLine -and $callStringRoutes.Count -eq 1 -and $callStringRoutes[0].line -eq $StringRouteLine
        if (-not $overloadPassed) {
            $failedAssertions.Add('overloadDownRoots')
        }
        $observations.callIntRouteCount = $callIntRoutes.Count
        $observations.callStringRouteCount = $callStringRoutes.Count

        $namespaceCall = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'baselineNamespaceDownMs' -Arguments @{
            method = @('CallTwo')
            class = 'NamespaceCaller'
            direction = 'down'
            depth = 1
        }
        $namespaceRoutes = @(Get-CallNode -Output $namespaceCall -MethodName 'Route')
        $namespaceFile = if ($namespaceRoutes.Count -eq 1) { Get-OptionalProperty -Object $namespaceRoutes[0] -Name 'file' } else { $null }
        $namespacePassed = $namespaceRoutes.Count -eq 1 -and $namespaceRoutes[0].line -eq $TwoRouteLine -and $namespaceFile -like '*Two.Router.cs'
        if (-not $namespacePassed) {
            $failedAssertions.Add('namespaceIsolation')
        }
        $observations.namespaceRouteCount = $namespaceRoutes.Count

        $unknown = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'baselineUnknownAmbiguousMs' -Arguments @{
            method = @('CallUnknown')
            class = 'Router'
            direction = 'down'
            depth = 1
        }
        $ambiguousRoutes = @($unknown.callTree | Where-Object {
            (Get-OptionalProperty -Object $_ -Name 'nodeKind') -eq 'ambiguousCall' -and $_.method -eq 'Route'
        })
        $exactUnknownRoutes = @($unknown.callTree | Where-Object {
            (Get-OptionalProperty -Object $_ -Name 'nodeKind') -eq 'callee' -and $_.method -eq 'Route'
        })
        $safeForExact = if ($null -ne (Get-OptionalProperty -Object $unknown -Name 'resultStatus')) {
            Get-OptionalProperty -Object $unknown.resultStatus -Name 'safeForExactSemantics'
        } else {
            $null
        }
        $ambiguityPassed = $ambiguousRoutes.Count -eq 1 -and $exactUnknownRoutes.Count -eq 0 -and $safeForExact -eq $false
        if (-not $ambiguityPassed) {
            $failedAssertions.Add('unknownAmbiguity')
        }

        try {
            $exactDown = Invoke-McpTool -Session $session -ToolName 'xray_callers' -MeasurementName 'baselineExactIntDownMs' -Arguments @{
                targets = @(@{ symbolId = 'cs:v1:0000000000000000000000000000000000000000000000000000000000000000' })
                direction = 'down'
                depth = 1
            }
            $exactPassed = (Get-OptionalProperty -Object $exactDown -Name 'rootResolution') -and $exactDown.rootResolution.status -eq 'exact'
            if (-not $exactPassed) {
                $failedAssertions.Add('exactSymbolQuery')
            }
        }
        catch {
            $exactQueryError = $_.Exception.Message
            if ($exactQueryError -notlike 'xray_callers returned isError=true:*') {
                throw
            }
            $failedAssertions.Add('exactSymbolQuery')
            $observations.exactQueryError = $exactQueryError
        }
    }
    finally {
        $shutdown = Close-McpSession -Session $session
        [void](Invoke-CapturedProcess -FilePath $BinaryPath -ArgumentList @('cleanup', '--dir', $Directory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds)
    }

    Assert-D20 -Condition ($shutdown.passed) -Message "Baseline MCP did not shut down cleanly: $($shutdown.details)"
    $expectedFailures = @('symbolIdentity', 'overloadDownRoots', 'namespaceIsolation', 'unknownAmbiguity', 'exactSymbolQuery') | Sort-Object
    $actualFailures = @($failedAssertions | Sort-Object -Unique)
    Assert-D20 -Condition (($actualFailures -join ',') -eq ($expectedFailures -join ',')) -Message "Baseline failure fingerprint changed. Expected: $($expectedFailures -join ', '); actual: $($actualFailures -join ', ')"
    $graphFailures = @($actualFailures | Where-Object { $_ -in @('overloadDownRoots', 'namespaceIsolation', 'unknownAmbiguity') })

    return [ordered]@{
        status = 'expectedFailure'
        passed = $true
        failedAssertions = @($failedAssertions)
        graphFailures = $graphFailures
        observations = $observations
        shutdown = $shutdown.details
    }
}


try {
    if (-not [string]::IsNullOrWhiteSpace($Binary) -and $BuildRelease) {
        throw 'Use either -Binary or -BuildRelease, not both'
    }
    [IO.Directory]::CreateDirectory($ArtifactRoot) | Out-Null
    $script:FixtureDirectory = Join-Path $ArtifactRoot 'fixture'

    foreach ($requiredCommand in @('git', 'rustc', 'cargo', 'dotnet')) {
        if ($null -eq (Get-Command $requiredCommand -ErrorAction SilentlyContinue)) {
            throw "Required command '$requiredCommand' was not found. Install .NET SDK 8.0 or newer and Rust 1.91-compatible tooling before running this gate."
        }
    }

    $candidateCommitResult = Invoke-RequiredProcess -FilePath 'git' -ArgumentList @('rev-parse', 'HEAD') -Description 'git rev-parse HEAD'
    $script:CandidateCommit = $candidateCommitResult.stdout.Trim()
    $rustcInfo = Invoke-RequiredProcess -FilePath 'rustc' -ArgumentList @('--version', '--verbose') -Description 'rustc metadata'
    $cargoInfo = Invoke-RequiredProcess -FilePath 'cargo' -ArgumentList @('--version', '--verbose') -Description 'cargo metadata'
    $dotnetInfo = Invoke-RequiredProcess -FilePath 'dotnet' -ArgumentList @('--info') -Description 'dotnet --info'

    if ([string]::IsNullOrWhiteSpace($Binary)) {
        $build = Invoke-RequiredProcess -FilePath 'cargo' -ArgumentList @('build', '--release', '--locked') -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'candidate release build'
        $script:Timings.releaseBuildMs = $build.durationMs
        $script:CandidateBinary = Join-Path $RepoRoot 'target/release/xray.exe'
    }
    else {
        $script:CandidateBinary = (Resolve-Path $Binary).Path
        $script:Timings.releaseBuildMs = $null
    }
    Assert-D20 -Condition (Test-Path $script:CandidateBinary -PathType Leaf) -Message "Candidate binary not found: $($script:CandidateBinary)"
    $candidateBinarySha256 = Get-Sha256 -Path $script:CandidateBinary
    $script:CargoLockSha256 = Get-Sha256 -Path (Join-Path $RepoRoot 'Cargo.lock')

    $fixture = Initialize-D20Fixture -Directory $script:FixtureDirectory
    $script:FixtureManifestSha256 = $fixture.sha256
    $script:FixtureManifest = $fixture.entries

    $oracleRoot = Join-Path $ArtifactRoot 'oracle'
    $oracleObjectDirectory = Join-Path $oracleRoot 'obj'
    $oracleOutputDirectory = Join-Path $oracleRoot 'bin'
    $directorySeparator = [IO.Path]::DirectorySeparatorChar
    $oracleBuildArguments = @(
        'build',
        'D20Fixture.csproj',
        '--configuration',
        'Release',
        '--nologo',
        '--verbosity',
        'minimal',
        "-p:BaseIntermediateOutputPath=$oracleObjectDirectory$directorySeparator",
        "-p:OutputPath=$oracleOutputDirectory$directorySeparator",
        '-p:AppendTargetFrameworkToOutputPath=false',
        '-p:AppendRuntimeIdentifierToOutputPath=false'
    )
    $oracleBuild = Invoke-RequiredProcess -FilePath 'dotnet' -ArgumentList $oracleBuildArguments -WorkingDirectory $script:FixtureDirectory -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description '.NET fixture build'
    $fixtureSourceFiles = @(Get-ChildItem -Path $script:FixtureDirectory -Filter '*.cs' -File -Recurse | ForEach-Object {
        [IO.Path]::GetRelativePath($script:FixtureDirectory, $_.FullName).Replace('\', '/')
    } | Sort-Object)
    $manifestSourceFiles = @($script:FixtureManifest | Where-Object { $_.path -like '*.cs' } | ForEach-Object { $_.path } | Sort-Object)
    Assert-D20 -Condition (($fixtureSourceFiles -join ',') -eq ($manifestSourceFiles -join ',')) -Message "Generated C# files contaminated the indexed fixture. Expected: $($manifestSourceFiles -join ', '); actual: $($fixtureSourceFiles -join ', ')"
    $oracleAssembly = Join-Path $oracleOutputDirectory 'D20Fixture.dll'
    Assert-D20 -Condition (Test-Path $oracleAssembly -PathType Leaf) -Message "Oracle assembly not found: $oracleAssembly"
    $oracleRun = Invoke-RequiredProcess -FilePath 'dotnet' -ArgumentList @($oracleAssembly) -WorkingDirectory $script:FixtureDirectory -ProcessTimeoutSeconds $TimeoutSeconds -Description '.NET fixture runtime oracle'
    $runtimeLine = @($oracleRun.stdout -split "`r?`n") | Where-Object { $_ -match '^\s*\{' } | Select-Object -Last 1
    Assert-D20 -Condition (-not [string]::IsNullOrWhiteSpace($runtimeLine)) -Message "Runtime oracle emitted no JSON counters: $($oracleRun.stdout)"
    $runtimeCounters = $runtimeLine | ConvertFrom-Json
    Assert-D20 -Condition ($runtimeCounters.intRoute -eq 1) -Message 'Runtime oracle did not select only Route(int) for CallInt'
    Assert-D20 -Condition ($runtimeCounters.stringRoute -eq 1) -Message 'Runtime oracle did not select only Route(string) for CallString'
    Assert-D20 -Condition ($runtimeCounters.intTarget -eq 1 -and $runtimeCounters.stringTarget -eq 1) -Message 'Runtime oracle downstream counters are incorrect'
    Assert-D20 -Condition ($runtimeCounters.twoTarget -eq 0) -Message 'Runtime oracle crossed into Two.Router'
    $script:CompilerOracle = [ordered]@{
        status = 'passed'
        buildExitCode = $oracleBuild.exitCode
        runExitCode = $oracleRun.exitCode
        buildDurationMs = $oracleBuild.durationMs
        runDurationMs = $oracleRun.durationMs
        runtimeOutput = $runtimeLine
        dotnetInfo = $dotnetInfo.stdout.Trim()
    }
    Add-Check -Name 'compilerOracle' -Passed $true -Details 'CallInt and CallString reached only their compiler-selected overload targets' -DurationMs ($oracleBuild.durationMs + $oracleRun.durationMs)

    $contentIndex = Invoke-RequiredProcess -FilePath $script:CandidateBinary -ArgumentList @('content-index', '--dir', $script:FixtureDirectory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'candidate content-index'
    $definitionIndex = Invoke-RequiredProcess -FilePath $script:CandidateBinary -ArgumentList @('def-index', '--dir', $script:FixtureDirectory, '--ext', 'cs') -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'candidate def-index'
    $script:Timings.contentIndexBuildMs = $contentIndex.durationMs
    $script:Timings.definitionIndexBuildMs = $definitionIndex.durationMs

    $script:McpSession = Open-McpSession -BinaryPath $script:CandidateBinary -Directory $script:FixtureDirectory
    $initialize = Send-McpRequest -Session $script:McpSession -Method 'initialize' -Params @{
        protocolVersion = '2025-03-26'
        capabilities = @{}
        clientInfo = @{ name = 'xray-d20-release-gate'; version = '1.0' }
    }
    Assert-D20 -Condition ($initialize.result.protocolVersion -eq '2025-03-26') -Message 'MCP initialize returned an unexpected protocol version'
    Send-McpNotification -Session $script:McpSession -Method 'notifications/initialized'
    Add-Check -Name 'mcpInitialize' -Passed $true -Details 'initialize response matched request id and protocol version'

    $definitions = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_definitions' -MeasurementName 'definitionsRouteMs' -Arguments @{
        name = @('Route')
        exactNameOnly = $true
    }
    $routeDefinitions = @($definitions.definitions) | Where-Object { $_.name -eq 'Route' }
    $oneRoutes = @($routeDefinitions | Where-Object { $_.qualifiedType -eq 'One.Router' })
    $twoRoutes = @($routeDefinitions | Where-Object { $_.qualifiedType -eq 'Two.Router' })
    Assert-D20 -Condition ($oneRoutes.Count -eq 2) -Message "Expected two One.Router.Route overloads, found $($oneRoutes.Count)"
    Assert-D20 -Condition ($twoRoutes.Count -eq 1) -Message "Expected one Two.Router.Route overload, found $($twoRoutes.Count)"
    $intRoute = @($oneRoutes | Where-Object { $_.signature -match '\bint\s+value\b' })
    $stringRoute = @($oneRoutes | Where-Object { $_.signature -match '\bstring\s+value\b' })
    Assert-D20 -Condition ($intRoute.Count -eq 1 -and $stringRoute.Count -eq 1) -Message 'Could not identify One.Router overloads from structured definitions'
    $intRoute = $intRoute[0]
    $stringRoute = $stringRoute[0]
    $twoIntRoute = $twoRoutes[0]
    $intRouteLine = Get-DefinitionStartLine -Definition $intRoute
    $stringRouteLine = Get-DefinitionStartLine -Definition $stringRoute
    $twoIntRouteLine = Get-DefinitionStartLine -Definition $twoIntRoute
    foreach ($definition in @($intRoute, $stringRoute, $twoIntRoute)) {
        Assert-D20 -Condition ($definition.symbolId -match '^cs:v1:[0-9a-f]{64}$') -Message "Invalid public C# symbolId: $($definition.symbolId)"
    }
    Assert-D20 -Condition ($intRoute.symbolId -ne $stringRoute.symbolId) -Message 'One.Router overload symbolIds were merged'
    Assert-D20 -Condition ($intRoute.symbolId -ne $twoIntRoute.symbolId) -Message 'One.Router and Two.Router symbolIds collided'
    Add-Check -Name 'symbolIdentity' -Passed $true -Details 'Overloads and namespace-collision control expose distinct stable public IDs'

    $callInt = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'callIntDownMs' -Arguments @{
        method = @('CallInt')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    $callIntRoutes = @(Get-CallNode -Output $callInt -MethodName 'Route')
    Assert-D20 -Condition ($callIntRoutes.Count -eq 1 -and $callIntRoutes[0].line -eq $intRouteLine) -Message 'CallInt did not resolve exclusively to One.Router.Route(int)'

    $callString = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'callStringDownMs' -Arguments @{
        method = @('CallString')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    $callStringRoutes = @(Get-CallNode -Output $callString -MethodName 'Route')
    Assert-D20 -Condition ($callStringRoutes.Count -eq 1 -and $callStringRoutes[0].line -eq $stringRouteLine) -Message 'CallString did not resolve exclusively to One.Router.Route(string)'
    Add-Check -Name 'overloadDownRoots' -Passed $true -Details 'CallInt and CallString resolve to different overload definitions'


    $sameLine = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'sameLineDownMs' -Arguments @{
        method = @('CallSameLine')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    $sameLineRoutes = @(Get-CallNode -Output $sameLine -MethodName 'Route')
    $sameLineTargets = @($sameLineRoutes | ForEach-Object { [int]$_.line } | Sort-Object)
    $expectedSameLineTargets = @($intRouteLine, $stringRouteLine | Sort-Object)
    Assert-D20 -Condition ($sameLineRoutes.Count -eq 2 -and ($sameLineTargets -join ',') -eq ($expectedSameLineTargets -join ',')) -Message 'Two call sites on one source line were collapsed or resolved to the wrong overloads'
    Add-Check -Name 'sameLineCallSites' -Passed $true -Details 'Two same-line call sites remain distinct and select different overload definitions'

    $partial = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'partialControlMs' -Arguments @{
        method = @('PartialControl')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    $partialTargets = @(Get-CallNode -Output $partial -MethodName 'PartialTarget')
    Assert-D20 -Condition ($partialTargets.Count -eq 1) -Message 'Partial declaration/body control did not resolve its target'
    Add-Check -Name 'partialBodyControl' -Passed $true -Details 'Partial class body resolves PartialControl to PartialTarget'

    $exactDown = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'exactIntDownMs' -Arguments @{
        targets = @(@{ symbolId = $intRoute.symbolId })
        direction = 'down'
        depth = 1
    }
    $exactDownMethods = @($exactDown.callTree | ForEach-Object { $_.method })
    Assert-D20 -Condition ($exactDown.rootResolution.status -eq 'exact') -Message 'Exact symbol root was not resolved exactly'
    Assert-D20 -Condition ($exactDownMethods.Count -eq 1 -and $exactDownMethods[0] -eq 'IntTarget') -Message "Exact int symbol traversed the wrong body: $($exactDownMethods -join ', ')"

    $exactUp = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'exactIntUpMs' -Arguments @{
        targets = @(@{ symbolId = $intRoute.symbolId })
        direction = 'up'
        depth = 1
    }
    $exactUpMethods = @($exactUp.callTree | ForEach-Object { $_.method })
    Assert-D20 -Condition ($exactUpMethods -contains 'CallInt') -Message 'Exact int up query omitted CallInt'
    Assert-D20 -Condition ($exactUpMethods -notcontains 'CallString') -Message 'Exact int up query included CallString'
    Add-Check -Name 'exactSymbolQueries' -Passed $true -Details 'Exact down selected IntTarget and exact up excluded CallString'

    $unknown = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'unknownAmbiguousMs' -Arguments @{
        method = @('CallUnknown')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    $ambiguousRoutes = @($unknown.callTree | Where-Object { $_.nodeKind -eq 'ambiguousCall' -and $_.method -eq 'Route' })
    $exactUnknownRoutes = @($unknown.callTree | Where-Object { $_.nodeKind -eq 'callee' -and $_.method -eq 'Route' })
    Assert-D20 -Condition ($ambiguousRoutes.Count -eq 1) -Message 'Dynamic call did not report one ambiguousCall node'
    Assert-D20 -Condition ($exactUnknownRoutes.Count -eq 0) -Message 'Dynamic call traversed an overload as an exact callee'
    Assert-D20 -Condition (@($ambiguousRoutes[0].resolution.candidates).Count -ge 2 -and @($ambiguousRoutes[0].resolution.candidates).Count -le 10) -Message 'Ambiguous call candidates were missing or unbounded'
    Assert-D20 -Condition ($unknown.resultStatus.safeForExactSemantics -eq $false) -Message 'Ambiguous call was marked safe for exact semantics'
    Add-Check -Name 'unknownAmbiguity' -Passed $true -Details 'Dynamic call reports bounded candidates without an exact overload subtree'

    $legacyRoot = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'legacyRootReportMs' -Arguments @{
        method = @('Route')
        class = 'Router'
        direction = 'down'
        depth = 1
    }
    Assert-D20 -Condition ($legacyRoot.rootResolution.status -eq 'ambiguous') -Message 'Legacy root was not reported as ambiguous'
    Assert-D20 -Condition (@($legacyRoot.callTree).Count -eq 0) -Message 'Default report policy traversed an ambiguous legacy root'
    Add-Check -Name 'legacyRootReport' -Passed $true -Details 'Short-name root is ambiguous and not traversed by default'


    $legacyUnsafe = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'legacyRootUnsafeMs' -Arguments @{
        method = @('Route')
        class = 'Router'
        direction = 'down'
        depth = 1
        ambiguityPolicy = 'legacy'
    }
    $legacyUnsafeMethods = @($legacyUnsafe.callTree | ForEach-Object { $_.method })
    Assert-D20 -Condition ($legacyUnsafe.resultStatus.safeForExactSemantics -eq $false) -Message 'Explicit legacy fan-out was marked safe for exact semantics'
    Assert-D20 -Condition (@($legacyUnsafe.resultStatus.reasons) -contains 'legacy_ambiguous_fanout') -Message 'Explicit legacy fan-out omitted its unsafe reason'
    Assert-D20 -Condition ($legacyUnsafeMethods -contains 'IntTarget' -and $legacyUnsafeMethods -contains 'StringTarget') -Message 'Explicit legacy policy did not expose the expected unsafe fan-out'
    Add-Check -Name 'legacyUnsafePolicy' -Passed $true -Details 'Explicit legacy fan-out is available and marked unsafe'

    $namespaceCall = Invoke-McpTool -Session $script:McpSession -ToolName 'xray_callers' -MeasurementName 'namespaceDownMs' -Arguments @{
        method = @('CallTwo')
        class = 'NamespaceCaller'
        direction = 'down'
        depth = 1
    }
    $namespaceRoutes = @(Get-CallNode -Output $namespaceCall -MethodName 'Route')
    Assert-D20 -Condition ($namespaceRoutes.Count -eq 1 -and $namespaceRoutes[0].line -eq $twoIntRouteLine -and $namespaceRoutes[0].file -like '*Two.Router.cs') -Message 'Qualified receiver crossed the namespace boundary'
    Add-Check -Name 'namespaceIsolation' -Passed $true -Details 'Qualified Two.Router receiver resolves only to Two.Router.Route(int)'

    $candidateShutdown = Close-McpSession -Session $script:McpSession
    $script:McpSession = $null
    Assert-D20 -Condition ($candidateShutdown.passed) -Message "Candidate MCP did not shut down cleanly: $($candidateShutdown.details)"
    Add-Check -Name 'candidateMcpShutdown' -Passed $true -Details $candidateShutdown.details

    if ($IncludeBaselineControl) {
        $script:BaselineWorktree = Join-Path ([IO.Path]::GetTempPath()) "xray-d20-baseline-$RunId"
        $worktreeAdd = Invoke-RequiredProcess -FilePath 'git' -ArgumentList @('worktree', 'add', '--detach', $script:BaselineWorktree, $BaselineCommit) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds -Description 'baseline worktree add'
        $script:Timings.baselineWorktreeAddMs = $worktreeAdd.durationMs
        $baselineCommitResult = Invoke-RequiredProcess -FilePath 'git' -ArgumentList @('rev-parse', 'HEAD') -WorkingDirectory $script:BaselineWorktree -Description 'baseline rev-parse'
        Assert-D20 -Condition ($baselineCommitResult.stdout.Trim() -eq $BaselineCommit) -Message 'Baseline worktree resolved to an unexpected commit'
        $baselineLockSha256 = Get-Sha256 -Path (Join-Path $script:BaselineWorktree 'Cargo.lock')
        Assert-D20 -Condition ($baselineLockSha256 -eq $script:CargoLockSha256) -Message 'Baseline and candidate Cargo.lock differ; comparison would be invalid'

        $baselineTarget = Join-Path $ArtifactRoot 'baseline-target'
        $baselineBuild = Invoke-RequiredProcess -FilePath 'cargo' -ArgumentList @('build', '--release', '--locked', '--target-dir', $baselineTarget) -WorkingDirectory $script:BaselineWorktree -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'baseline release build'
        $script:Timings.baselineReleaseBuildMs = $baselineBuild.durationMs
        $baselineBinary = Join-Path $baselineTarget 'release/xray.exe'
        Assert-D20 -Condition (Test-Path $baselineBinary -PathType Leaf) -Message 'Baseline release binary was not produced'
        $baselineBinarySha256 = Get-Sha256 -Path $baselineBinary

        $script:BaselineControl = Invoke-BaselineSemanticControl -BinaryPath $baselineBinary -Directory $script:FixtureDirectory -IntRouteLine $intRouteLine -StringRouteLine $stringRouteLine -TwoRouteLine $twoIntRouteLine
        $script:BaselineControl.baselineBinarySha256 = $baselineBinarySha256
        $script:BaselineControl.cargoLockSha256 = $baselineLockSha256
        $script:BaselineControl.comparability = 'sameMachineToolchainLockFixture; ambientDefenderAndIndexerUncontrolled'
        Add-Check -Name 'baselineRedControl' -Passed $true -Details "Expected failures: $($script:BaselineControl.failedAssertions -join ', ')" -DurationMs $baselineBuild.durationMs


        $script:MigrationSmoke = Invoke-MigrationSmoke -BaselineBinaryPath $baselineBinary -CandidateBinaryPath $script:CandidateBinary -Directory $script:FixtureDirectory -ExpectedIntSymbolId $intRoute.symbolId
        $script:Timings.migrationV6BuildMs = $script:MigrationSmoke.v6BuildDurationMs
        $script:Timings.persistedDefinitionIndexBytes = $script:MigrationSmoke.v7Bytes
        Add-Check -Name 'v6ToV7Migration' -Passed $true -Details 'Genuine v6 rejected by the header precheck, rebuilt to v7, and reloaded with stable exact identity' -DurationMs $script:MigrationSmoke.v6BuildDurationMs
    }

    $lifecycleDirectory = Join-Path $ArtifactRoot 'lifecycle'
    $script:LifecycleSmoke = Invoke-IncrementalLifecycleSmoke -BinaryPath $script:CandidateBinary -Directory $lifecycleDirectory
    Add-Check -Name 'incrementalLifecycle' -Passed $true -Details 'Add/delete/rename/reindex and extension reparse converged through observable MCP outcomes'
}
catch {
    $script:Failure = $_
    Add-Check -Name 'gateExecution' -Passed $false -Details $_.Exception.Message
}
finally {
    if ($null -ne $script:McpSession) {
        $shutdown = Close-McpSession -Session $script:McpSession
        Add-Check -Name 'mcpShutdown' -Passed $shutdown.passed -Details $shutdown.details
        $script:McpSession = $null
    }

    if ($null -ne $script:CandidateBinary -and $null -ne $script:FixtureDirectory -and (Test-Path $script:FixtureDirectory)) {
        try {
            $cleanup = Invoke-CapturedProcess -FilePath $script:CandidateBinary -ArgumentList @('cleanup', '--dir', $script:FixtureDirectory) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds
            $script:CleanupStatus = if ($cleanup.exitCode -eq 0) { 'passed' } else { "failed:exitCode=$($cleanup.exitCode)" }
        }
        catch {
            $script:CleanupStatus = "failed:$($_.Exception.Message)"
        }
    }
    else {
        $script:CleanupStatus = 'notRequired'
    }

    if ($null -ne $script:BaselineWorktree -and (Test-Path $script:BaselineWorktree)) {
        try {
            $removeWorktree = Invoke-CapturedProcess -FilePath 'git' -ArgumentList @('worktree', 'remove', '--force', $script:BaselineWorktree) -WorkingDirectory $RepoRoot -ProcessTimeoutSeconds $TimeoutSeconds
            if ($removeWorktree.exitCode -ne 0) {
                $script:CleanupStatus = "failed:baselineWorktreeExitCode=$($removeWorktree.exitCode)"
            }
        }
        catch {
            $script:CleanupStatus = "failed:baselineWorktree:$($_.Exception.Message)"
        }
    }


    if ($KeepArtifacts) {
        $script:ArtifactCleanupStatus = 'kept'
    }
    elseif (Test-Path $ArtifactRoot) {
        try {
            Remove-Item -Recurse -Force $ArtifactRoot -ErrorAction Stop
            if (Test-Path $ArtifactRoot) {
                throw "Artifact directory still exists after removal: $ArtifactRoot"
            }
            $script:ArtifactCleanupStatus = 'passed'
        }
        catch {
            $script:ArtifactCleanupStatus = "failed:$($_.Exception.Message)"
            if ($script:CleanupStatus -notlike 'failed*') {
                $script:CleanupStatus = "failed:artifacts:$($_.Exception.Message)"
            }
        }
    }
    else {
        $script:ArtifactCleanupStatus = 'notRequired'
    }

    $endedUtc = [DateTime]::UtcNow.ToString('o')
    $passed = ($null -eq $script:Failure) -and (@($script:Checks | Where-Object { -not $_.passed }).Count -eq 0) -and ($script:CleanupStatus -notlike 'failed*')
    $result = [ordered]@{
        schemaVersion = 1
        startedUtc = if ($null -ne $script:Checks -and $script:Checks.Count -gt 0) { $scriptStartUtc } else { $endedUtc }
        endedUtc = $endedUtc
        passed = $passed
        candidateCommit = $script:CandidateCommit
        baselineCommit = $BaselineCommit
        cargoLockSha256 = $script:CargoLockSha256
        baselineControl = $script:BaselineControl
        migrationSmoke = $script:MigrationSmoke
        lifecycleSmoke = $script:LifecycleSmoke
        candidateBinarySha256 = if ($null -ne (Get-Variable candidateBinarySha256 -ErrorAction SilentlyContinue)) { $candidateBinarySha256 } else { $null }
        fixtureManifestSha256 = $script:FixtureManifestSha256
        fixtureManifest = $script:FixtureManifest
        environment = [ordered]@{
            os = [Runtime.InteropServices.RuntimeInformation]::OSDescription
            architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
            rustc = if ($null -ne (Get-Variable rustcInfo -ErrorAction SilentlyContinue)) { $rustcInfo.stdout.Trim() } else { $null }
            cargo = if ($null -ne (Get-Variable cargoInfo -ErrorAction SilentlyContinue)) { $cargoInfo.stdout.Trim() } else { $null }
            dotnet = if ($null -ne (Get-Variable dotnetInfo -ErrorAction SilentlyContinue)) { $dotnetInfo.stdout.Trim() } else { $null }
        }
        compilerOracle = $script:CompilerOracle
        checks = @($script:Checks)
        timings = $script:Timings
        queryDurationsMs = $script:QueryTimings
        cleanup = [ordered]@{
            status = $script:CleanupStatus
            artifactsKept = [bool]$KeepArtifacts
            artifactRemoval = $script:ArtifactCleanupStatus
        }
    }

    $outputDirectory = Split-Path -Parent $OutputPath
    if (-not [string]::IsNullOrWhiteSpace($outputDirectory)) {
        [IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
    }
    [IO.File]::WriteAllText($OutputPath, ($result | ConvertTo-Json -Depth 100), [Text.UTF8Encoding]::new($false))
}

if ($null -ne $script:Failure) {
    Write-Error "D20 release gate failed: $($script:Failure.Exception.Message). Result: $OutputPath"
    exit 1
}
if (@($script:Checks | Where-Object { -not $_.passed }).Count -gt 0 -or $script:CleanupStatus -like 'failed*') {
    Write-Error "D20 release gate failed during cleanup. Result: $OutputPath"
    exit 1
}

Write-Output "D20 release gate passed. Result: $OutputPath"
