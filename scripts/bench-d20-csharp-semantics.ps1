#requires -Version 7.0

[CmdletBinding()]
param(
    [ValidateSet('Development', 'Release')]
    [string]$BenchmarkProfile = 'Development',
    [ValidateRange(0, 1000000)]
    [int]$MethodsPerCorpus = 0,
    [ValidateRange(1, 1000)]
    [int]$ClassesPerFile = 64,
    [uint64]$Seed = 0xD20C5,
    [string]$OutputDirectory,
    [string]$ResultPath,
    [string]$Binary,
    [switch]$BuildRelease,
    [switch]$GenerateOnly,
    [switch]$KeepArtifacts,
    [switch]$ValidationSelfTest,
    [string]$ExpectedManifestPath,
    [ValidateRange(0, 20)]
    [int]$ColdBuildCount = 0,
    [ValidateRange(0, 1000)]
    [int]$WarmQueryCount = 0,
    [ValidateRange(0, 100)]
    [int]$IncrementalSampleCount = 0,
    [ValidateRange(5, 600)]
    [int]$TimeoutSeconds = 120,
    [ValidateRange(60, 7200)]
    [int]$BuildTimeoutSeconds = 1800
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:RequestTimeoutSeconds = $TimeoutSeconds

$RepoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($ExpectedManifestPath)) {
    $ExpectedManifestPath = Join-Path $RepoRoot 'benches/fixtures/d20-corpus-manifest.json'
}
elseif (-not [IO.Path]::IsPathRooted($ExpectedManifestPath)) {
    $ExpectedManifestPath = Join-Path $RepoRoot $ExpectedManifestPath
}
$GeneratorVersion = 1
$RunId = [Guid]::NewGuid().ToString('N')
if ($MethodsPerCorpus -eq 0) {
    $MethodsPerCorpus = if ($BenchmarkProfile -eq 'Release') { 100000 } else { 1000 }
}
if ($MethodsPerCorpus -lt 32) {
    throw 'MethodsPerCorpus must be at least 32 so every semantic control is present.'
}
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $RepoRoot "target/d20-scale/corpus-$RunId"
}
elseif (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $RepoRoot $OutputDirectory
}
if ([string]::IsNullOrWhiteSpace($ResultPath)) {
    $ResultPath = if ($GenerateOnly) {
        Join-Path $OutputDirectory 'generation-result.json'
    }
    else {
        Join-Path $RepoRoot 'target/d20-scale-result.json'
    }
}
elseif (-not [IO.Path]::IsPathRooted($ResultPath)) {
    $ResultPath = Join-Path $RepoRoot $ResultPath
}

function Write-Utf8File {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Content
    )

    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        [IO.Directory]::CreateDirectory($parent) | Out-Null
    }
    [IO.File]::WriteAllText(
        $Path,
        $Content.Replace("`r`n", "`n"),
        [Text.UTF8Encoding]::new($false)
    )
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$Path)

    $bytes = [IO.File]::ReadAllBytes($Path)
    return [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData($bytes)
    ).ToLowerInvariant()
}

function Initialize-OwnedDirectory {
    param([Parameter(Mandatory)][string]$Path)

    if (Test-Path $Path) {
        throw "OutputDirectory already exists; refusing to overwrite it: $Path"
    }
    [IO.Directory]::CreateDirectory($Path) | Out-Null
    Write-Utf8File -Path (Join-Path $Path '.xray-d20-corpus') -Content "schemaVersion=1`nrunId=$RunId`n"
}

function Add-SourceFile {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]]$Files,
        [Parameter(Mandatory)][string]$CorpusRoot,
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $path = Join-Path $CorpusRoot $RelativePath
    Write-Utf8File -Path $path -Content $Content
    $item = Get-Item $path
    $Files.Add([ordered]@{
        path = $RelativePath.Replace('\', '/')
        bytes = $item.Length
        sha256 = Get-Sha256 -Path $path
    })
}

function Write-ClassChunkSet {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]]$Files,
        [Parameter(Mandatory)][string]$CorpusRoot,
        [Parameter(Mandatory)][string]$Prefix,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[string]]$Classes,
        [Parameter(Mandatory)][int]$ChunkSize
    )

    $fileIndex = 0
    for ($offset = 0; $offset -lt $Classes.Count; $offset += $ChunkSize) {
        $count = [Math]::Min($ChunkSize, $Classes.Count - $offset)
        $builder = [Text.StringBuilder]::new()
        for ($index = 0; $index -lt $count; $index++) {
            [void]$builder.Append($Classes[$offset + $index])
            [void]$builder.Append("`n")
        }
        Add-SourceFile -Files $Files -CorpusRoot $CorpusRoot `
            -RelativePath ("{0}-{1:d6}.cs" -f $Prefix, $fileIndex) `
            -Content $builder.ToString()
        $fileIndex++
    }
}

function Initialize-OverloadCorpus {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][int]$TargetMethods,
        [Parameter(Mandatory)][int]$ChunkSize,
        [Parameter(Mandatory)][uint64]$CorpusSeed
    )

    $files = [System.Collections.Generic.List[object]]::new()
    $classes = [System.Collections.Generic.List[string]]::new()
    $seedText = $CorpusSeed.ToString('x')
    $probe = @"
namespace D20.Scale.Overloads.Seed$seedText
{
    public sealed class ProbeRouter
    {
        public void Route(int value) => IntTarget();
        public void Route(string value) => StringTarget();
        public void CallInt() => Route(1);
        public void CallString() => Route("x");
        public void CallUnknown(dynamic value) => Route(value);
        private void IntTarget() { }
        private void StringTarget() { }
    }

    public sealed class LifecycleRouter
    {
        public void Route(int value) => IntTarget();
        // D20_INCREMENTAL_OVERLOAD_SLOT
        public void CallUnknown(dynamic value) => Route(value);
        private void IntTarget() { }
        private void StringTarget() { }
    }
}
"@
    Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'overloads-probe.cs' -Content $probe

    $methodCount = 11
    $overloadCounts = @(1, 2, 4, 8, 16)
    $parameterTypes = @(
        'int', 'string', 'long', 'bool', 'char', 'double', 'float', 'decimal',
        'byte', 'short', 'uint', 'ulong', 'ushort', 'sbyte', 'object', 'System.Guid'
    )
    $groupCounts = [ordered]@{ '1' = 0; '2' = 0; '4' = 0; '8' = 0; '16' = 0 }
    $probeGroups = [ordered]@{}
    $groupIndex = 0
    while ($methodCount -lt $TargetMethods) {
        $remaining = $TargetMethods - $methodCount
        $patternIndex = [int](($CorpusSeed + [uint64]$groupIndex) % [uint64]$overloadCounts.Count)
        $overloadCount = $overloadCounts[$patternIndex]
        if ($overloadCount -gt $remaining) {
            break
        }

        $builder = [Text.StringBuilder]::new()
        [void]$builder.AppendLine("namespace D20.Scale.Overloads.G$groupIndex")
        [void]$builder.AppendLine('{')
        [void]$builder.AppendLine("    public sealed class Router$groupIndex")
        [void]$builder.AppendLine('    {')
        for ($overloadIndex = 0; $overloadIndex -lt $overloadCount; $overloadIndex++) {
            $parameterType = $parameterTypes[$overloadIndex]
            [void]$builder.AppendLine(
                "        public int Route($parameterType value) => $overloadIndex;"
            )
        }
        [void]$builder.AppendLine('    }')
        [void]$builder.AppendLine('}')
        $classes.Add($builder.ToString())
        $groupCounts[[string]$overloadCount]++
        if (-not $probeGroups.Contains([string]$overloadCount)) {
            $probeGroups[[string]$overloadCount] = [ordered]@{
                class = "Router$groupIndex"
                qualifiedType = "D20.Scale.Overloads.G$groupIndex.Router$groupIndex"
                expectedCandidates = $overloadCount
            }
        }
        $methodCount += $overloadCount
        $groupIndex++
    }

    Write-ClassChunkSet -Files $files -CorpusRoot $Root -Prefix 'overloads' `
        -Classes $classes -ChunkSize $ChunkSize

    $paddingMethods = $TargetMethods - $methodCount
    if ($paddingMethods -gt 0) {
        $builder = [Text.StringBuilder]::new()
        [void]$builder.AppendLine('namespace D20.Scale.Overloads.Padding')
        [void]$builder.AppendLine('{')
        [void]$builder.AppendLine('    public sealed class PaddingMethods')
        [void]$builder.AppendLine('    {')
        for ($index = 0; $index -lt $paddingMethods; $index++) {
            [void]$builder.AppendLine("        public void Pad$index() { }")
        }
        [void]$builder.AppendLine('    }')
        [void]$builder.AppendLine('}')
        Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'overloads-padding.cs' -Content $builder.ToString()
        $methodCount += $paddingMethods
    }

    return [ordered]@{
        name = 'synthetic-overloads'
        methodCount = $methodCount
        sourceFileCount = $files.Count
        overloadGroupCounts = $groupCounts
        probeGroups = $probeGroups
        probes = [ordered]@{
            qualifiedType = "D20.Scale.Overloads.Seed$seedText.ProbeRouter"
            exactCaller = 'CallInt'
            ambiguousCaller = 'CallUnknown'
            targetMethod = 'Route'
        }
        incremental = [ordered]@{
            relativePath = 'overloads-probe.cs'
            initialState = 'exact'
            mutatedState = 'ambiguous'
            class = 'LifecycleRouter'
            caller = 'CallUnknown'
            marker = '// D20_INCREMENTAL_OVERLOAD_SLOT'
            addedDeclaration = '        public void Route(string value) => StringTarget();'
            baselineSha256 = $files[0].sha256
        }
        files = @($files)
    }
}

function Initialize-NamespaceCorpus {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][int]$TargetMethods,
        [Parameter(Mandatory)][int]$ChunkSize,
        [Parameter(Mandatory)][uint64]$CorpusSeed
    )

    $files = [System.Collections.Generic.List[object]]::new()
    $classes = [System.Collections.Generic.List[string]]::new()
    $seedText = $CorpusSeed.ToString('x')
    $probe = @"
namespace D20.Scale.Namespaces.Seed$seedText.One
{
    public sealed class Collision
    {
        public void Route(int value) => OneTarget();
        private void OneTarget() { }
    }
}
namespace D20.Scale.Namespaces.Seed$seedText.Two
{
    public sealed class Collision
    {
        public void Route(int value) => TwoTarget();
        private void TwoTarget() { }
    }
}
namespace D20.Scale.Namespaces.Seed$seedText
{
    public sealed class NamespaceCaller
    {
        public void CallOne() => new D20.Scale.Namespaces.Seed$seedText.One.Collision().Route(1);
        public void CallTwo() => new D20.Scale.Namespaces.Seed$seedText.Two.Collision().Route(1);
    }
}
"@
    Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'namespaces-probe.cs' -Content $probe

    $partialA = @"
namespace D20.Scale.Namespaces.Seed$seedText.Partials
{
    public partial class PartialRouter
    {
        public void PartA() => SharedTarget();
        private void SharedTarget() { }
    }
}
"@
    $partialB = @"
namespace D20.Scale.Namespaces.Seed$seedText.Partials
{
    public partial class PartialRouter
    {
        public void PartB() => SharedTarget();
    }
}
"@
    $shapeControls = @"
namespace D20.Scale.Namespaces.Seed$seedText.Shapes
{
    public sealed class Box<T> { public void Store(T value) { } }
    public sealed class Box<T, U> { public void Pair(T first, U second) { } }
    public sealed class Outer { public sealed class Inner { public void Route(int value) { } } }
}
namespace D20.Scale.Namespaces.Seed$seedText.Extensions
{
    public static class RouterExtensions
    {
        public static void ExtensionPulse(this Partials.PartialRouter router) { }
    }
    public sealed class ExtensionCaller
    {
        public void Call(Partials.PartialRouter router) => router.ExtensionPulse();
    }
}
"@
    Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'namespaces-partial-a.cs' -Content $partialA
    Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'namespaces-partial-b.cs' -Content $partialB
    Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'namespaces-shapes.cs' -Content $shapeControls

    $methodCount = 14
    $namespaceIndex = 0
    while (($methodCount + 6) -le $TargetMethods) {
        $logicalIndex = [uint64]$namespaceIndex + $CorpusSeed
        $builder = [Text.StringBuilder]::new()
        [void]$builder.AppendLine("namespace D20.Scale.Namespaces.N$logicalIndex")
        [void]$builder.AppendLine('{')
        [void]$builder.AppendLine('    public sealed class Collision')
        [void]$builder.AppendLine('    {')
        [void]$builder.AppendLine('        public void Route(int value) => IntTarget();')
        [void]$builder.AppendLine('        public void Route(string value) => StringTarget();')
        [void]$builder.AppendLine('        public void CallInt() => Route(1);')
        [void]$builder.AppendLine('        public void CallUnknown(dynamic value) => Route(value);')
        [void]$builder.AppendLine('        private void IntTarget() { }')
        [void]$builder.AppendLine('        private void StringTarget() { }')
        [void]$builder.AppendLine('    }')
        [void]$builder.AppendLine('}')
        $classes.Add($builder.ToString())
        $methodCount += 6
        $namespaceIndex++
    }

    Write-ClassChunkSet -Files $files -CorpusRoot $Root -Prefix 'namespaces' `
        -Classes $classes -ChunkSize $ChunkSize

    $paddingMethods = $TargetMethods - $methodCount
    if ($paddingMethods -gt 0) {
        $builder = [Text.StringBuilder]::new()
        [void]$builder.AppendLine('namespace D20.Scale.Namespaces.Padding')
        [void]$builder.AppendLine('{')
        [void]$builder.AppendLine('    public sealed class PaddingMethods')
        [void]$builder.AppendLine('    {')
        for ($index = 0; $index -lt $paddingMethods; $index++) {
            [void]$builder.AppendLine("        public void Pad$index() { }")
        }
        [void]$builder.AppendLine('    }')
        [void]$builder.AppendLine('}')
        Add-SourceFile -Files $files -CorpusRoot $Root -RelativePath 'namespaces-padding.cs' -Content $builder.ToString()
        $methodCount += $paddingMethods
    }

    return [ordered]@{
        name = 'synthetic-namespaces'
        methodCount = $methodCount
        sourceFileCount = $files.Count
        namespaceCollisionCount = $namespaceIndex
        probes = [ordered]@{
            callerQualifiedType = "D20.Scale.Namespaces.Seed$seedText.NamespaceCaller"
            oneQualifiedType = "D20.Scale.Namespaces.Seed$seedText.One.Collision"
            twoQualifiedType = "D20.Scale.Namespaces.Seed$seedText.Two.Collision"
            callerMethod = 'CallOne'
            targetMethod = 'Route'
        }
        files = @($files)
    }
}

function Assert-ExpectedReleaseManifest {
    param([Parameter(Mandatory)][object]$ActualManifest)

    if ($BenchmarkProfile -ne 'Release') {
        return
    }
    if (-not (Test-Path $ExpectedManifestPath -PathType Leaf)) {
        throw "Expected D20 corpus manifest not found: $ExpectedManifestPath"
    }
    $expected = Get-Content -Raw $ExpectedManifestPath | ConvertFrom-Json
    foreach ($property in @(
        'schemaVersion', 'generatorVersion', 'seed', 'profile',
        'methodsPerCorpus', 'classesPerFile', 'fixtureManifestSha256'
    )) {
        if ([string]$ActualManifest.$property -cne [string]$expected.$property) {
            throw "D20 corpus manifest drift in '$property': expected '$($expected.$property)', actual '$($ActualManifest.$property)'"
        }
    }

    $actualByName = @{}
    foreach ($corpus in @($ActualManifest.corpora)) { $actualByName[$corpus.name] = $corpus }
    $expectedByName = @{}
    foreach ($corpus in @($expected.corpora)) { $expectedByName[$corpus.name] = $corpus }
    if (($actualByName.Keys | Sort-Object) -join ',' -cne
        (($expectedByName.Keys | Sort-Object) -join ',')) {
        throw 'D20 corpus manifest drift in corpus names.'
    }
    foreach ($name in @($expectedByName.Keys)) {
        $actualCorpus = $actualByName[$name]
        $expectedCorpus = $expectedByName[$name]
        foreach ($property in @('methodCount', 'sourceFileCount')) {
            if ([string]$actualCorpus.$property -cne [string]$expectedCorpus.$property) {
                throw "D20 corpus '$name' drift in '$property'."
            }
        }
    }

    $actualOverloads = $actualByName['synthetic-overloads']
    $expectedOverloads = $expectedByName['synthetic-overloads']
    foreach ($count in @('1', '2', '4', '8', '16')) {
        if ([string]$actualOverloads.overloadGroupCounts.$count -cne
            [string]$expectedOverloads.overloadGroupCounts.$count) {
            throw "D20 overload group '$count' drift."
        }
    }
    foreach ($property in @('relativePath', 'initialState', 'mutatedState', 'baselineSha256')) {
        if ([string]$actualOverloads.incremental.$property -cne
            [string]$expectedOverloads.incrementalProbe.$property) {
            throw "D20 incremental probe drift in '$property'."
        }
    }

    $actualNamespaces = $actualByName['synthetic-namespaces']
    $expectedNamespaces = $expectedByName['synthetic-namespaces']
    if ([string]$actualNamespaces.namespaceCollisionCount -cne
        [string]$expectedNamespaces.namespaceCollisionCount) {
        throw 'D20 namespace collision count drift.'
    }
}


function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = $RepoRoot,
        [int]$ProcessTimeoutSeconds = $script:RequestTimeoutSeconds
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
        $deadline = [DateTime]::UtcNow.AddSeconds($ProcessTimeoutSeconds)
        [long]$peakWorkingSetBytes = 0
        while (-not $process.WaitForExit(25)) {
            try {
                $process.Refresh()
                $peakWorkingSetBytes = [Math]::Max(
                    $peakWorkingSetBytes,
                    [Math]::Max($process.WorkingSet64, $process.PeakWorkingSet64)
                )
            }
            catch {
                Write-Verbose 'Working set sample was unavailable.'
            }
            if ([DateTime]::UtcNow -ge $deadline) {
                $process.Kill($true)
                $process.WaitForExit()
                throw "Process timed out after $ProcessTimeoutSeconds seconds: $FilePath $($ArgumentList -join ' ')"
            }
        }
        try {
            $process.Refresh()
            $peakWorkingSetBytes = [Math]::Max(
                $peakWorkingSetBytes,
                [Math]::Max($process.WorkingSet64, $process.PeakWorkingSet64)
            )
        }
        catch {
            Write-Verbose 'Final working set sample was unavailable.'
        }
        return [pscustomobject]@{
            exitCode = $process.ExitCode
            stdout = $stdoutTask.GetAwaiter().GetResult()
            stderr = $stderrTask.GetAwaiter().GetResult()
            durationMs = $stopwatch.Elapsed.TotalMilliseconds
            peakWorkingSetBytes = $peakWorkingSetBytes
        }
    }
    finally {
        $stopwatch.Stop()
        $process.Dispose()
    }
}

function Invoke-RequiredProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = $RepoRoot,
        [int]$ProcessTimeoutSeconds = $script:RequestTimeoutSeconds,
        [string]$Description = $FilePath
    )

    $result = Invoke-CapturedProcess -FilePath $FilePath -ArgumentList $ArgumentList `
        -WorkingDirectory $WorkingDirectory -ProcessTimeoutSeconds $ProcessTimeoutSeconds
    if ($result.exitCode -ne 0) {
        $output = ($result.stdout + [Environment]::NewLine + $result.stderr).Trim()
        throw "$Description failed with exit code $($result.exitCode): $output"
    }
    return $result
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

function Get-Median {
    param([Parameter(Mandatory)][AllowEmptyCollection()][double[]]$Values)

    if ($Values.Count -eq 0) {
        return $null
    }
    $sorted = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if (($sorted.Count % 2) -eq 1) {
        return $sorted[$middle]
    }
    return ($sorted[$middle - 1] + $sorted[$middle]) / 2.0
}

function Get-NearestRankPercentile {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][double[]]$Values,
        [Parameter(Mandatory)][ValidateRange(0.0, 1.0)][double]$Percentile
    )

    if ($Values.Count -eq 0) {
        return $null
    }
    $sorted = @($Values | Sort-Object)
    $rank = [Math]::Max(1, [Math]::Ceiling($Percentile * $sorted.Count))
    return $sorted[[int]$rank - 1]
}

function Get-SampleSummary {
    param([Parameter(Mandatory)][AllowEmptyCollection()][double[]]$Values)

    return [ordered]@{
        count = $Values.Count
        min = if ($Values.Count -gt 0) { ($Values | Measure-Object -Minimum).Minimum } else { $null }
        p50 = Get-Median -Values $Values
        p95 = Get-NearestRankPercentile -Values $Values -Percentile 0.95
        p99 = Get-NearestRankPercentile -Values $Values -Percentile 0.99
        max = if ($Values.Count -gt 0) { ($Values | Measure-Object -Maximum).Maximum } else { $null }
        raw = @($Values)
    }
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

function Open-McpSession {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$Directory,
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
    foreach ($argument in @(
        'serve', '--dir', $Directory, '--ext', 'cs', '--definitions', '--metrics',
        '--log-level', 'warn', '--max-response-kb', '256'
    )) {
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
    return [pscustomobject]@{
        Process = $process
        StderrTask = $process.StandardError.ReadToEndAsync()
        NextId = 1
    }
}

function Send-McpNotification {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][string]$Method,
        [object]$Params = @{}
    )

    $payload = [ordered]@{ jsonrpc = '2.0'; method = $Method; params = $Params } |
        ConvertTo-Json -Compress -Depth 50
    $Session.Process.StandardInput.WriteLine($payload)
    $Session.Process.StandardInput.Flush()
}

function Send-McpRequest {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][string]$Method,
        [object]$Params = @{}
    )

    $id = $Session.NextId
    $Session.NextId++
    $payload = [ordered]@{ jsonrpc = '2.0'; id = $id; method = $Method; params = $Params } |
        ConvertTo-Json -Compress -Depth 50
    $Session.Process.StandardInput.WriteLine($payload)
    $Session.Process.StandardInput.Flush()

    $deadline = [DateTime]::UtcNow.AddSeconds($script:RequestTimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remainingMs = [int][Math]::Max(1, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        $readTask = $Session.Process.StandardOutput.ReadLineAsync()
        if (-not $readTask.Wait($remainingMs)) {
            throw "Timed out waiting for MCP response id $id ($Method)"
        }
        $line = $readTask.GetAwaiter().GetResult()
        if ($null -eq $line) {
            throw "MCP server closed stdout while waiting for id $id ($Method)"
        }
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
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

function Invoke-McpTool {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][string]$ToolName,
        [Parameter(Mandatory)][hashtable]$Arguments
    )

    $response = Send-McpRequest -Session $Session -Method 'tools/call' -Params @{
        name = $ToolName
        arguments = $Arguments
    }
    $textItem = @($response.result.content) | Where-Object { $_.type -eq 'text' } |
        Select-Object -First 1
    if ($null -ne $response.result.PSObject.Properties['isError'] -and $response.result.isError) {
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
        return [ordered]@{ passed = $true; details = 'notStarted'; stderr = '' }
    }
    $process = $Session.Process
    try {
        if (-not $process.HasExited) {
            $process.StandardInput.Close()
            if (-not $process.WaitForExit($script:RequestTimeoutSeconds * 1000)) {
                $process.Kill($true)
                $process.WaitForExit()
                return [ordered]@{
                    passed = $false
                    details = 'forcedTerminationAfterTimeout'
                    stderr = $Session.StderrTask.GetAwaiter().GetResult()
                }
            }
        }
        $stderr = $Session.StderrTask.GetAwaiter().GetResult()
        return [ordered]@{
            passed = ($process.ExitCode -eq 0)
            details = if ($process.ExitCode -eq 0) { 'stdinClosedAndExited' } else { "exitCode=$($process.ExitCode)" }
            stderr = $stderr
        }
    }
    finally {
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $process.Dispose()
    }
}

function Get-DefinitionIndexSnapshot {
    $indexRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'xray'
    if (-not (Test-Path $indexRoot)) {
        return @()
    }
    return @(Get-ChildItem -Path $indexRoot -Filter '*.code-structure' -File |
        ForEach-Object { $_.FullName })
}

function Measure-DefinitionBuildSet {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$CorpusRoot,
        [Parameter(Mandatory)][int]$BuildCount
    )

    $samples = [System.Collections.Generic.List[object]]::new()
    $finalIndexPath = $null
    for ($iteration = 1; $iteration -le $BuildCount; $iteration++) {
        [void](Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @(
            'cleanup', '--dir', $CorpusRoot
        ) -Description 'pre-build cleanup')
        $before = @(Get-DefinitionIndexSnapshot)
        $build = Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @(
            'def-index', '--dir', $CorpusRoot, '--ext', 'cs'
        ) -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'definition-index build'
        $after = @(Get-DefinitionIndexSnapshot)
        $newIndexes = @($after | Where-Object { $before -notcontains $_ })
        if ($newIndexes.Count -ne 1) {
            throw "Expected one new definition index after build, found $($newIndexes.Count)"
        }
        $finalIndexPath = $newIndexes[0]
        $indexBytes = (Get-Item $finalIndexPath).Length
        $samples.Add([ordered]@{
            iteration = $iteration
            durationMs = $build.durationMs
            peakWorkingSetBytes = $build.peakWorkingSetBytes
            persistedIndexBytes = $indexBytes
        })
    }

    return [ordered]@{
        samples = @($samples)
        durationMs = Get-SampleSummary -Values @($samples | ForEach-Object { [double]$_.durationMs })
        peakWorkingSetBytes = Get-SampleSummary -Values @($samples | ForEach-Object { [double]$_.peakWorkingSetBytes })
        persistedIndexBytes = Get-SampleSummary -Values @($samples | ForEach-Object { [double]$_.persistedIndexBytes })
        finalIndexFile = Split-Path -Leaf $finalIndexPath
    }
}

function Get-CallNode {
    param(
        [Parameter(Mandatory)][object]$Output,
        [Parameter(Mandatory)][string]$MethodName
    )

    $nodes = [System.Collections.Generic.List[object]]::new()
    foreach ($node in @($Output.callTree)) {
        if ($node.method -eq $MethodName) {
            $nodes.Add($node)
        }
    }
    return @($nodes)
}

function Get-DefinitionStartLine {
    param([Parameter(Mandatory)][object]$Definition)

    if ($null -ne $Definition.PSObject.Properties['line']) {
        return [int]$Definition.line
    }
    if ($null -ne $Definition.PSObject.Properties['bodyStartLine']) {
        return [int]$Definition.bodyStartLine
    }
    if ($null -ne $Definition.PSObject.Properties['lines'] -and
        [string]$Definition.lines -match '^(\d+)') {
        return [int]$Matches[1]
    }
    throw "Definition has no source line metadata: $($Definition | ConvertTo-Json -Compress -Depth 20)"
}


function Test-QueryOutput {
    param(
        [Parameter(Mandatory)][object]$Specification,
        [Parameter(Mandatory)][object]$Output,
        [Parameter(Mandatory)][hashtable]$Context
    )

    switch ($Specification.validation) {
        'overloadDefinitions' {
            $definitions = @($Output.definitions | Where-Object {
                $_.qualifiedType -eq $Specification.qualifiedType -and $_.name -eq 'Route'
            })
            if ($definitions.Count -ne 2) {
                throw "Expected two overload probe definitions, found $($definitions.Count)"
            }
            $ids = @($definitions | ForEach-Object { $_.symbolId } | Sort-Object -Unique)
            if ($ids.Count -ne 2 -or @($ids | Where-Object { $_ -notmatch '^cs:v1:[0-9a-f]{64}$' }).Count -gt 0) {
                throw 'Overload probe SymbolIds are missing, invalid, or merged'
            }
            $intDefinition = @($definitions | Where-Object { $_.signature -match '\bint\s+value\b' })
            $stringDefinition = @($definitions | Where-Object { $_.signature -match '\bstring\s+value\b' })
            $intLine = if ($intDefinition.Count -eq 1) {
                Get-DefinitionStartLine -Definition $intDefinition[0]
            } else { $null }
            $stringLine = if ($stringDefinition.Count -eq 1) {
                Get-DefinitionStartLine -Definition $stringDefinition[0]
            } else { $null }
            if ($intDefinition.Count -ne 1 -or $stringDefinition.Count -ne 1 -or
                $intLine -eq $stringLine) {
                throw 'Overload probe signatures or source identities are invalid'
            }
            $Context.intRoute = [ordered]@{
                symbolId = $intDefinition[0].symbolId
                qualifiedType = $intDefinition[0].qualifiedType
                signature = $intDefinition[0].signature
                line = $intLine
            }
            return [ordered]@{ candidateCount = 2; symbolIds = $ids }
        }
        'callInt' {
            $routes = @(Get-CallNode -Output $Output -MethodName 'Route')
            if (-not $Context.ContainsKey('intRoute')) {
                throw 'CallInt validation is missing definition identity evidence'
            }
            if ($routes.Count -ne 1 -or $routes[0].nodeKind -ne 'callee' -or
                [int]$routes[0].line -ne [int]$Context.intRoute.line) {
                throw 'CallInt did not resolve to the int Route definition identity'
            }
            return [ordered]@{
                candidateCount = 1
                expectedSymbolId = $Context.intRoute.symbolId
            }
        }
        'callUnknown' {
            $ambiguous = @($Output.callTree | Where-Object {
                $_.method -eq 'Route' -and $_.nodeKind -eq 'ambiguousCall'
            })
            $exact = @($Output.callTree | Where-Object {
                $_.method -eq 'Route' -and $_.nodeKind -eq 'callee'
            })
            if ($ambiguous.Count -ne 1 -or $exact.Count -ne 0 -or $Output.resultStatus.safeForExactSemantics -ne $false) {
                throw 'CallUnknown did not remain safely ambiguous'
            }
            return [ordered]@{
                candidateCount = @($ambiguous[0].resolution.candidates).Count
            }
        }
        'rootCandidates' {
            if ($Specification.expectedCandidates -eq 1) {
                $rootResolution = Get-OptionalProperty -Object $Output -Name 'rootResolution'
                if ($null -ne $rootResolution -and $rootResolution.status -ne 'exact') {
                    throw 'Single-candidate root reported a non-exact resolution'
                }
                if ($null -eq $rootResolution -and $Output.resultStatus.status -ne 'complete') {
                    throw 'Single-candidate legacy root was not complete'
                }
                return [ordered]@{ candidateCount = 1 }
            }
            if ($Output.rootResolution.status -ne 'ambiguous' -or @($Output.callTree).Count -ne 0) {
                throw "Expected ambiguous root for $($Specification.name)"
            }
            $candidates = @($Output.rootResolution.candidates)
            $listed = $candidates.Count
            if ($listed -ne $Specification.expectedCandidates) {
                throw "Candidate count $listed does not equal expected $($Specification.expectedCandidates)"
            }
            if (@($candidates | Where-Object {
                $_.qualifiedType -ne $Specification.qualifiedType -or
                $_.symbolId -notmatch '^cs:v1:[0-9a-f]{64}$'
            }).Count -gt 0) {
                throw "Root candidates were not bound to $($Specification.qualifiedType)"
            }
            return [ordered]@{ candidateCount = $listed }
        }
        'namespaceDefinitions' {
            $definitions = @($Output.definitions | Where-Object { $_.name -eq 'Route' })
            $one = @($definitions | Where-Object { $_.qualifiedType -eq $Specification.oneQualifiedType })
            $two = @($definitions | Where-Object { $_.qualifiedType -eq $Specification.twoQualifiedType })
            if ($one.Count -ne 1 -or $two.Count -ne 1 -or $one[0].symbolId -eq $two[0].symbolId) {
                throw 'Namespace probe definitions collided or were not found'
            }
            $Context.oneRoute = [ordered]@{
                symbolId = $one[0].symbolId
                qualifiedType = $one[0].qualifiedType
                signature = $one[0].signature
                line = (Get-DefinitionStartLine -Definition $one[0])
            }
            return [ordered]@{ candidateCount = 2; oneSymbolId = $one[0].symbolId }
        }
        'namespaceCallOne' {
            $routes = @(Get-CallNode -Output $Output -MethodName 'Route')
            if (-not $Context.ContainsKey('oneRoute')) {
                throw 'Namespace CallOne validation is missing definition identity evidence'
            }
            if ($routes.Count -ne 1 -or $routes[0].nodeKind -ne 'callee' -or
                [int]$routes[0].line -ne [int]$Context.oneRoute.line) {
                throw 'Namespace CallOne did not resolve to the One.Collision definition identity'
            }
            return [ordered]@{
                candidateCount = 1
                expectedSymbolId = $Context.oneRoute.symbolId
            }
        }
        default {
            throw "Unknown query validation mode: $($Specification.validation)"
        }
    }
}

function Invoke-ValidationSelfTest {
    function Assert-Rejected {
        param(
            [Parameter(Mandatory)][string]$Name,
            [Parameter(Mandatory)][scriptblock]$Action
        )

        $rejected = $false
        try { & $Action } catch { $rejected = $true }
        if (-not $rejected) { throw "Validation self-test '$Name' was not rejected." }
    }

    $intContext = @{
        intRoute = [ordered]@{
            symbolId = 'cs:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
            line = 10
        }
    }
    $callSpecification = [pscustomobject]@{ validation = 'callInt' }
    $wrongCall = '{"callTree":[{"method":"Route","nodeKind":"callee","line":11}]}' |
        ConvertFrom-Json
    Assert-Rejected -Name 'callInt wrong definition identity' -Action {
        [void](Test-QueryOutput -Specification $callSpecification -Output $wrongCall `
            -Context $intContext)
    }

    $rootSpecification = [pscustomobject]@{
        validation = 'rootCandidates'
        name = 'root_candidates_16'
        expectedCandidates = 16
        qualifiedType = 'D20.Scale.Router16'
    }
    $underCountedRoot = @{
        callTree = @()
        rootResolution = @{
            status = 'ambiguous'
            candidates = @(@{
                qualifiedType = 'D20.Scale.Router16'
                symbolId = 'cs:v1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
            })
        }
    } | ConvertTo-Json -Depth 10 | ConvertFrom-Json
    Assert-Rejected -Name 'root candidate undercount' -Action {
        [void](Test-QueryOutput -Specification $rootSpecification -Output $underCountedRoot `
            -Context @{})
    }

    $namespaceContext = @{
        oneRoute = [ordered]@{
            symbolId = 'cs:v1:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
            line = 20
        }
    }
    $namespaceSpecification = [pscustomobject]@{ validation = 'namespaceCallOne' }
    $wrongNamespace = '{"callTree":[{"method":"Route","nodeKind":"callee","line":21}]}' |
        ConvertFrom-Json
    Assert-Rejected -Name 'namespace wrong definition identity' -Action {
        [void](Test-QueryOutput -Specification $namespaceSpecification -Output $wrongNamespace `
            -Context $namespaceContext)
    }

    return [ordered]@{
        passed = $true
        rejectedMutations = 3
    }
}


function Get-QuerySpecification {
    param([Parameter(Mandatory)][object]$Corpus)

    $specifications = [System.Collections.Generic.List[object]]::new()
    if ($Corpus.name -eq 'synthetic-overloads') {
        $specifications.Add([pscustomobject]@{
            name = 'definitions_probe'
            tool = 'xray_definitions'
            arguments = @{ name = @('Route'); file = @('overloads-probe.cs'); exactNameOnly = $true }
            validation = 'overloadDefinitions'
            qualifiedType = $Corpus.probes.qualifiedType
            expectedCandidates = 2
        })
        $specifications.Add([pscustomobject]@{
            name = 'call_int_down'
            tool = 'xray_callers'
            arguments = @{ method = @('CallInt'); class = 'ProbeRouter'; direction = 'down'; depth = 1 }
            validation = 'callInt'
            expectedCandidates = 1
        })
        $specifications.Add([pscustomobject]@{
            name = 'call_unknown_down'
            tool = 'xray_callers'
            arguments = @{ method = @('CallUnknown'); class = 'ProbeRouter'; direction = 'down'; depth = 1 }
            validation = 'callUnknown'
            expectedCandidates = 2
        })
        foreach ($property in $Corpus.probeGroups.GetEnumerator()) {
            $probe = $property.Value
            if ([int]$probe.expectedCandidates -eq 1) {
                continue
            }
            $specifications.Add([pscustomobject]@{
                name = "root_candidates_$($property.Key)"
                tool = 'xray_callers'
                arguments = @{
                    method = @('Route')
                    class = $probe.class
                    direction = 'down'
                    depth = 1
                }
                validation = 'rootCandidates'
                expectedCandidates = [int]$probe.expectedCandidates
                qualifiedType = $probe.qualifiedType
            })
        }
    }
    elseif ($Corpus.name -eq 'synthetic-namespaces') {
        $specifications.Add([pscustomobject]@{
            name = 'definitions_namespace_probe'
            tool = 'xray_definitions'
            arguments = @{ name = @('Route'); file = @('namespaces-probe.cs'); exactNameOnly = $true }
            validation = 'namespaceDefinitions'
            oneQualifiedType = $Corpus.probes.oneQualifiedType
            twoQualifiedType = $Corpus.probes.twoQualifiedType
            expectedCandidates = 2
        })
        $specifications.Add([pscustomobject]@{
            name = 'namespace_call_one_down'
            tool = 'xray_callers'
            arguments = @{
                method = @('CallOne')
                class = 'NamespaceCaller'
                direction = 'down'
                depth = 1
            }
            validation = 'namespaceCallOne'
            expectedCandidates = 1
        })
    }
    else {
        throw "Unknown corpus: $($Corpus.name)"
    }
    return @($specifications)
}

function Measure-McpQuerySet {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][object]$Corpus,
        [Parameter(Mandatory)][int]$Iterations
    )

    $results = [System.Collections.Generic.List[object]]::new()
    $validationContext = @{}
    foreach ($specification in @(Get-QuerySpecification -Corpus $Corpus)) {
        $samples = [System.Collections.Generic.List[double]]::new()
        $observedCandidateCounts = [System.Collections.Generic.List[double]]::new()
        $warmupOutput = Invoke-McpTool -Session $Session -ToolName $specification.tool `
            -Arguments $specification.arguments
        [void](Test-QueryOutput -Specification $specification -Output $warmupOutput `
            -Context $validationContext)
        for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
            $stopwatch = [Diagnostics.Stopwatch]::StartNew()
            $output = Invoke-McpTool -Session $Session -ToolName $specification.tool `
                -Arguments $specification.arguments
            $stopwatch.Stop()
            $validation = Test-QueryOutput -Specification $specification -Output $output `
                -Context $validationContext
            $samples.Add($stopwatch.Elapsed.TotalMilliseconds)
            $observedCandidateCounts.Add([double]$validation.candidateCount)
        }
        $results.Add([ordered]@{
            name = $specification.name
            tool = $specification.tool
            expectedCandidateCount = $specification.expectedCandidates
            warmupCount = 1
            observedCandidateCount = Get-SampleSummary -Values @($observedCandidateCounts)
            durationMs = Get-SampleSummary -Values @($samples)
        })
    }
    return @($results)
}

function Get-IncrementalState {
    param([Parameter(Mandatory)][object]$Output)

    $ambiguous = @($Output.callTree | Where-Object {
        $_.method -eq 'Route' -and $_.nodeKind -eq 'ambiguousCall'
    })
    $exact = @($Output.callTree | Where-Object {
        $_.method -eq 'Route' -and $_.nodeKind -eq 'callee'
    })
    if ($ambiguous.Count -eq 1 -and $exact.Count -eq 0) {
        return 'ambiguous'
    }
    if ($exact.Count -eq 1 -and $ambiguous.Count -eq 0) {
        return 'exact'
    }
    return 'transitioning'
}

function Wait-IncrementalState {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][ValidateSet('exact', 'ambiguous')][string]$ExpectedState,
        [Parameter(Mandatory)][hashtable]$Arguments
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($script:RequestTimeoutSeconds)
    $attempts = 0
    $lastState = 'notQueried'
    $lastError = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $attempts++
        try {
            $output = Invoke-McpTool -Session $Session -ToolName 'xray_callers' -Arguments $Arguments
            $lastState = Get-IncrementalState -Output $output
            if ($lastState -eq $ExpectedState) {
                return [ordered]@{ attempts = $attempts; state = $lastState }
            }
        }
        catch {
            $lastError = $_.Exception.Message
        }
        [Threading.Tasks.Task]::Delay(25).GetAwaiter().GetResult()
    }
    throw "Timed out waiting for incremental state '$ExpectedState'; lastState=$lastState; lastError=$lastError"
}

function Measure-IncrementalLifecycle {
    param(
        [Parameter(Mandatory)][object]$Session,
        [Parameter(Mandatory)][string]$CorpusRoot,
        [Parameter(Mandatory)][object]$Corpus,
        [Parameter(Mandatory)][int]$SampleCount
    )

    $probe = $Corpus.incremental
    $path = Join-Path $CorpusRoot $probe.relativePath
    $baseline = [IO.File]::ReadAllText($path, [Text.UTF8Encoding]::new($false, $true))
    if ((Get-Sha256 -Path $path) -ne $probe.baselineSha256) {
        throw 'Incremental probe baseline hash does not match the corpus manifest.'
    }
    if (@([regex]::Matches($baseline, [regex]::Escape($probe.marker))).Count -ne 1) {
        throw 'Incremental probe marker must occur exactly once.'
    }
    $modified = $baseline.Replace($probe.marker, $probe.addedDeclaration)
    $arguments = @{
        method = @($probe.caller)
        class = $probe.class
        direction = 'down'
        depth = 1
    }
    $initial = Invoke-McpTool -Session $Session -ToolName 'xray_callers' -Arguments $arguments
    if ((Get-IncrementalState -Output $initial) -ne 'exact') {
        throw 'Incremental probe did not start in the exact state.'
    }

    $addSamples = [System.Collections.Generic.List[double]]::new()
    $removeSamples = [System.Collections.Generic.List[double]]::new()
    $addAttempts = [System.Collections.Generic.List[double]]::new()
    $removeAttempts = [System.Collections.Generic.List[double]]::new()
    try {
        for ($iteration = 1; $iteration -le $SampleCount; $iteration++) {
            $stopwatch = [Diagnostics.Stopwatch]::StartNew()
            Write-Utf8File -Path $path -Content $modified
            $added = Wait-IncrementalState -Session $Session -ExpectedState 'ambiguous' `
                -Arguments $arguments
            $stopwatch.Stop()
            $addSamples.Add($stopwatch.Elapsed.TotalMilliseconds)
            $addAttempts.Add([double]$added.attempts)

            $stopwatch.Restart()
            Write-Utf8File -Path $path -Content $baseline
            $removed = Wait-IncrementalState -Session $Session -ExpectedState 'exact' `
                -Arguments $arguments
            $stopwatch.Stop()
            $removeSamples.Add($stopwatch.Elapsed.TotalMilliseconds)
            $removeAttempts.Add([double]$removed.attempts)
        }
    }
    finally {
        Write-Utf8File -Path $path -Content $baseline
    }
    if ((Get-Sha256 -Path $path) -ne $probe.baselineSha256) {
        throw 'Incremental probe did not restore its original source bytes.'
    }

    return [ordered]@{
        sampleCount = $SampleCount
        addOverloadMs = Get-SampleSummary -Values @($addSamples)
        removeOverloadMs = Get-SampleSummary -Values @($removeSamples)
        addPollAttempts = Get-SampleSummary -Values @($addAttempts)
        removePollAttempts = Get-SampleSummary -Values @($removeAttempts)
        sourceRestored = $true
    }
}


function Measure-Corpus {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$CorpusRoot,
        [Parameter(Mandatory)][object]$Corpus,
        [Parameter(Mandatory)][int]$BuildCount,
        [Parameter(Mandatory)][int]$QueryCount,
        [Parameter(Mandatory)][int]$IncrementalCount
    )

    $session = $null
    try {
        $definitionBuilds = Measure-DefinitionBuildSet -BinaryPath $BinaryPath `
            -CorpusRoot $CorpusRoot -BuildCount $BuildCount
        $contentBuild = Invoke-RequiredProcess -FilePath $BinaryPath -ArgumentList @(
            'content-index', '--dir', $CorpusRoot, '--ext', 'cs'
        ) -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'content-index build'

        $session = if ($Corpus.name -eq 'synthetic-overloads') {
            Open-McpSession -BinaryPath $BinaryPath -Directory $CorpusRoot -Watch
        }
        else {
            Open-McpSession -BinaryPath $BinaryPath -Directory $CorpusRoot
        }
        $initialize = Send-McpRequest -Session $session -Method 'initialize' -Params @{
            protocolVersion = '2025-03-26'
            capabilities = @{}
            clientInfo = @{ name = 'xray-d20-scale-baseline'; version = '1.0' }
        }
        if ($initialize.result.protocolVersion -ne '2025-03-26') {
            throw 'MCP initialize returned an unexpected protocol version'
        }
        Send-McpNotification -Session $session -Method 'notifications/initialized'
        $queries = Measure-McpQuerySet -Session $session -Corpus $Corpus -Iterations $QueryCount
        $incremental = if ($Corpus.name -eq 'synthetic-overloads' -and $IncrementalCount -gt 0) {
            Measure-IncrementalLifecycle -Session $session -CorpusRoot $CorpusRoot `
                -Corpus $Corpus -SampleCount $IncrementalCount
        }
        else {
            $null
        }
        $steadyWorkingSetBytes = $session.Process.WorkingSet64
        $peakWorkingSetBytes = $session.Process.PeakWorkingSet64
        $shutdown = Close-McpSession -Session $session
        $session = $null
        if (-not $shutdown.passed) {
            throw "MCP shutdown failed: $($shutdown.details)"
        }

        return [ordered]@{
            name = $Corpus.name
            methodCount = $Corpus.methodCount
            sourceFileCount = $Corpus.sourceFileCount
            definitionBuilds = $definitionBuilds
            contentBuild = [ordered]@{
                durationMs = $contentBuild.durationMs
                peakWorkingSetBytes = $contentBuild.peakWorkingSetBytes
            }
            server = [ordered]@{
                steadyWorkingSetBytes = $steadyWorkingSetBytes
                peakWorkingSetBytes = $peakWorkingSetBytes
                shutdown = $shutdown.details
            }
            queries = $queries
            incremental = $incremental
        }
    }
    finally {
        if ($null -ne $session) {
            [void](Close-McpSession -Session $session)
        }
        [void](Invoke-CapturedProcess -FilePath $BinaryPath -ArgumentList @(
            'cleanup', '--dir', $CorpusRoot
        ) -ProcessTimeoutSeconds $TimeoutSeconds)
    }
}


if ($ValidationSelfTest) {
    Write-Output ((Invoke-ValidationSelfTest) | ConvertTo-Json -Compress)
    return
}


$startedUtc = [DateTime]::UtcNow
$generationStopwatch = [Diagnostics.Stopwatch]::StartNew()
$ownedOutput = $false
$manifest = $null
$manifestPath = $null
$fixtureManifestSha256 = $null
$overloads = $null
$namespaces = $null
$measurements = @()
$failure = $null
$artifactCleanupStatus = 'notStarted'
$releaseBuild = $null
$candidateBinary = $null
$candidateCommit = $null
$candidateBinarySha256 = $null
$rustcInfo = $null
$cargoInfo = $null
$machineInfo = $null

if ($ColdBuildCount -eq 0) {
    $ColdBuildCount = if ($BenchmarkProfile -eq 'Release') { 7 } else { 1 }
}
if ($WarmQueryCount -eq 0) {
    $WarmQueryCount = if ($BenchmarkProfile -eq 'Release') { 100 } else { 5 }
}
if ($IncrementalSampleCount -eq 0) {
    $IncrementalSampleCount = if ($BenchmarkProfile -eq 'Release') { 20 } else { 3 }
}

try {
    if (-not $GenerateOnly) {
        if ($BuildRelease -and -not [string]::IsNullOrWhiteSpace($Binary)) {
            throw 'Use either -Binary or -BuildRelease, not both.'
        }
        if (-not $BuildRelease -and [string]::IsNullOrWhiteSpace($Binary)) {
            throw 'Measurement mode requires -BuildRelease or -Binary <path>.'
        }
    }

    Initialize-OwnedDirectory -Path $OutputDirectory
    $ownedOutput = $true
    $overloadRoot = Join-Path $OutputDirectory 'synthetic-overloads'
    $namespaceRoot = Join-Path $OutputDirectory 'synthetic-namespaces'
    [IO.Directory]::CreateDirectory($overloadRoot) | Out-Null
    [IO.Directory]::CreateDirectory($namespaceRoot) | Out-Null

    $overloads = Initialize-OverloadCorpus -Root $overloadRoot -TargetMethods $MethodsPerCorpus `
        -ChunkSize $ClassesPerFile -CorpusSeed $Seed
    $namespaces = Initialize-NamespaceCorpus -Root $namespaceRoot -TargetMethods $MethodsPerCorpus `
        -ChunkSize $ClassesPerFile -CorpusSeed $Seed

    $allFiles = [System.Collections.Generic.List[object]]::new()
    foreach ($corpus in @($overloads, $namespaces)) {
        foreach ($file in $corpus.files) {
            $allFiles.Add([ordered]@{
                corpus = $corpus.name
                path = $file.path
                bytes = $file.bytes
                sha256 = $file.sha256
            })
        }
    }
    $canonicalLines = @($allFiles | Sort-Object `
        @{ Expression = { $_.corpus } }, @{ Expression = { $_.path } } | ForEach-Object {
        "$($_.corpus)/$($_.path)`t$($_.bytes)`t$($_.sha256)"
    })
    $canonicalManifest = ($canonicalLines -join "`n") + "`n"
    $canonicalManifestPath = Join-Path $OutputDirectory 'source-manifest.txt'
    Write-Utf8File -Path $canonicalManifestPath -Content $canonicalManifest
    $fixtureManifestSha256 = Get-Sha256 -Path $canonicalManifestPath

    $manifest = [ordered]@{
        schemaVersion = 1
        generatorVersion = $GeneratorVersion
        seed = "0x$($Seed.ToString('x'))"
        profile = $BenchmarkProfile
        methodsPerCorpus = $MethodsPerCorpus
        classesPerFile = $ClassesPerFile
        fixtureManifestSha256 = $fixtureManifestSha256
        corpora = @(
            [ordered]@{
                name = $overloads.name
                methodCount = $overloads.methodCount
                sourceFileCount = $overloads.sourceFileCount
                overloadGroupCounts = $overloads.overloadGroupCounts
                probeGroups = $overloads.probeGroups
                probes = $overloads.probes
                incremental = $overloads.incremental
            },
            [ordered]@{
                name = $namespaces.name
                methodCount = $namespaces.methodCount
                sourceFileCount = $namespaces.sourceFileCount
                namespaceCollisionCount = $namespaces.namespaceCollisionCount
                probes = $namespaces.probes
            }
        )
        files = @($allFiles | Sort-Object `
            @{ Expression = { $_.corpus } }, @{ Expression = { $_.path } })
    }
    Assert-ExpectedReleaseManifest -ActualManifest $manifest
    $manifestPath = Join-Path $OutputDirectory 'd20-corpus-manifest.json'
    Write-Utf8File -Path $manifestPath -Content (($manifest | ConvertTo-Json -Depth 20) + "`n")
    $generationStopwatch.Stop()

    if (-not $GenerateOnly) {
        foreach ($requiredCommand in @('git', 'rustc', 'cargo')) {
            if ($null -eq (Get-Command $requiredCommand -ErrorAction SilentlyContinue)) {
                throw "Required command '$requiredCommand' was not found."
            }
        }
        $candidateCommit = (Invoke-RequiredProcess -FilePath 'git' -ArgumentList @(
            'rev-parse', 'HEAD'
        ) -Description 'git rev-parse HEAD').stdout.Trim()
        $rustcInfo = (Invoke-RequiredProcess -FilePath 'rustc' -ArgumentList @(
            '--version', '--verbose'
        ) -Description 'rustc metadata').stdout.Trim()
        $cargoInfo = (Invoke-RequiredProcess -FilePath 'cargo' -ArgumentList @(
            '--version', '--verbose'
        ) -Description 'cargo metadata').stdout.Trim()

        if ($BuildRelease) {
            $releaseBuild = Invoke-RequiredProcess -FilePath 'cargo' -ArgumentList @(
                'build', '--release', '--locked'
            ) -ProcessTimeoutSeconds $BuildTimeoutSeconds -Description 'candidate release build'
            $candidateBinary = Join-Path $RepoRoot 'target/release/xray.exe'
        }
        else {
            $candidateBinary = (Resolve-Path $Binary).Path
        }
        if (-not (Test-Path $candidateBinary -PathType Leaf)) {
            throw "Candidate binary not found: $candidateBinary"
        }
        $candidateBinarySha256 = Get-Sha256 -Path $candidateBinary

        try {
            $cpu = Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1
            $computer = Get-CimInstance Win32_ComputerSystem -ErrorAction Stop
            $machineInfo = [ordered]@{
                cpuModel = $cpu.Name
                logicalProcessorCount = $computer.NumberOfLogicalProcessors
                totalPhysicalMemoryBytes = [uint64]$computer.TotalPhysicalMemory
            }
        }
        catch {
            $machineInfo = [ordered]@{
                cpuModel = $null
                logicalProcessorCount = [Environment]::ProcessorCount
                totalPhysicalMemoryBytes = $null
            }
        }

        $measurements = @(
            Measure-Corpus -BinaryPath $candidateBinary -CorpusRoot $overloadRoot `
                -Corpus $overloads -BuildCount $ColdBuildCount -QueryCount $WarmQueryCount `
                -IncrementalCount $IncrementalSampleCount
            Measure-Corpus -BinaryPath $candidateBinary -CorpusRoot $namespaceRoot `
                -Corpus $namespaces -BuildCount $ColdBuildCount -QueryCount $WarmQueryCount `
                -IncrementalCount 0
        )
    }
}
catch {
    $failure = $_
    if ($generationStopwatch.IsRunning) {
        $generationStopwatch.Stop()
    }
}
finally {
    if ($GenerateOnly) {
        $artifactCleanupStatus = 'keptGenerateOnly'
    }
    elseif ($KeepArtifacts) {
        $artifactCleanupStatus = 'kept'
    }
    elseif ($ownedOutput -and (Test-Path $OutputDirectory)) {
        try {
            $marker = Join-Path $OutputDirectory '.xray-d20-corpus'
            if (-not (Test-Path $marker -PathType Leaf)) {
                throw "Refusing to remove unowned directory without marker: $OutputDirectory"
            }
            Remove-Item -Recurse -Force $OutputDirectory -ErrorAction Stop
            if (Test-Path $OutputDirectory) {
                throw "Artifact directory still exists after removal: $OutputDirectory"
            }
            $artifactCleanupStatus = 'passed'
        }
        catch {
            $artifactCleanupStatus = "failed:$($_.Exception.Message)"
            if ($null -eq $failure) {
                $failure = $_
            }
        }
    }
    else {
        $artifactCleanupStatus = 'notRequired'
    }

    $querySamples = @($measurements | ForEach-Object {
        $_.queries | ForEach-Object { $_.durationMs.raw }
    })
    $candidateSamples = @($measurements | ForEach-Object {
        $_.queries | ForEach-Object { $_.observedCandidateCount.raw }
    })
    $incrementalAddSamples = @($measurements | ForEach-Object {
        if ($null -ne $_.incremental) { $_.incremental.addOverloadMs.raw }
    })
    $incrementalRemoveSamples = @($measurements | ForEach-Object {
        if ($null -ne $_.incremental) { $_.incremental.removeOverloadMs.raw }
    })
    $relativeArtifactPath = if ($null -ne $manifestPath) {
        $relative = [IO.Path]::GetRelativePath($RepoRoot, $manifestPath).Replace('\', '/')
        if ($relative.StartsWith('../')) { '<external>' } else { $relative }
    }
    else {
        $null
    }
    $cargoLockSha256 = if (Test-Path (Join-Path $RepoRoot 'Cargo.lock')) {
        Get-Sha256 -Path (Join-Path $RepoRoot 'Cargo.lock')
    }
    else {
        $null
    }

    $result = [ordered]@{
        schemaVersion = 1
        startedUtc = $startedUtc.ToString('o')
        endedUtc = [DateTime]::UtcNow.ToString('o')
        passed = ($null -eq $failure)
        status = if ($null -ne $failure) { 'failed' } elseif ($GenerateOnly) { 'generated' } else { 'baselineRecorded' }
        profile = $BenchmarkProfile
        generatorVersion = $GeneratorVersion
        seed = if ($null -ne $manifest) { $manifest.seed } else { "0x$($Seed.ToString('x'))" }
        methodsPerCorpus = $MethodsPerCorpus
        coldBuildCount = $ColdBuildCount
        warmQueryCount = $WarmQueryCount
        incrementalSampleCount = $IncrementalSampleCount
        fixtureManifestSha256 = $fixtureManifestSha256
        generationDurationMs = $generationStopwatch.Elapsed.TotalMilliseconds
        corpusManifest = $relativeArtifactPath
        corpusSummaries = if ($null -ne $manifest) { $manifest.corpora } else { @() }
        candidate = [ordered]@{
            commit = $candidateCommit
            binarySha256 = $candidateBinarySha256
            releaseBuildMs = if ($null -ne $releaseBuild) { $releaseBuild.durationMs } else { $null }
            cargoLockSha256 = $cargoLockSha256
        }
        environment = [ordered]@{
            os = [Runtime.InteropServices.RuntimeInformation]::OSDescription
            osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
            processArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
            rustc = $rustcInfo
            cargo = $cargoInfo
            machine = $machineInfo
            defenderAndIndexingPolicy = 'uncontrolled'
        }
        measurements = @($measurements)
        aggregate = [ordered]@{
            queryDurationMs = Get-SampleSummary -Values @($querySamples | ForEach-Object { [double]$_ })
            observedCandidateCount = Get-SampleSummary -Values @($candidateSamples | ForEach-Object { [double]$_ })
            incrementalAddOverloadMs = Get-SampleSummary -Values @($incrementalAddSamples | ForEach-Object { [double]$_ })
            incrementalRemoveOverloadMs = Get-SampleSummary -Values @($incrementalRemoveSamples | ForEach-Object { [double]$_ })
        }
        budgetEvaluation = [ordered]@{
            status = if ($GenerateOnly) { 'notApplicable' } else { 'baselineRecorded' }
            reason = if ($GenerateOnly) {
                'Generation-only run has no performance samples.'
            }
            else {
                'No comparable prior D20 scale artifact was supplied; raw baseline recorded without performance claims.'
            }
        }
        cleanup = [ordered]@{
            status = $artifactCleanupStatus
            artifactsKept = [bool]($GenerateOnly -or $KeepArtifacts)
        }
        error = if ($null -ne $failure) { $failure.Exception.Message } else { $null }
    }

    $resultParent = Split-Path -Parent $ResultPath
    if (-not [string]::IsNullOrWhiteSpace($resultParent)) {
        [IO.Directory]::CreateDirectory($resultParent) | Out-Null
    }
    Write-Utf8File -Path $ResultPath -Content (($result | ConvertTo-Json -Depth 100) + "`n")
}

if ($null -ne $failure) {
    Write-Error "D20 scale baseline failed: $($failure.Exception.Message). Result: $ResultPath"
    exit 1
}
Write-Output ($result | ConvertTo-Json -Compress -Depth 100)
