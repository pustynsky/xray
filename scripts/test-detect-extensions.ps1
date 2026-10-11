#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Regression tests for extension detection and auto-selection in setup-xray.ps1.

.DESCRIPTION
    Loads the production extension detection and auto-selection policy via AST extraction
    (the script has top-level imperative code, so plain dot-source is not
    possible) and runs it against fixture trees that exercise the
    invariants the function must preserve:

      * Skip-set membership pruned BEFORE descending (prune-at-boundary,
        not prune-after-recurse). Verified via case-insensitive match
        on directory leaves.
      * Reparse points (junctions / symlinks / mount points) NOT followed.
        Verified by creating a junction to a "poison" tree and asserting
        none of the poison files appear in the result.
      * Hidden files ARE counted (matches the previous Get-ChildItem -Force
        behavior).
      * Files with no extension OR a bare-dot extension are NOT tallied.
      * Unknown extensions are NOT tallied.
      * Known extensions are tallied with the correct count.

    The test does NOT measure performance; perf characteristics are
    covered by the dev-time harness in target/tmp/bench-scan.ps1 (not
    committed, recreated on demand).

    Exits 0 on all-pass, 1 on any failure.
#>

$ErrorActionPreference = 'Stop'
$Global:_failures = 0
$Global:_passes = 0

function Assert-Equal {
    param([Parameter(Mandatory)] $Actual, [Parameter(Mandatory)] $Expected, [Parameter(Mandatory)] [string]$Label)
    if ($Expected -is [System.Collections.IDictionary] -or $Expected -is [hashtable]) {
        $a = ($Actual.GetEnumerator() | Sort-Object Key | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ','
        $e = ($Expected.GetEnumerator() | Sort-Object Key | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ','
        if ($a -eq $e) {
            Write-Host "PASS  $Label" -ForegroundColor Green
            $Global:_passes++
        }
        else {
            Write-Host "FAIL  $Label" -ForegroundColor Red
            Write-Host "  expected: $e" -ForegroundColor Red
            Write-Host "  actual:   $a" -ForegroundColor Red
            $Global:_failures++
        }
    }
    else {
        if ($Actual -eq $Expected) {
            Write-Host "PASS  $Label" -ForegroundColor Green
            $Global:_passes++
        }
        else {
            Write-Host "FAIL  $Label" -ForegroundColor Red
            Write-Host "  expected: $Expected" -ForegroundColor Red
            Write-Host "  actual:   $Actual" -ForegroundColor Red
            $Global:_failures++
        }
    }
}

function New-TempDir {
    $d = Join-Path ([IO.Path]::GetTempPath()) ('xray-detect-test-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $d -Force | Out-Null
    return $d
}

function Remove-TempDir {
    param([string]$Path)
    if (Test-Path $Path) {
        # Junctions: remove via Remove-Item -Force, NOT recursive — recursive
        # delete on a junction can wipe the target. Walk top-level entries
        # first and unlink junctions before recursing.
        Get-ChildItem $Path -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                $attrs = [IO.File]::GetAttributes($_.FullName)
                if ($_.PSIsContainer -and ($attrs -band [IO.FileAttributes]::ReparsePoint)) {
                    # Remove the junction without following it.
                    [IO.Directory]::Delete($_.FullName, $false)
                }
            }
            catch { }
        }
        Remove-Item -Path $Path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function New-InstallerFixture {
    $root = New-TempDir
    $repo = Join-Path $root 'repo'
    $install = Join-Path $root 'install'
    New-Item -ItemType Directory -Path $repo, $install -Force | Out-Null
    New-Item -ItemType File -Path (Join-Path $install 'xray.exe') -Force | Out-Null
    & git -C $repo init --quiet
    if ($LASTEXITCODE -ne 0) { throw "git init failed for installer fixture: $repo" }

    return [PSCustomObject]@{
        Root    = $root
        Repo    = $repo
        Install = $install
    }
}

function Add-InstallerSelectionFiles {
    param([Parameter(Mandatory)] [string]$Repo)

    1..20 | ForEach-Object {
        "export const value$_ = $_;" | Set-Content (Join-Path $Repo "file$_.ts") -Encoding UTF8
    }
    '<h1>View</h1>' | Set-Content (Join-Path $Repo 'Index.cshtml') -Encoding UTF8
    '<Project />' | Set-Content (Join-Path $Repo 'App.csproj') -Encoding UTF8
    '<Project />' | Set-Content (Join-Path $Repo 'Directory.Build.props') -Encoding UTF8
    '<Project />' | Set-Content (Join-Path $Repo 'Directory.Build.targets') -Encoding UTF8
    'unknown' | Set-Content (Join-Path $Repo 'sample.rareunknown') -Encoding UTF8
}

function Invoke-InstallerFixture {
    param(
        [Parameter(Mandatory)] [string]$ScriptPath,
        [Parameter(Mandatory)] [string]$Repo,
        [Parameter(Mandatory)] [string]$Install,
        [switch]$Force,
        [switch]$AcceptSuggestion,
        [string[]]$Responses,
        [switch]$AllowFailure,
        [string]$Extensions
    )

    $pwshPath = (Get-Process -Id $PID).Path
    $parameters = @{
        RepoPath = $Repo
        InstallDir = $Install
        SkipDownload = $true
        EnableVSCode = $true
    }
    if ($Force) { $parameters.Force = $true }
    if ($Extensions) { $parameters.Extensions = $Extensions }
    $payload = @{ ScriptPath = $ScriptPath; Parameters = $parameters } | ConvertTo-Json -Compress
    $payloadBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $launchCode = @'
$ErrorActionPreference = 'Stop'
$invocation = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD__')) | ConvertFrom-Json
$parameters = @{}
foreach ($property in $invocation.Parameters.PSObject.Properties) { $parameters[$property.Name] = $property.Value }
& $invocation.ScriptPath @parameters
exit $LASTEXITCODE
'@.Replace('__PAYLOAD__', $payloadBase64)
    $startInfo = New-Object Diagnostics.ProcessStartInfo
    $startInfo.FileName = $pwshPath
    $startInfo.Arguments = '-NoLogo -NoProfile -OutputFormat Text -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($launchCode))
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $startInfo
    $started = $false
    try {
        $started = $process.Start()
        if (-not $started) { throw 'Could not start installer fixture process' }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if ($AcceptSuggestion) { $process.StandardInput.WriteLine('') }
        foreach ($response in $Responses) { $process.StandardInput.WriteLine($response) }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit(30000)) {
            throw "Installer fixture timed out after 30 seconds: $Repo (responses: $($Responses -join ', '))"
        }
        $exitCode = $process.ExitCode
        $output = @($stdoutTask.GetAwaiter().GetResult(), $stderrTask.GetAwaiter().GetResult())
    }
    finally {
        if ($started -and -not $process.HasExited) {
            $process.Kill()
            $null = $process.WaitForExit(5000)
        }
        $process.Dispose()
    }
    if ($exitCode -ne 0 -and -not $AllowFailure) {
        throw "setup-xray.ps1 exited $exitCode`n$($output -join "`n")"
    }

    return [PSCustomObject]@{
        ExitCode = $exitCode
        Output = $output
        Config = if ($exitCode -eq 0) { Get-Content -Path (Join-Path $Repo '.vscode/mcp.json') -Raw | ConvertFrom-Json } else { $null }
    }
}

function Get-ConfiguredExtensionState {
    param([Parameter(Mandatory)] $Config)

    $xrayArgs = @($Config.servers.xray.args)
    $extIndexes = @(for ($index = 0; $index -lt $xrayArgs.Count; $index++) {
            if ($xrayArgs[$index] -eq '--ext') { $index }
        })
    $extensionList = if ($extIndexes.Count -eq 1) { $xrayArgs[$extIndexes[0] + 1] } else { $null }

    return [PSCustomObject]@{
        ExtFlagCount  = $extIndexes.Count
        ExtensionList = $extensionList
    }
}

# Load extension detection and auto-selection policy from setup-xray.ps1 via AST.
$scriptPath = Join-Path (Split-Path -Parent $PSCommandPath) 'setup-xray.ps1'
if (-not (Test-Path $scriptPath)) {
    Write-Error "setup-xray.ps1 not found next to this test: $scriptPath"
    exit 1
}
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors -and $parseErrors.Count -gt 0) {
    Write-Error "Parse errors in setup-xray.ps1: $($parseErrors -join '; ')"
    exit 1
}
foreach ($functionName in @('Get-DetectedExtensions', 'Get-AutoSelectedExtensions', 'Normalize-ExtensionList', 'Read-ExtensionSelection')) {
    $functionAst = $ast.Find({
            param($node)
            $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $functionName
        }, $true)
    if (-not $functionAst) {
        Write-Error "$functionName not found in $scriptPath"
        exit 1
    }
    . ([scriptblock]::Create($functionAst.Extent.Text))
}

foreach ($variableName in @('KnownCodeExtensions', 'AlwaysIncludeIfDetected')) {
    $assignmentAst = $ast.Find({
            param($node)
            $node -is [Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left.Extent.Text -eq ('$' + $variableName)
        }, $true)
    if (-not $assignmentAst) {
        Write-Error "$variableName assignment not found in $scriptPath"
        exit 1
    }
    . ([scriptblock]::Create($assignmentAst.Extent.Text))
}

# Fixture knowns and skips for scanner invariants below.
$KnownExt = @{ 'cs' = 'C#'; 'rs' = 'Rust'; 'md' = 'MD'; 'ps1' = 'PS' }
$SkipDirs = @('.git', 'node_modules', 'target', 'bin', 'obj')

# ---------------------------------------------------------------
# Test 1: basic count of known extensions, ignoring unknown ones.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'src') -Force | Out-Null
    'fn main() {}' | Set-Content (Join-Path $root 'src\main.rs') -Encoding UTF8
    'pub mod x;' | Set-Content (Join-Path $root 'src\lib.rs') -Encoding UTF8
    '# Title' | Set-Content (Join-Path $root 'README.md') -Encoding UTF8
    '<svg/>' | Set-Content (Join-Path $root 'logo.svg') -Encoding UTF8     # unknown ext
    'no ext at all' | Set-Content (Join-Path $root 'LICENSE') -Encoding UTF8   # no ext
    'bare dot' | Set-Content (Join-Path $root 'oddfile.') -Encoding UTF8        # bare dot
    '.dotfile content' | Set-Content (Join-Path $root '.dotfile') -Encoding UTF8 # dot prefix, no ext

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 2; 'md' = 1 } -Label 'T1 known/unknown/no-ext basic counting'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 2: prune-at-boundary — case-insensitive skip-set match on
#         dir leaf names. Files inside skipped dirs MUST NOT be tallied.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'node_modules\react') -Force | Out-Null
    'real x' | Set-Content (Join-Path $root 'node_modules\react\index.md') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'NODE_MODULES\angular') -Force | Out-Null
    'should be skipped (case)' | Set-Content (Join-Path $root 'NODE_MODULES\angular\poison.md') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'target\release') -Force | Out-Null
    'rust build artifact' | Set-Content (Join-Path $root 'target\release\poison.rs') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'kept') -Force | Out-Null
    'kept' | Set-Content (Join-Path $root 'kept\real.rs') -Encoding UTF8

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T2 prune skipped dirs (case-insensitive)'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 3: hidden files MUST be counted (matches old -Force).
# ---------------------------------------------------------------
$root = New-TempDir
try {
    'visible' | Set-Content (Join-Path $root 'visible.md') -Encoding UTF8
    $hiddenFile = Join-Path $root 'hidden.md'
    'hidden' | Set-Content $hiddenFile -Encoding UTF8
    [IO.File]::SetAttributes($hiddenFile, [IO.FileAttributes]::Hidden)

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'md' = 2 } -Label 'T3 hidden files are counted'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 4: reparse-point (junction) MUST NOT be followed.
#         Windows-only — Linux/macOS skip (test-suite still passes).
# ---------------------------------------------------------------
if ($PSVersionTable.Platform -eq 'Win32NT' -or $env:OS -eq 'Windows_NT') {
    $root = New-TempDir
    $poisonRoot = New-TempDir
    try {
        'real' | Set-Content (Join-Path $root 'real.rs') -Encoding UTF8
        'poison' | Set-Content (Join-Path $poisonRoot 'poison.rs') -Encoding UTF8
        'poison2' | Set-Content (Join-Path $poisonRoot 'poison.md') -Encoding UTF8

        # Create a junction (NTFS, no admin needed) from $root\linked → $poisonRoot.
        # Use cmd's mklink /J because PowerShell New-Item -ItemType Junction
        # is not available on Windows PowerShell 5.1 in all configurations.
        $junction = Join-Path $root 'linked'
        & cmd.exe /c mklink /J "$junction" "$poisonRoot" 2>&1 | Out-Null
        if (-not (Test-Path $junction)) {
            Write-Host "SKIP  T4 reparse-point skip (could not create junction; need NTFS)" -ForegroundColor DarkYellow
        }
        else {
            $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
            Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T4 reparse-point (junction) not followed'
        }
    }
    finally {
        Remove-TempDir $root
        Remove-TempDir $poisonRoot
    }
}
else {
    Write-Host "SKIP  T4 reparse-point skip (non-Windows; test would need symlink permissions)" -ForegroundColor DarkYellow
}

# ---------------------------------------------------------------
# Test 5: deeply nested layout (sanity that DFS handles depth).
# ---------------------------------------------------------------
$root = New-TempDir
try {
    $deep = $root
    for ($i = 0; $i -lt 8; $i++) {
        $deep = Join-Path $deep ('lvl' + $i)
        New-Item -ItemType Directory -Path $deep -Force | Out-Null
        "fn x() {}" | Set-Content (Join-Path $deep ("file$i.rs")) -Encoding UTF8
    }
    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 8 } -Label 'T5 deeply nested DFS counts every level'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 6: empty repo.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    if ($r.Count -eq 0) {
        Write-Host "PASS  T6 empty repo returns empty hashtable" -ForegroundColor Green
        $Global:_passes++
    }
    else {
        Write-Host "FAIL  T6 empty repo returned $($r.Count) entries" -ForegroundColor Red
        $Global:_failures++
    }
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 7: dir whose name STARTS with a skip-dir name (e.g. 'target-old')
#         must NOT be skipped — the skip-set is leaf-equality, not prefix.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    New-Item -ItemType Directory -Path (Join-Path $root 'target-old') -Force | Out-Null
    'kept' | Set-Content (Join-Path $root 'target-old\real.rs') -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $root 'target') -Force | Out-Null
    'pruned' | Set-Content (Join-Path $root 'target\poison.rs') -Encoding UTF8

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownExt -SkipDirectoryNames $SkipDirs
    Assert-Equal -Actual $r -Expected @{ 'rs' = 1 } -Label 'T7 prefix-of-skip-name not falsely skipped'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 8: a rare structural extension bypasses the count threshold.
# ---------------------------------------------------------------
$root = New-TempDir
try {
    1..20 | ForEach-Object {
        "export const value$_ = $_;" | Set-Content (Join-Path $root "file$_.ts") -Encoding UTF8
    }
    '<h1>View</h1>' | Set-Content (Join-Path $root 'Index.cshtml') -Encoding UTF8

    $r = Get-DetectedExtensions -RootPath $root -KnownExtensions $KnownCodeExtensions -SkipDirectoryNames $SkipDirs
    $threshold = [Math]::Max(5, [Math]::Ceiling((($r.Values | Measure-Object -Sum).Sum) * 0.005))
    $selection = Get-AutoSelectedExtensions -ExtensionCounts $r -Threshold $threshold -AlwaysIncludeIfDetected $AlwaysIncludeIfDetected

    Assert-Equal -Actual ($selection.ThresholdSelected -join ',') -Expected 'ts' -Label 'T8 threshold-selected source extension'
    Assert-Equal -Actual ($selection.StructuralSelected -join ',') -Expected 'cshtml' -Label 'T8 rare cshtml structural exception'
    Assert-Equal -Actual ($selection.All -join ',') -Expected 'cshtml,ts' -Label 'T8 deterministic complete suggestion'
}
finally { Remove-TempDir $root }

# ---------------------------------------------------------------
# Test 9: project/build structural files are retained below threshold.
# ---------------------------------------------------------------
$counts = @{ 'ts' = 20; 'csproj' = 1; 'props' = 1; 'targets' = 1 }
$selection = Get-AutoSelectedExtensions -ExtensionCounts $counts -Threshold 5 -AlwaysIncludeIfDetected $AlwaysIncludeIfDetected
Assert-Equal -Actual ($selection.StructuralSelected -join ',') -Expected 'csproj,props,targets' -Label 'T9 project and build structural exceptions'
Assert-Equal -Actual ($selection.All -join ',') -Expected 'csproj,props,targets,ts' -Label 'T9 sorted complete suggestion'

# ---------------------------------------------------------------
# Test 9b: structural extensions at the threshold stay in the threshold bucket.
# ---------------------------------------------------------------
$counts = @{ 'cs' = 500; 'xml' = 5; 'csproj' = 2 }
$selection = Get-AutoSelectedExtensions -ExtensionCounts $counts -Threshold 5 -AlwaysIncludeIfDetected $AlwaysIncludeIfDetected
Assert-Equal -Actual ($selection.ThresholdSelected -join ',') -Expected 'cs,xml' -Label 'T9b at-threshold structural extension uses threshold bucket'
Assert-Equal -Actual ($selection.StructuralSelected -join ',') -Expected 'csproj' -Label 'T9b structural exception bucket stays below threshold'
Assert-Equal -Actual ($selection.All -join ',') -Expected 'cs,csproj,xml' -Label 'T9b boundary selection remains sorted and deduplicated'

# ---------------------------------------------------------------
# Test 10: unknown and absent structural extensions are not selected.
# ---------------------------------------------------------------
$counts = @{ 'ts' = 20; 'rareunknown' = 1; 'cshtml' = 1 }
$selection = Get-AutoSelectedExtensions -ExtensionCounts $counts -Threshold 5 -AlwaysIncludeIfDetected $AlwaysIncludeIfDetected
Assert-Equal -Actual ($selection.StructuralSelected -join ',') -Expected 'cshtml' -Label 'T10 absent structural extensions are not added'
Assert-Equal -Actual ($selection.All -join ',') -Expected 'cshtml,ts' -Label 'T10 rare unknown extension remains excluded'

# ---------------------------------------------------------------
# Test 11: every structural policy extension is detectable in production.
# ---------------------------------------------------------------
$missingKnownExtensions = @($AlwaysIncludeIfDetected | Where-Object { -not $KnownCodeExtensions.ContainsKey($_) })
Assert-Equal -Actual ($missingKnownExtensions -join ',') -Expected '' -Label 'T11 structural policy is covered by scanner allowlist'
Assert-Equal -Actual (($AlwaysIncludeIfDetected | Sort-Object -Unique) -join ',') `
    -Expected 'appxmanifest,config,cshtml,csproj,fsproj,manifestxml,md,props,razor,resx,sln,targets,vbproj,vcxproj,vsixmanifest,xaml,xml' `
    -Label 'T11 structural policy is deduplicated and complete'

# ---------------------------------------------------------------
# Test 12: real -Force setup writes the complete structural suggestion once.
# ---------------------------------------------------------------
$fixture = New-InstallerFixture
try {
    Add-InstallerSelectionFiles -Repo $fixture.Repo
    $forceResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force
    $forceState = Get-ConfiguredExtensionState -Config $forceResult.Config
    $forceOutput = $forceResult.Output -join "`n"

    Assert-Equal -Actual $forceState.ExtensionList -Expected 'cshtml,csproj,props,targets,ts' -Label 'T12 Force accepts complete structural suggestion'
    Assert-Equal -Actual ([bool]($forceOutput -match 'Threshold-selected extensions \(>= 5 files\): ts')) -Expected $true -Label 'T12 output identifies threshold-selected extensions'
    Assert-Equal -Actual ([bool]($forceOutput -match 'Structural exceptions \(detected below threshold\): cshtml,csproj,props,targets')) -Expected $true -Label 'T12 output identifies structural exceptions'

    $repeatResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force
    $repeatState = Get-ConfiguredExtensionState -Config $repeatResult.Config
    $extensionValues = @($repeatState.ExtensionList -split ',')
    Assert-Equal -Actual $repeatState.ExtFlagCount -Expected 1 -Label 'T12 repeated setup writes one --ext argument'
    Assert-Equal -Actual (($extensionValues | Sort-Object -Unique).Count) -Expected $extensionValues.Count -Label 'T12 repeated setup does not duplicate extension values'

    $explicitResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force -Extensions 'ts'
    $explicitState = Get-ConfiguredExtensionState -Config $explicitResult.Config
    Assert-Equal -Actual $explicitState.ExtensionList -Expected 'ts' -Label 'T12 explicit Extensions remains authoritative'

    $explicitAnswerResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force -Extensions 'y,n'
    $explicitAnswerState = Get-ConfiguredExtensionState -Config $explicitAnswerResult.Config
    Assert-Equal -Actual $explicitAnswerState.ExtensionList -Expected 'n,y' -Label 'T12 explicit Extensions does not interpret yes/no answers'
}
finally { Remove-TempDir $fixture.Root }

# ---------------------------------------------------------------
# Test 13: interactive Enter and -Force accept the same suggestion.
# ---------------------------------------------------------------
$fixture = New-InstallerFixture
try {
    Add-InstallerSelectionFiles -Repo $fixture.Repo
    $interactiveResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -AcceptSuggestion
    $interactiveState = Get-ConfiguredExtensionState -Config $interactiveResult.Config
    $parityForceResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force
    $parityForceState = Get-ConfiguredExtensionState -Config $parityForceResult.Config

    Assert-Equal -Actual $interactiveState.ExtensionList -Expected 'cshtml,csproj,props,targets,ts' -Label 'T13 Enter accepts complete structural suggestion'
    Assert-Equal -Actual $parityForceState.ExtensionList -Expected 'cshtml,csproj,props,targets,ts' -Label 'T13 Force accepts complete structural suggestion'
    Assert-Equal -Actual $interactiveState.ExtensionList -Expected $parityForceState.ExtensionList -Label 'T13 Enter and Force produce identical extensions'
}
finally { Remove-TempDir $fixture.Root }

# ---------------------------------------------------------------
# Test 14: installer output names empty selection buckets explicitly.
# ---------------------------------------------------------------
$fixture = New-InstallerFixture
try {
    '<h1>View</h1>' | Set-Content (Join-Path $fixture.Repo 'Index.cshtml') -Encoding UTF8
    $structuralOnlyResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force
    $structuralOnlyOutput = $structuralOnlyResult.Output -join "`n"
    Assert-Equal -Actual ([bool]($structuralOnlyOutput -match 'Threshold-selected extensions \(>= 5 files\): \(none\)')) -Expected $true -Label 'T14 empty threshold bucket is explicit'
    Assert-Equal -Actual ([bool]($structuralOnlyOutput -match 'Structural exceptions \(detected below threshold\): cshtml')) -Expected $true -Label 'T14 structural-only bucket remains visible'

    Remove-Item -Path (Join-Path $fixture.Repo 'Index.cshtml') -Force
    1..5 | ForEach-Object {
        "export const value$_ = $_;" | Set-Content (Join-Path $fixture.Repo "file$_.ts") -Encoding UTF8
    }
    $thresholdOnlyResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Force
    $thresholdOnlyOutput = $thresholdOnlyResult.Output -join "`n"
    Assert-Equal -Actual ([bool]($thresholdOnlyOutput -match 'Threshold-selected extensions \(>= 5 files\): ts')) -Expected $true -Label 'T14 threshold-only bucket remains visible'
    Assert-Equal -Actual ([bool]($thresholdOnlyOutput -match 'Structural exceptions \(detected below threshold\): \(none\)')) -Expected $true -Label 'T14 empty structural bucket is explicit'
}
finally { Remove-TempDir $fixture.Root }

$dialogCases = @(
    @{ Label = 'Enter'; Answers = @(''); Expected = 'cs,md'; ManualPrompts = 0 },
    @{ Label = 'whitespace Enter'; Answers = @('   '); Expected = 'cs,md'; ManualPrompts = 0 },
    @{ Label = 'trimmed y'; Answers = @(' Y '); Expected = 'cs,md'; ManualPrompts = 0 },
    @{ Label = 'yes'; Answers = @('YES'); Expected = 'cs,md'; ManualPrompts = 0 },
    @{ Label = 'n then custom'; Answers = @('n', ' .CS, sql,MD,cs '); Expected = 'cs,md,sql'; ManualPrompts = 1 },
    @{ Label = 'no then single-character extensions'; Answers = @(' NO ', 'r,c,h'); Expected = 'c,h,r'; ManualPrompts = 1 },
    @{ Label = 'invalid confirmation'; Answers = @('cs,sql', 'maybe', 'yes'); Expected = 'cs,md'; ManualPrompts = 0 },
    @{ Label = 'invalid manual entries'; Answers = @('n', '', ' ', 'Y', ' N ', 'yes', 'NO', ',.,', '.y,.n'); Expected = 'n,y'; ManualPrompts = 8 },
    @{ Label = 'explicit .y'; Answers = @('no', '.y'); Expected = 'y'; ManualPrompts = 1 },
    @{ Label = 'explicit .n'; Answers = @('no', '.n'); Expected = 'n'; ManualPrompts = 1 }
)
foreach ($dialogCase in $dialogCases) {
    & {
        param($Case)
        $answers = [Collections.Generic.Queue[string]]::new()
        $prompts = [Collections.Generic.List[string]]::new()
        foreach ($answer in $Case.Answers) { $answers.Enqueue($answer) }
        function Read-Host {
            param([string]$Prompt)
            $prompts.Add($Prompt)
            if ($answers.Count -eq 0) { throw "Unexpected prompt: $Prompt" }
            return $answers.Dequeue()
        }

        $actual = Read-ExtensionSelection -Suggested 'cs,md'
        Assert-Equal -Actual $actual -Expected $Case.Expected -Label "T15 $($Case.Label) selects expected list"
        Assert-Equal -Actual $answers.Count -Expected 0 -Label "T15 $($Case.Label) consumes expected answers"
        $manualPrompts = @($prompts | Where-Object { $_ -like 'Enter extensions*' })
        Assert-Equal -Actual $manualPrompts.Count -Expected $Case.ManualPrompts -Label "T15 $($Case.Label) uses separate manual prompt"
    } $dialogCase
}

$installerDialogCases = @(
    @{ Label = 'y'; Answers = @('y'); Expected = 'cshtml,csproj,props,targets,ts' },
    @{ Label = 'yes'; Answers = @('yes'); Expected = 'cshtml,csproj,props,targets,ts' },
    @{ Label = 'n then custom'; Answers = @('n', ' C,.h, r,c '); Expected = 'c,h,r' },
    @{ Label = 'invalid answers then explicit dots'; Answers = @('maybe', 'NO', '', 'y', 'n', ',.,', '.y,.n'); Expected = 'n,y' }
)
foreach ($dialogCase in $installerDialogCases) {
    $fixture = New-InstallerFixture
    try {
        Add-InstallerSelectionFiles -Repo $fixture.Repo
        $dialogResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Responses $dialogCase.Answers
        $dialogState = Get-ConfiguredExtensionState -Config $dialogResult.Config
        Assert-Equal -Actual $dialogState.ExtensionList -Expected $dialogCase.Expected -Label "T16 $($dialogCase.Label) writes selected extensions"
        Assert-Equal -Actual $dialogState.ExtFlagCount -Expected 1 -Label "T16 $($dialogCase.Label) writes one --ext argument"
    }
    finally { Remove-TempDir $fixture.Root }
}

$incompleteDialogCases = @(
    @{ Label = 'n then EOF'; Answers = @('n'); Message = 'No valid extension list received' },
    @{ Label = 'n then invalid input and EOF'; Answers = @('n', 'y'); Message = 'No valid extension list received' },
    @{ Label = 'exhausted confirmation attempts'; Answers = @('maybe') * 10; Message = 'No valid confirmation received' }
)
foreach ($dialogCase in $incompleteDialogCases) {
    $fixture = New-InstallerFixture
    try {
        Add-InstallerSelectionFiles -Repo $fixture.Repo
        $dialogResult = Invoke-InstallerFixture -ScriptPath $scriptPath -Repo $fixture.Repo -Install $fixture.Install -Responses $dialogCase.Answers -AllowFailure
        $diagnostic = ((($dialogResult.Output -join ' ') -replace '\x1b\[[0-9;]*m', '') -replace '[\r\n|]+', ' ') -replace '\s+', ' '
        Assert-Equal -Actual $dialogResult.ExitCode -Expected 1 -Label "T17 $($dialogCase.Label) exits with error"
        Assert-Equal -Actual $diagnostic.Contains($dialogCase.Message) -Expected $true -Label "T17 $($dialogCase.Label) explains failure"
        Assert-Equal -Actual (Test-Path (Join-Path $fixture.Repo '.vscode/mcp.json')) -Expected $false -Label "T17 $($dialogCase.Label) leaves VS Code unconfigured"
        Assert-Equal -Actual (Test-Path (Join-Path $fixture.Repo '.github\mcp.json')) -Expected $false -Label "T17 $($dialogCase.Label) leaves CLI unconfigured"
    }
    finally { Remove-TempDir $fixture.Root }
}

# ---------------------------------------------------------------
Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Cyan
Write-Host "Passed: $Global:_passes" -ForegroundColor Green
if ($Global:_failures -gt 0) {
    Write-Host "Failed: $Global:_failures" -ForegroundColor Red
    exit 1
}
Write-Host "Failed: 0" -ForegroundColor Green
exit 0
