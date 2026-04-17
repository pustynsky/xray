#!/usr/bin/env pwsh
param(
    [string]$TestDir = ".",
    [string]$TestExt = "rs",
    [string]$Binary = "cargo run --"
)

$ErrorActionPreference = "Stop"
$passed = 0
$failed = 0
$total = 0

function Run-Test {
    param([string]$Name, [string]$Command, [int]$ExpectedExit = 0)

    $script:total++
    Write-Host -NoNewline "  $Name ... "

    $ErrorActionPreference = "Continue"
    $result = Invoke-Expression "$Command 2>&1" | Out-String
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = "Stop"

    if ($exitCode -ne $ExpectedExit) {
        Write-Host "FAILED (exit=$exitCode, expected=$ExpectedExit)" -ForegroundColor Red
        $script:failed++
        return
    }

    Write-Host "OK" -ForegroundColor Green
    $script:passed++
}

Write-Host "`n=== E2E Tests (dir=$TestDir, ext=$TestExt) ===`n"

# Build first
Write-Host "Building..."
$ErrorActionPreference = "Continue"
& cargo build 2>&1 | Out-Null
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed!" -ForegroundColor Red; exit 1 }

# After successful build, use the compiled binary directly instead of "cargo run --"
# to avoid ~0.5-1s cargo freshness check overhead per test invocation.
if ($Binary -eq "cargo run --") {
    $Binary = ".\target\debug\xray.exe"
    Write-Host "Using direct binary: $Binary"
}

# === SEQUENTIAL TESTS (share index state in %LOCALAPPDATA%/xray/) ===

Write-Host "`n=== Sequential CLI tests ===`n"

# T01-T05: find (REMOVED — xray find tool was removed in audit batch 2026-03-14)

# T06-T09: index + fast
Run-Test "T06 index-build"         "$Binary index -d $TestDir"
Run-Test "T07 fast-search"         "$Binary fast main -d $TestDir -e $TestExt"
Run-Test "T08 fast-regex-icase"    "$Binary fast `".*handler.*`" -d $TestDir -e $TestExt --regex -i"
Run-Test "T09 fast-dirs-only"      "$Binary fast src -d $TestDir --dirs-only"
Run-Test "T09a fast-multi-term"    "$Binary fast `"main,lib,handler`" -d $TestDir -e $TestExt"

# T10: content-index
Run-Test "T10 content-index"       "$Binary content-index -d $TestDir -e $TestExt"

# T11-T18: grep
Run-Test "T11 grep-single"         "$Binary grep tokenize -d $TestDir -e $TestExt"
Run-Test "T12 grep-multi-or"       "$Binary grep `"tokenize,posting`" -d $TestDir -e $TestExt"
Run-Test "T13 grep-multi-and"      "$Binary grep `"tokenize,posting`" -d $TestDir -e $TestExt --all"
Run-Test "T14 grep-regex"          "$Binary grep `".*stale.*`" -d $TestDir -e $TestExt --regex"
Run-Test "T15 grep-phrase"         "$Binary grep `"pub fn`" -d $TestDir -e $TestExt --phrase"
Run-Test "T15b grep-phrase-punct"  "$Binary grep `"pub(crate)`" -d $TestDir -e $TestExt --phrase"
Run-Test "T15c grep-multi-phrase"  "$Binary grep `"pub fn,pub struct`" -d $TestDir -e $TestExt --phrase"
Run-Test "T16 grep-context"        "$Binary grep is_stale -d $TestDir -e $TestExt --show-lines -C 2 --max-results 2"
Run-Test "T17 grep-exclude"        "$Binary grep ContentIndex -d $TestDir -e $TestExt --exclude-dir bench"
Run-Test "T18 grep-count"          "$Binary grep fn -d $TestDir -e $TestExt -c"
Run-Test "T24 grep-before-after"   "$Binary grep is_stale -d $TestDir -e $TestExt --show-lines -B 1 -A 3"

# T61-T64: grep substring (default) and --exact
Run-Test "T61 grep-substring-default" "$Binary grep contentindex -d $TestDir -e $TestExt"
Run-Test "T62 grep-substring-and"     "$Binary grep `"contentindex,tokenize`" -d $TestDir -e $TestExt --all"
Run-Test "T63 grep-exact"             "$Binary grep contentindex -d $TestDir -e $TestExt --exact"
Run-Test "T64 grep-regex-no-substr"   "$Binary grep `".*stale.*`" -d $TestDir -e $TestExt --regex"

# T19: info
Run-Test "T19 info"                "$Binary info"

# T20: def-index + def-audit
Run-Test "T20 def-index"           "$Binary def-index -d $TestDir -e $TestExt"
Run-Test "T-DEF-AUDIT def-audit"   "$Binary def-audit -d $TestDir -e $TestExt"

# T49: def-index with TypeScript
Run-Test "T49 def-index-ts"        "$Binary def-index -d $TestDir -e ts"
# T-EXT-CHECK: verify index files have new semantic extensions
# NOTE: must run AFTER def-index (T20) since .code-structure files are created by def-index
Write-Host -NoNewline "  T-EXT-CHECK index-file-extensions ... "
$total++
try {
    $idxDir = Join-Path $env:LOCALAPPDATA "xray"
    $fileListFiles = Get-ChildItem -Path $idxDir -Filter "*.file-list" -ErrorAction SilentlyContinue
    $wordSearchFiles = Get-ChildItem -Path $idxDir -Filter "*.word-search" -ErrorAction SilentlyContinue
    $codeStructFiles = Get-ChildItem -Path $idxDir -Filter "*.code-structure" -ErrorAction SilentlyContinue
    $oldIdx = Get-ChildItem -Path $idxDir -Filter "*.idx" -ErrorAction SilentlyContinue
    $oldCidx = Get-ChildItem -Path $idxDir -Filter "*.cidx" -ErrorAction SilentlyContinue
    $oldDidx = Get-ChildItem -Path $idxDir -Filter "*.didx" -ErrorAction SilentlyContinue
    $tmpFiles = Get-ChildItem -Path $idxDir -Filter "*.tmp" -ErrorAction SilentlyContinue

    $extPassed = $true
    if ($tmpFiles) {
        Write-Host "FAILED (.tmp files found — atomic save cleanup failed: $($tmpFiles.Name -join ', '))" -ForegroundColor Red
        $extPassed = $false
    }
    if (-not $fileListFiles) {
        Write-Host "FAILED (no .file-list files found)" -ForegroundColor Red
        $extPassed = $false
    }
    if (-not $wordSearchFiles) {
        Write-Host "FAILED (no .word-search files found)" -ForegroundColor Red
        $extPassed = $false
    }
    if (-not $codeStructFiles) {
        Write-Host "FAILED (no .code-structure files found)" -ForegroundColor Red
        $extPassed = $false
    }
    if ($oldIdx -or $oldCidx -or $oldDidx) {
        Write-Host "FAILED (old .idx/.cidx/.didx files found)" -ForegroundColor Red
        $extPassed = $false
    }
    if ($extPassed) {
        Write-Host "OK (.file-list=$($fileListFiles.Count), .word-search=$($wordSearchFiles.Count), .code-structure=$($codeStructFiles.Count))" -ForegroundColor Green
        $passed++
    }
    else {
        $failed++
    }
}
catch {
    Write-Host "FAILED (exception: $_)" -ForegroundColor Red
    $failed++
}


# T21-T23: error handling
Run-Test "T21 invalid-regex"       "$Binary grep `"[invalid`" -d $TestDir -e $TestExt --regex" -ExpectedExit 1
Run-Test "T22 nonexistent-dir"     "$Binary fast test -d /nonexistent/path/xyz" -ExpectedExit 1

# T42/T42b: tips
Run-Test "T42 tips-strategy-recipes" "$Binary tips | Select-String 'STRATEGY RECIPES'"
Run-Test "T42b tips-query-budget"    "$Binary tips | Select-String 'Query budget'"

# Safety net: ensure content index exists before grep edge-case tests.
$ErrorActionPreference = "Continue"
Invoke-Expression "$Binary content-index -d $TestDir -e $TestExt 2>&1" | Out-Null
$ErrorActionPreference = "Stop"

# T54: grep with non-existent term should return 0 matches gracefully (not crash)
Run-Test "T54 grep-nonexistent-term" "$Binary grep ZZZNonExistentXYZ123 -d $TestDir -e $TestExt"

# T65: fast with invalid regex should return error (exit 1)
Run-Test "T65 fast-invalid-regex"    "$Binary fast `"[invalid`" -d $TestDir --regex" -ExpectedExit 1

# T76: fast with empty pattern should return error (exit 1)
Run-Test "T76 fast-empty-pattern"    "$Binary fast `"`" -d $TestDir -e $TestExt" -ExpectedExit 1

# T80: grep with non-existent directory should return error (no index found)
Run-Test "T80 grep-nonexistent-dir"  "$Binary grep fn -d C:\nonexistent\fakepath123 -e $TestExt" -ExpectedExit 1

# T82: grep with --max-results 0 should work (0 means unlimited)
Run-Test "T82 grep-max-results-zero" "$Binary grep fn -d $TestDir -e $TestExt --max-results 0"

# T83: grep with Unicode terms should not crash (exit 0, 0 results)
Run-Test "T83 grep-unicode-no-crash" "$Binary grep `"数据库连接`" -d $TestDir -e $TestExt"

# T-SHUTDOWN: save-on-shutdown
Write-Host -NoNewline "  T-SHUTDOWN save-on-shutdown ... "
$total++
try {
    $t59dir = Join-Path $env:TEMP "search_e2e_shutdown_$PID"
    if (Test-Path $t59dir) { Remove-Item -Recurse -Force $t59dir }
    New-Item -ItemType Directory -Path $t59dir | Out-Null

    $t59file = Join-Path $t59dir "Original.cs"
    Set-Content -Path $t59file -Value "class Original { void Run() { } }"

    # Find the search binary (installed or debug)
    $searchBin = (Get-Command xray.exe -ErrorAction SilentlyContinue).Source
    if (-not $searchBin) { $searchBin = ".\target\debug\xray.exe" }

    $ErrorActionPreference = "Continue"
    & $searchBin content-index -d $t59dir -e cs 2>&1 | Out-Null
    $ErrorActionPreference = "Stop"

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $searchBin
    $psi.Arguments = "serve --dir `"$t59dir`" --ext cs --watch"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $t59proc = New-Object System.Diagnostics.Process
    $t59proc.StartInfo = $psi

    $stderrBuilder = New-Object System.Text.StringBuilder
    $stdoutBuilder = New-Object System.Text.StringBuilder
    $errHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }
    $outHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }

    $errEvent = Register-ObjectEvent -InputObject $t59proc -EventName ErrorDataReceived -Action $errHandler -MessageData $stderrBuilder
    $outEvent = Register-ObjectEvent -InputObject $t59proc -EventName OutputDataReceived -Action $outHandler -MessageData $stdoutBuilder

    $t59proc.Start() | Out-Null
    $t59proc.BeginErrorReadLine()
    $t59proc.BeginOutputReadLine()

    $initReq = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
    $t59proc.StandardInput.WriteLine($initReq)

    # Wait for server startup + watcher init (typically <1.5s)
    Start-Sleep -Seconds 2

    Set-Content -Path $t59file -Value "class Modified { void Execute() { } }"

    # Wait for watcher debounce (500ms debounce + processing)
    Start-Sleep -Seconds 2

    $t59proc.StandardInput.Close()

    if (-not $t59proc.WaitForExit(15000)) {
        $t59proc.Kill()
        $t59proc.WaitForExit(5000) | Out-Null
    }

    Start-Sleep -Milliseconds 200

    Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue
    Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue

    $stderrContent = $stderrBuilder.ToString()

    if ($stderrContent -match "Content index saved on shutdown|saving indexes before shutdown") {
        Write-Host "OK" -ForegroundColor Green
        $passed++
    }
    else {
        $cidxFilesAfter = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA "xray") -Filter "*.word-search" |
        Where-Object { $_.LastWriteTime -gt (Get-Date).AddMinutes(-1) }
        if ($cidxFilesAfter) {
            Write-Host "OK (verified via file timestamp)" -ForegroundColor Green
            $passed++
        }
        else {
            Write-Host "FAILED (no save-on-shutdown detected)" -ForegroundColor Red
            Write-Host "    stderr: $stderrContent" -ForegroundColor Yellow
            $failed++
        }
    }

    if (!$t59proc.HasExited) { $t59proc.Kill() }
    $t59proc.Dispose()
    Remove-Item -Recurse -Force $t59dir -ErrorAction SilentlyContinue
}
catch {
    Write-Host "FAILED (exception: $_)" -ForegroundColor Red
    $failed++
    if ($t59proc -and !$t59proc.HasExited) { $t59proc.Kill() }
    if ($t59proc) { $t59proc.Dispose() }
    if ($errEvent) { Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue }
    if ($outEvent) { Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $t59dir -ErrorAction SilentlyContinue
}

# T-FORMAT-VERSION: stale index version doesn't crash server
Write-Host -NoNewline "  T-FORMAT-VERSION stale-index-rebuild ... "
$total++
try {
    $tvDir = Join-Path $env:TEMP "search_e2e_fmtver_$PID"
    if (Test-Path $tvDir) { Remove-Item -Recurse -Force $tvDir }
    New-Item -ItemType Directory -Path $tvDir | Out-Null
    Set-Content -Path (Join-Path $tvDir "hello.cs") -Value "class Hello { void Run() { } }"

    $searchBin2 = (Get-Command xray.exe -ErrorAction SilentlyContinue).Source
    if (-not $searchBin2) { $searchBin2 = ".\target\debug\xray.exe" }

    # Create a stale index with format_version=0 using the hidden test command
    $ErrorActionPreference = "Continue"
    & $searchBin2 test-create-stale-index -d $tvDir -e cs --version 0 2>&1 | Out-Null
    $ErrorActionPreference = "Stop"

    # Start serve — it should detect version mismatch and rebuild, NOT crash
    $tvPsi = New-Object System.Diagnostics.ProcessStartInfo
    $tvPsi.FileName = $searchBin2
    $tvPsi.Arguments = "serve --dir `"$tvDir`" --ext cs"
    $tvPsi.UseShellExecute = $false
    $tvPsi.RedirectStandardInput = $true
    $tvPsi.RedirectStandardOutput = $true
    $tvPsi.RedirectStandardError = $true
    $tvPsi.CreateNoWindow = $true

    $tvProc = New-Object System.Diagnostics.Process
    $tvProc.StartInfo = $tvPsi

    $tvStderr = New-Object System.Text.StringBuilder
    $tvErrHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }
    $tvErrEvent = Register-ObjectEvent -InputObject $tvProc -EventName ErrorDataReceived -Action $tvErrHandler -MessageData $tvStderr

    $tvProc.Start() | Out-Null
    $tvProc.BeginErrorReadLine()

    $tvInitReq = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
    $tvProc.StandardInput.WriteLine($tvInitReq)

    # Wait for server to start and load/rebuild index
    Start-Sleep -Seconds 3

    $tvProc.StandardInput.Close()
    if (-not $tvProc.WaitForExit(10000)) {
        $tvProc.Kill()
        $tvProc.WaitForExit(5000) | Out-Null
    }
    Start-Sleep -Milliseconds 200

    Unregister-Event -SourceIdentifier $tvErrEvent.Name -ErrorAction SilentlyContinue

    $tvStderrText = $tvStderr.ToString()
    $tvExitCode = $tvProc.ExitCode

    # Server should detect version mismatch, rebuild, and start successfully.
    # Exit code -1 is normal when stdin is closed (serve shutdown).
    if (($tvExitCode -eq 0 -or $tvExitCode -eq -1) -and ($tvStderrText -match "version mismatch|Format version mismatch|Cannot read format version|Building content index|MCP server ready")) {
        # Verify we see version mismatch OR successful rebuild message
        Write-Host "OK" -ForegroundColor Green
        $passed++
    }
    else {
        Write-Host "FAILED (exit=$tvExitCode)" -ForegroundColor Red
        Write-Host "    stderr: $tvStderrText" -ForegroundColor Yellow
        $failed++
    }

    if (!$tvProc.HasExited) { $tvProc.Kill() }
    $tvProc.Dispose()
    Remove-Item -Recurse -Force $tvDir -ErrorAction SilentlyContinue
}
catch {
    Write-Host "FAILED (exception: $_)" -ForegroundColor Red
    $failed++
    if ($tvProc -and !$tvProc.HasExited) { $tvProc.Kill() }
    if ($tvProc) { $tvProc.Dispose() }
    if ($tvErrEvent) { Unregister-Event -SourceIdentifier $tvErrEvent.Name -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $tvDir -ErrorAction SilentlyContinue
}

# T-GREP-STALE: CLI grep auto-rebuilds stale content index (format version mismatch)
Write-Host -NoNewline "  T-GREP-STALE grep-auto-rebuild-stale ... "
$total++
try {
    $gsDir = Join-Path $env:TEMP "search_e2e_grep_stale_$PID"
    if (Test-Path $gsDir) { Remove-Item -Recurse -Force $gsDir }
    New-Item -ItemType Directory -Path $gsDir | Out-Null
    Set-Content -Path (Join-Path $gsDir "hello.rs") -Value "fn hello() { println!(""hello""); }"

    $searchBin3 = (Get-Command xray.exe -ErrorAction SilentlyContinue).Source
    if (-not $searchBin3) { $searchBin3 = ".\target\debug\xray.exe" }

    # Create a stale index with format_version=0
    $ErrorActionPreference = "Continue"
    & $searchBin3 test-create-stale-index -d $gsDir -e rs --version 0 2>&1 | Out-Null
    $ErrorActionPreference = "Stop"

    # Run grep — should auto-rebuild and return results (exit 0), not fail
    $ErrorActionPreference = "Continue"
    $gsOutput = & $searchBin3 grep hello -d $gsDir -e rs 2>&1 | Out-String
    $gsExitCode = $LASTEXITCODE
    $ErrorActionPreference = "Stop"

    if ($gsExitCode -eq 0 -and $gsOutput -match "hello") {
        Write-Host "OK" -ForegroundColor Green
        $passed++
    }
    else {
        Write-Host "FAILED (exit=$gsExitCode)" -ForegroundColor Red
        Write-Host "    output: $gsOutput" -ForegroundColor Yellow
        $failed++
    }

    & $searchBin3 cleanup --dir $gsDir 2>&1 | Out-Null
    Remove-Item -Recurse -Force $gsDir -ErrorAction SilentlyContinue
}
catch {
    Write-Host "FAILED (exception: $_)" -ForegroundColor Red
    $failed++
    Remove-Item -Recurse -Force $gsDir -ErrorAction SilentlyContinue
}

# === PARALLEL TEST SECTION ===
# These tests are independent: MCP callers tests use isolated temp directories,
# Git MCP tests are read-only. Safe to run concurrently via Start-Job.

Write-Host "`n=== Parallel MCP tests (Start-Job) ===`n"

$parallelTimer = [System.Diagnostics.Stopwatch]::StartNew()

# Resolve search binary to absolute path (jobs run in different working directory)
$searchBinAbs = (Get-Command xray.exe -ErrorAction SilentlyContinue).Source
if (-not $searchBinAbs) {
    $searchBinAbs = (Resolve-Path ".\target\debug\xray.exe" -ErrorAction SilentlyContinue).Path
}
if (-not $searchBinAbs) {
    Write-Host "ERROR: xray.exe not found (not installed, no debug build)" -ForegroundColor Red
    exit 1
}
$projectDirAbs = (Resolve-Path $TestDir).Path

# --- Define parallel test scriptblocks ---
$testBlocks = @()

# T65-66: callers-local-var-types-down
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T65-66 callers-local-var-types-down"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_callers_down_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "validator.ts") -Value "export class OrderValidator {`n    check(): boolean {`n        return true;`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "service.ts") -Value "import { OrderValidator } from './validator';`n`nexport class OrderService {`n    processOrder(): void {`n        const validator = new OrderValidator();`n        validator.check();`n    }`n}"
        & $Bin content-index -d $tmpDir -e ts 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"processOrder","class":"OrderService","direction":"down","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($jsonLine -and $jsonLine -match 'check') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (check() not found in callTree)" } }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T67: callers-up-false-positive-filter
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T67 callers-up-false-positive-filter"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_callers_up_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "task.ts") -Value "export class TaskRunner {`n    resolve(): boolean {`n        return true;`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "orchestrator.ts") -Value "import { TaskRunner } from './task';`n`nexport class Orchestrator {`n    run(): void {`n        const task = new TaskRunner();`n        task.resolve();`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "pathhelper.ts") -Value "import * as path from 'path';`n`nexport class PathHelper {`n    getFullPath(): string {`n        return path.resolve('/tmp');`n    }`n}"
        & $Bin content-index -d $tmpDir -e ts 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"resolve","class":"TaskRunner","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no JSON-RPC response)" } }
        if ($jsonLine -notmatch 'orchestrator') { return @{ Name = $name; Passed = $false; Output = "FAILED (orchestrator.ts not found)" } }
        if ($jsonLine -match 'pathhelper') { return @{ Name = $name; Passed = $false; Output = "FAILED (pathhelper.ts should be filtered)" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T68: callers-up-graceful-fallback
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T68 callers-up-graceful-fallback"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_callers_fallback_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "dataservice.ts") -Value "export class DataService {`n    fetch(): any[] {`n        return [];`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "consumer.ts") -Value "import { DataService } from './dataservice';`n`nexport class Consumer {`n    load(): void {`n        const svc = new DataService();`n        const result = svc.fetch();`n    }`n}"
        & $Bin content-index -d $tmpDir -e ts 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"fetch","class":"DataService","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($jsonLine -and $jsonLine -match 'consumer') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (consumer.ts not found)" } }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T69: callers-up-comment-false-positive
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T69 callers-up-comment-false-positive"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_callers_comment_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "task-runner.ts") -Value "export class TaskRunner {`n    resolve(): void {`n        console.log(`"resolved`");`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "consumer.ts") -Value "import { TaskRunner } from `"./task-runner`";`n`nexport class Consumer {`n    processData(): void {`n        // We need to resolve the task before proceeding`n        // The resolve method handles cleanup`n        const runner = new TaskRunner();`n        runner.resolve();`n    }`n}"
        & $Bin content-index -d $tmpDir -e ts 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"resolve","class":"TaskRunner","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no JSON-RPC response)" } }
        if ($jsonLine -notmatch 'processData') { return @{ Name = $name; Passed = $false; Output = "FAILED (processData not found)" } }
        if ($jsonLine -notmatch 'totalNodes[^0-9]+1[^0-9]') { return @{ Name = $name; Passed = $false; Output = "FAILED (expected totalNodes=1)" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-FIX3-EXPR-BODY: C# expression body property call sites
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-FIX3-EXPR-BODY callers-csharp-expression-body"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_expr_body_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "NameProvider.cs") -Value "namespace TestApp`n{`n    public class NameProvider`n    {`n        public string GetName() => `"test`";`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "Consumer.cs") -Value "namespace TestApp`n{`n    public class Consumer`n    {`n        private NameProvider _provider;`n        public string DisplayName => _provider.GetName();`n    }`n}"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"GetName","class":"NameProvider","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($jsonLine -and $jsonLine -match 'DisplayName') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (DisplayName not found)" } }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-FIX3-VERIFY: No false positives from missing call-site data
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-FIX3-VERIFY callers-no-false-positives-missing-data"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_verify_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "DataService.cs") -Value "namespace TestApp`n{`n    public class DataService`n    {`n        public void Process() { }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "RealCaller.cs") -Value "namespace TestApp`n{`n    public class RealCaller`n    {`n        private DataService _service;`n        public void Execute()`n        {`n            _service.Process();`n        }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "FalseCaller.cs") -Value "namespace TestApp`n{`n    public class FalseCaller`n    {`n        public void DoWork()`n        {`n            var msg = `"We need to Process the data`";`n            System.Console.WriteLine(msg);`n        }`n    }`n}"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"Process","class":"DataService","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
        if ($jsonLine -notmatch 'RealCaller') { return @{ Name = $name; Passed = $false; Output = "FAILED (RealCaller not found)" } }
        if ($jsonLine -match 'FalseCaller') { return @{ Name = $name; Passed = $false; Output = "FAILED (FalseCaller should be filtered)" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-FIX3-LAMBDA: Lambda calls in arguments captured (C#)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-FIX3-LAMBDA callers-csharp-lambda-in-args"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_lambda_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "Validator.cs") -Value "using System;`nnamespace TestApp`n{`n    public class Validator`n    {`n        public bool Validate(string s)`n        {`n            return s.Length > 0;`n        }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "Processor.cs") -Value "using System;`nusing System.Collections.Generic;`nnamespace TestApp`n{`n    public class Processor`n    {`n        private Validator _validator;`n        public void ProcessAll(List<string> items)`n        {`n            items.ForEach(x => _validator.Validate(x));`n        }`n    }`n}"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"Validate","class":"Validator","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($jsonLine -and $jsonLine -match 'ProcessAll') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (ProcessAll not found)" } }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-OVERLOAD-DEDUP-UP: Overloaded callers not collapsed (direction=up)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-OVERLOAD-DEDUP-UP callers-overloads-not-collapsed-up"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_overload_up_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "Validator.cs") -Value "namespace TestApp`n{`n    public class Validator`n    {`n        public bool Validate() { return true; }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "Processor.cs") -Value "namespace TestApp`n{`n    public class Processor`n    {`n        private Validator _validator;`n        public void Process(int x)`n        {`n            _validator.Validate();`n        }`n        public void Process(string s)`n        {`n            _validator.Validate();`n        }`n    }`n}"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"Validate","class":"Validator","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($jsonLine) {
            $processMatches = [regex]::Matches($jsonLine, '\\?"Process\\?"')
            if ($processMatches.Count -ge 2) { return @{ Name = $name; Passed = $true; Output = "OK" } }
            else { return @{ Name = $name; Passed = $false; Output = "FAILED (expected 2 Process overloads, got $($processMatches.Count))" } }
        } else { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-SAME-NAME-IFACE: Same method name on unrelated interfaces
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-SAME-NAME-IFACE callers-same-name-unrelated-iface"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_same_name_iface_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "IServiceA.cs") -Value "namespace TestApp`n{`n    public interface IServiceA`n    {`n        void Execute();`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "IServiceB.cs") -Value "namespace TestApp`n{`n    public interface IServiceB`n    {`n        void Execute();`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "ServiceA.cs") -Value "namespace TestApp`n{`n    public class ServiceA : IServiceA`n    {`n        public void Execute() { }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "ServiceB.cs") -Value "namespace TestApp`n{`n    public class ServiceB : IServiceB`n    {`n        public void Execute() { }`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "Consumer.cs") -Value "namespace TestApp`n{`n    public class Consumer`n    {`n        private IServiceB _serviceB;`n        public void DoWork()`n        {`n            _serviceB.Execute();`n        }`n    }`n}"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"Execute","class":"ServiceA","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
        if ($jsonLine -match 'Consumer') { return @{ Name = $name; Passed = $false; Output = "FAILED (Consumer should NOT appear)" } }
        if ($jsonLine -notmatch 'totalNodes[^0-9]+0[^0-9]') { return @{ Name = $name; Passed = $false; Output = "FAILED (expected totalNodes=0)" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-SEARCH-INFO-MCP: verify xray_info returns index metadata via MCP
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-SEARCH-INFO-MCP search-info-mcp-response"
    try {
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_info","arguments":{}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no JSON-RPC response)" } }
        $errors = @()
        if ($jsonLine -notmatch 'contentIndex') { $errors += "missing contentIndex" }
        if ($jsonLine -notmatch 'definitionIndex') { $errors += "missing definitionIndex" }
        if ($jsonLine -notmatch 'inMemory') { $errors += "missing inMemory field" }
        if ($jsonLine -match '"isError"\s*:\s*true') { $errors += "isError=true" }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-SERVE-HELP-TOOLS: verify serve --help lists key tools
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-SERVE-HELP-TOOLS serve-help-tool-list"
    try {
        $helpOutput = & $Bin serve --help 2>&1 | Out-String
        $requiredTools = @("xray_branch_status", "xray_git_blame", "xray_help", "xray_reindex_definitions")
        foreach ($tool in $requiredTools) {
            if ($helpOutput -notmatch $tool) { return @{ Name = $name; Passed = $false; Output = "FAILED (missing: $tool)" } }
        }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-BRANCH-STATUS: smoke test for xray_branch_status MCP tool
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-BRANCH-STATUS branch-status-smoke"
    try {
        $repoPath = $Dir -replace '\\', '/'
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}',('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_branch_status","arguments":{"repo":"' + $repoPath + '"}}}')) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if ($jsonLine -and $jsonLine -match 'currentBranch' -and $jsonLine -match 'isMainBranch' -and $jsonLine -notmatch '"isError"\s*:\s*true') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (missing fields or isError)" } }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-GIT-FILE-NOT-FOUND: nonexistent file returns warning, not error
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-GIT-FILE-NOT-FOUND git-history-file-warning"
    try {
        $repoPath = $Dir -replace '\\', '/'
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}',('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":"' + $repoPath + '","file":"DOES_NOT_EXIST_12345.txt"}}}')) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if ($jsonLine -and $jsonLine -match 'warning' -and $jsonLine -notmatch '"isError"\s*:\s*true') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (expected warning, no isError)" } }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-GIT-NOCACHE: noCache parameter returns valid result
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-GIT-NOCACHE git-history-nocache"
    try {
        $repoPath = $Dir -replace '\\', '/'
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}',('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":"' + $repoPath + '","file":"Cargo.toml","noCache":true,"maxResults":1}}}')) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if ($jsonLine -and $jsonLine -match 'commits' -and $jsonLine -notmatch '"isError"\s*:\s*true') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (expected commits, no isError)" } }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-GIT-TOTALCOMMITS: totalCommits shows real total, not truncated count
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-GIT-TOTALCOMMITS git-history-total-vs-returned"
    try {
        $repoPath = $Dir -replace '\\', '/'
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}',('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":"' + $repoPath + '","file":"Cargo.toml","maxResults":1}}}')) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
        $totalMatch = [regex]::Match($jsonLine, 'totalCommits\\?"?\s*:\s*(\d+)')
        $returnedMatch = [regex]::Match($jsonLine, 'returned\\?"?\s*:\s*(\d+)')
        if ($totalMatch.Success -and $returnedMatch.Success) {
            $totalVal = [int]$totalMatch.Groups[1].Value
            $returnedVal = [int]$returnedMatch.Groups[1].Value
            if ($totalVal -gt $returnedVal -and $returnedVal -eq 1) { return @{ Name = $name; Passed = $true; Output = "OK (total=$totalVal, returned=$returnedVal)" } }
            else { return @{ Name = $name; Passed = $false; Output = "FAILED (total=$totalVal should be > returned=$returnedVal)" } }
        } else { return @{ Name = $name; Passed = $false; Output = "FAILED (could not parse counts)" } }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-GIT-CACHE: Git cache routing
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-GIT-CACHE git-cache-routing"
    try {
        $repoPath = $Dir -replace '\\', '/'
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}',('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_git_history","arguments":{"repo":"' + $repoPath + '","file":"Cargo.toml","maxResults":2}}}')) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if ($jsonLine -and $jsonLine -match 'commits') { return @{ Name = $name; Passed = $true; Output = "OK" } }
        else { return @{ Name = $name; Passed = $false; Output = "FAILED (no commits)" } }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-SQL: SQL definition parsing (def-index + xray_definitions)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-SQL sql-definition-parsing"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_sql_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        $sqlContent = @"
CREATE TABLE dbo.Orders (
    OrderId INT PRIMARY KEY,
    CustomerId INT NOT NULL
);
GO

CREATE PROCEDURE dbo.GetOrders
    @CustomerId INT
AS
BEGIN
    SELECT * FROM dbo.Orders WHERE CustomerId = @CustomerId;
END;
GO

CREATE VIEW dbo.OrderSummary AS
SELECT CustomerId, COUNT(*) AS OrderCount FROM dbo.Orders GROUP BY CustomerId;
GO
"@
        Set-Content -Path (Join-Path $tmpDir "schema.sql") -Value $sqlContent

        & $Bin content-index -d $tmpDir -e sql 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e sql 2>&1 | Out-Null

        # Query for stored procedures
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"kind":"storedProcedure"}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext sql --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*4' } | Select-Object -Last 1

        $errors = @()

        if (-not $jsonLine) {
            $errors += "no JSON-RPC response for storedProcedure query"
        } else {
            if ($jsonLine -notmatch 'GetOrders') { $errors += "GetOrders SP not found" }
        }

        # Query for tables
        $msgs2 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"kind":"table"}}}'
        ) -join "`n"
        $output2 = ($msgs2 | & $Bin serve --dir $tmpDir --ext sql --definitions 2>$null) | Out-String
        $jsonLine2 = $output2 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*4' } | Select-Object -Last 1

        if (-not $jsonLine2) {
            $errors += "no JSON-RPC response for table query"
        } else {
            if ($jsonLine2 -notmatch 'Orders') { $errors += "Orders table not found" }
        }

        # Query for views
        $msgs3 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"kind":"view"}}}'
        ) -join "`n"
        $output3 = ($msgs3 | & $Bin serve --dir $tmpDir --ext sql --definitions 2>$null) | Out-String
        $jsonLine3 = $output3 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*4' } | Select-Object -Last 1

        if (-not $jsonLine3) {
            $errors += "no JSON-RPC response for view query"
        } else {
            if ($jsonLine3 -notmatch 'OrderSummary') { $errors += "OrderSummary view not found" }
        }

        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        if ($errors.Count -gt 0) {
            return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" }
        }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-ANGULAR: Angular template metadata (def-index + .code-structure verification)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-ANGULAR angular-template-metadata"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_angular_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        # Create TypeScript fixture with @Component decorators
        $tsContent = @"
import { Component } from '@angular/core';

@Component({
    selector: 'app-root',
    templateUrl: './app.component.html',
})
export class AppComponent {
    title = 'test-app';
}

@Component({
    selector: 'child-widget',
    templateUrl: './child-widget.component.html',
})
export class ChildWidgetComponent {
    data = [];
}
"@
        Set-Content -Path (Join-Path $tmpDir "app.component.ts") -Value $tsContent

        # Create HTML template for AppComponent
        $htmlApp = @"
<div class="container">
    <child-widget [data]="items"></child-widget>
    <pbi-spinner *ngIf="loading"></pbi-spinner>
</div>
"@
        Set-Content -Path (Join-Path $tmpDir "app.component.html") -Value $htmlApp

        # Create HTML template for ChildWidgetComponent
        $htmlChild = @"
<div>
    <data-grid [config]="gridConfig"></data-grid>
</div>
"@
        Set-Content -Path (Join-Path $tmpDir "child-widget.component.html") -Value $htmlChild

        # Run def-index
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
            Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
            return @{ Name = $name; Passed = $false; Output = "FAILED (def-index exit code: $LASTEXITCODE)" }
        }

        # Find and read the .code-structure file
        $idxDir = Join-Path $env:LOCALAPPDATA "xray"
        $codeStructFiles = Get-ChildItem -Path $idxDir -Filter "*.code-structure" -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending
        if (-not $codeStructFiles) {
            & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
            Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
            return @{ Name = $name; Passed = $false; Output = "FAILED (no .code-structure file found)" }
        }

        # Use xray_definitions via MCP to check for selector and templateChildren
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"name":"AppComponent"}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*4' } | Select-Object -Last 1

        $errors = @()

        # Check that the response contains selector info
        if (-not $jsonLine) {
            $errors += "no JSON-RPC response for xray_definitions"
        } else {
            if ($jsonLine -notmatch 'app-root') {
                $errors += "selector 'app-root' not found in response"
            }
            if ($jsonLine -notmatch 'child-widget') {
                $errors += "'child-widget' not found in templateChildren"
            }
            if ($jsonLine -notmatch 'pbi-spinner') {
                $errors += "'pbi-spinner' not found in templateChildren"
            }
        }

        # Also check ChildWidgetComponent for data-grid
        $msgs2 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"name":"ChildWidgetComponent"}}}'
        ) -join "`n"
        $output2 = ($msgs2 | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine2 = $output2 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*4' } | Select-Object -Last 1

        if (-not $jsonLine2) {
            $errors += "no JSON-RPC response for ChildWidgetComponent"
        } else {
            if ($jsonLine2 -notmatch 'child-widget') {
                $errors += "selector 'child-widget' not found in ChildWidgetComponent"
            }
            if ($jsonLine2 -notmatch 'data-grid') {
                $errors += "'data-grid' not found in ChildWidgetComponent templateChildren"
            }
        }

        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        if ($errors.Count -gt 0) {
            return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" }
        }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-RECONCILE: Watcher startup reconciliation catches stale cache files
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-RECONCILE watcher-startup-reconciliation"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_reconcile_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        # Step 1: Create initial file and build index
        Set-Content -Path (Join-Path $tmpDir "Initial.cs") -Value "public class InitialService { public void Run() { } }"
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null

        # Step 2: Add a NEW file AFTER index is built (simulates git pull while server was offline)
        Set-Content -Path (Join-Path $tmpDir "NewFile.cs") -Value "public class NewService { public void Process() { } }"

        # Step 3: Start server with --watch --definitions (reconciliation should catch NewFile.cs)
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"name":"NewService"}}}'
        ) -join "`n"

        # Use --watch so reconciliation runs; sleep 2s to allow reconciliation to complete
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $Bin
        $psi.Arguments = "serve --dir `"$tmpDir`" --ext cs --watch --definitions"
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true

        $proc = New-Object System.Diagnostics.Process
        $proc.StartInfo = $psi

        $stderrBuilder = New-Object System.Text.StringBuilder
        $stdoutBuilder = New-Object System.Text.StringBuilder
        $errHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }
        $outHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }

        $errEvent = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action $errHandler -MessageData $stderrBuilder
        $outEvent = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action $outHandler -MessageData $stdoutBuilder

        $proc.Start() | Out-Null
        $proc.BeginErrorReadLine()
        $proc.BeginOutputReadLine()

        # Send initialize + wait for reconciliation
        $proc.StandardInput.WriteLine($msgs.Split("`n")[0])
        Start-Sleep -Seconds 3  # Allow reconciliation to complete

        # Send notifications/initialized + xray_definitions query
        $proc.StandardInput.WriteLine($msgs.Split("`n")[1])
        $proc.StandardInput.WriteLine($msgs.Split("`n")[2])
        Start-Sleep -Milliseconds 500

        # Close stdin to trigger shutdown
        $proc.StandardInput.Close()
        if (-not $proc.WaitForExit(10000)) { $proc.Kill(); $proc.WaitForExit(5000) | Out-Null }

        Start-Sleep -Milliseconds 200
        Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue
        Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue

        $stdoutContent = $stdoutBuilder.ToString()
        $stderrContent = $stderrBuilder.ToString()

        if (!$proc.HasExited) { $proc.Kill() }
        $proc.Dispose()

        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        # Verify: NewService should be found (reconciliation added the new file)
        if ($stdoutContent -match 'NewService') {
            return @{ Name = $name; Passed = $true; Output = "OK (NewService found after reconciliation)" }
        } else {
            # Also check stderr for reconciliation log
            $reconcileLog = if ($stderrContent -match 'reconciliation') { " (reconciliation logged)" } else { " (no reconciliation log)" }
            return @{ Name = $name; Passed = $false; Output = "FAILED (NewService not found in output$reconcileLog)" }
        }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        if ($proc -and !$proc.HasExited) { $proc.Kill() }
        if ($proc) { $proc.Dispose() }
        if ($errEvent) { Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue }
        if ($outEvent) { Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-BATCH-WATCHER: Batch watcher update — multiple files modified at once (tests batch_purge_files)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-BATCH-WATCHER batch-watcher-multi-file-update"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_batch_watcher_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        # Step 1: Create 5 initial files
        for ($i = 1; $i -le 5; $i++) {
            Set-Content -Path (Join-Path $tmpDir "Service$i.cs") -Value "public class OriginalService$i { public void OldMethod$i() { } }"
        }

        # Build initial indexes
        & $Bin content-index -d $tmpDir -e cs 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e cs 2>&1 | Out-Null

        # Step 2: Modify ALL 5 files (simulates git pull batch update)
        for ($i = 1; $i -le 5; $i++) {
            Set-Content -Path (Join-Path $tmpDir "Service$i.cs") -Value "public class UpdatedService$i { public void NewMethod$i() { } }"
        }

        # Step 3: Start server with --watch --definitions, wait for watcher to process batch
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $Bin
        $psi.Arguments = "serve --dir `"$tmpDir`" --ext cs --watch --definitions"
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true

        $proc = New-Object System.Diagnostics.Process
        $proc.StartInfo = $psi

        $stderrBuilder = New-Object System.Text.StringBuilder
        $stdoutBuilder = New-Object System.Text.StringBuilder
        $errHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }
        $outHandler = { if ($EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) } }

        $errEvent = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action $errHandler -MessageData $stderrBuilder
        $outEvent = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action $outHandler -MessageData $stdoutBuilder

        $proc.Start() | Out-Null
        $proc.BeginErrorReadLine()
        $proc.BeginOutputReadLine()

        # Send initialize and wait for reconciliation + watcher to process modified files
        $proc.StandardInput.WriteLine('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}')
        Start-Sleep -Seconds 4  # Reconciliation catches all 5 modified files

        # Query for UpdatedService3 (definition index) and NewMethod5 (content index)
        $proc.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')
        $proc.StandardInput.WriteLine('{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"name":"UpdatedService3"}}}')
        Start-Sleep -Milliseconds 500
        $proc.StandardInput.WriteLine('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_grep","arguments":{"terms":"NewMethod5"}}}')
        Start-Sleep -Milliseconds 500

        $proc.StandardInput.Close()
        if (-not $proc.WaitForExit(10000)) { $proc.Kill(); $proc.WaitForExit(5000) | Out-Null }

        Start-Sleep -Milliseconds 200
        Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue
        Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue

        $stdoutContent = $stdoutBuilder.ToString()

        if (!$proc.HasExited) { $proc.Kill() }
        $proc.Dispose()

        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        $errors = @()
        # Check definition index updated: UpdatedService3 should be found
        if ($stdoutContent -notmatch 'UpdatedService3') { $errors += "UpdatedService3 not found in definitions" }
        # Check content index updated: NewMethod5 should be found
        if ($stdoutContent -notmatch 'newmethod5') { $errors += "NewMethod5 not found in content index" }
        # Check OLD names are NOT present (should be purged)
        if ($stdoutContent -match 'OriginalService3') { $errors += "OriginalService3 should have been purged" }

        if ($errors.Count -gt 0) {
            return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" }
        }
        return @{ Name = $name; Passed = $true; Output = "OK (5 files batch-updated, both indexes verified)" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        if ($proc -and !$proc.HasExited) { $proc.Kill() }
        if ($proc) { $proc.Dispose() }
        if ($errEvent) { Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue }
        if ($outEvent) { Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-EDIT: xray_edit MCP tool — line-range replace, text-match replace, dryRun
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-EDIT search-edit-line-range-and-text-match"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_edit_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        # Create test file
        Set-Content -Path (Join-Path $tmpDir "test.txt") -Value "line1`nline2`nline3`nline4`nline5"

        # Test 1: Mode A — replace line 2
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + ($tmpDir -replace '\\', '/') + '/test.txt","operations":[{"startLine":2,"endLine":2,"content":"REPLACED"}]}}}')
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1

        $errors = @()
        if (-not $jsonLine) { $errors += "no response for Mode A" }
        elseif ($jsonLine -match '"isError"\s*:\s*true') { $errors += "Mode A returned error" }

        # Verify file was modified
        $content = Get-Content -Path (Join-Path $tmpDir "test.txt") -Raw
        if ($content -notmatch 'REPLACED') { $errors += "file not modified by Mode A" }
        if ($content -match 'line2') { $errors += "line2 should have been replaced" }

        # Test 2: Mode B — text replace
        Set-Content -Path (Join-Path $tmpDir "test2.txt") -Value "hello world hello"
        $msgs2 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + ($tmpDir -replace '\\', '/') + '/test2.txt","edits":[{"search":"hello","replace":"bye"}]}}}')
        ) -join "`n"
        $output2 = ($msgs2 | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine2 = $output2 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1

        if (-not $jsonLine2) { $errors += "no response for Mode B" }
        elseif ($jsonLine2 -match '"isError"\s*:\s*true') { $errors += "Mode B returned error" }

        $content2 = Get-Content -Path (Join-Path $tmpDir "test2.txt") -Raw
        if ($content2 -match 'hello') { $errors += "hello should have been replaced" }
        if ($content2 -notmatch 'bye') { $errors += "bye not found after replace" }

        # Test 3: dryRun — file should NOT be modified
        Set-Content -Path (Join-Path $tmpDir "test3.txt") -Value "original content"
        $msgs3 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + ($tmpDir -replace '\\', '/') + '/test3.txt","edits":[{"search":"original","replace":"modified"}],"dryRun":true}}}')
        ) -join "`n"
        $output3 = ($msgs3 | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine3 = $output3 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1

        if (-not $jsonLine3) { $errors += "no response for dryRun" }
        elseif ($jsonLine3 -match '"isError"\s*:\s*true') { $errors += "dryRun returned error" }

        $content3 = Get-Content -Path (Join-Path $tmpDir "test3.txt") -Raw
        if ($content3 -notmatch 'original') { $errors += "dryRun should not modify file" }

        # Verify diff is in response
        if ($jsonLine3 -and $jsonLine3 -notmatch 'diff') { $errors += "dryRun response missing diff" }

        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        if ($errors.Count -gt 0) {
            return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" }
        }
        return @{ Name = $name; Passed = $true; Output = "OK (Mode A replace + Mode B replace + dryRun verified)" }
    } catch {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-EDIT-MULTI: xray_edit multi-file + insert after/before + expectedContext
# T-MULTI-METHOD-CALLERS: multi-method batch returns results array
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-MULTI-METHOD callers-multi-method-batch"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_multi_method_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        Set-Content -Path (Join-Path $tmpDir "service.ts") -Value "export class OrderService {`n    process(): void {`n        console.log('processing');`n    }`n    validate(): boolean {`n        return true;`n    }`n}"
        Set-Content -Path (Join-Path $tmpDir "consumer.ts") -Value "import { OrderService } from './service';`n`nexport class Consumer {`n    run(): void {`n        const svc = new OrderService();`n        svc.process();`n        svc.validate();`n    }`n}"
        & $Bin content-index -d $tmpDir -e ts 2>&1 | Out-Null
        & $Bin def-index -d $tmpDir -e ts 2>&1 | Out-Null
        $msgs = @('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}','{"jsonrpc":"2.0","method":"notifications/initialized"}','{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_callers","arguments":{"method":"process,validate","class":"OrderService","direction":"up","depth":1}}}') -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext ts --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no JSON-RPC response)" } }
        $errors = @()
        if ($jsonLine -notmatch 'results') { $errors += 'missing results array' }
        if ($jsonLine -notmatch 'totalMethods') { $errors += 'missing totalMethods in summary' }
        if ($jsonLine -notmatch 'process') { $errors += 'process method not in results' }
        if ($jsonLine -notmatch 'validate') { $errors += 'validate method not in results' }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK (multi-method batch returned results array)" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-EDIT-MULTI search-edit-multi-file-insert-context"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_edit_multi_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        $errors = @()

        # Test 1: Multi-file replace
        Set-Content -Path (Join-Path $tmpDir "a.txt") -Value "old text here"
        Set-Content -Path (Join-Path $tmpDir "b.txt") -Value "old text there"
        $pathA = ($tmpDir -replace '\\', '/') + '/a.txt'
        $pathB = ($tmpDir -replace '\\', '/') + '/b.txt'
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"paths":["' + $pathA + '","' + $pathB + '"],"edits":[{"search":"old","replace":"new"}]}}}')
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine -or $jsonLine -match '"isError"\s*:\s*true') { $errors += "multi-file replace failed" }
        $ca = Get-Content -Path (Join-Path $tmpDir "a.txt") -Raw
        $cb = Get-Content -Path (Join-Path $tmpDir "b.txt") -Raw
        if ($ca -match 'old') { $errors += "a.txt still has old" }
        if ($cb -match 'old') { $errors += "b.txt still has old" }
        if ($jsonLine -notmatch 'filesEdited') { $errors += "missing filesEdited in summary" }

        # Test 2: Insert after
        Set-Content -Path (Join-Path $tmpDir "c.txt") -Value "using System;`nusing System.IO;`n`nclass Foo {}"
        $pathC = ($tmpDir -replace '\\', '/') + '/c.txt'
        $msgs2 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + $pathC + '","edits":[{"insertAfter":"using System.IO;","content":"using System.Linq;"}]}}}')
        ) -join "`n"
        $output2 = ($msgs2 | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine2 = $output2 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine2 -or $jsonLine2 -match '"isError"\s*:\s*true') { $errors += "insertAfter failed" }
        $cc = Get-Content -Path (Join-Path $tmpDir "c.txt") -Raw
        if ($cc -notmatch 'System\.Linq') { $errors += "insertAfter content missing" }

        # Test 3: expectedContext
        Set-Content -Path (Join-Path $tmpDir "d.txt") -Value "var semaphore = new SemaphoreSlim(10);`nDoWork();"
        $pathD = ($tmpDir -replace '\\', '/') + '/d.txt'
        $msgs3 = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + $pathD + '","edits":[{"search":"SemaphoreSlim(10)","replace":"SemaphoreSlim(30)","expectedContext":"var semaphore = new"}]}}}')
        ) -join "`n"
        $output3 = ($msgs3 | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine3 = $output3 -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine3 -or $jsonLine3 -match '"isError"\s*:\s*true') { $errors += "expectedContext replace failed" }
        $cd = Get-Content -Path (Join-Path $tmpDir "d.txt") -Raw
        if ($cd -notmatch 'SemaphoreSlim\(30\)') { $errors += "expectedContext edit not applied" }

        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        if ($errors.Count -gt 0) {
            return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" }
        }
        return @{ Name = $name; Passed = $true; Output = "OK (multi-file + insertAfter + expectedContext verified)" }
    } catch {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-FAST-SUBDIR: xray_fast with subdirectory reuses parent index (no orphan index created)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-FAST-SUBDIR fast-subdir-no-orphan-index"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_fast_subdir_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null

        # Create structure: root/src/App.cs, root/tests/Test.cs
        $srcDir = Join-Path $tmpDir "src"
        $testsDir = Join-Path $tmpDir "tests"
        New-Item -ItemType Directory -Path $srcDir | Out-Null
        New-Item -ItemType Directory -Path $testsDir | Out-Null
        Set-Content -Path (Join-Path $srcDir "App.cs") -Value "class App { }"
        Set-Content -Path (Join-Path $testsDir "Test.cs") -Value "class Test { }"

        # Build index for ROOT only
        & $Bin index -d $tmpDir 2>&1 | Out-Null

        # Count file-list indexes BEFORE
        $idxDir = Join-Path $env:LOCALAPPDATA "xray"
        $countBefore = (Get-ChildItem -Path $idxDir -Filter "*.file-list" -ErrorAction SilentlyContinue).Count

        # Call xray_fast via MCP with dir=src (subdirectory)
        $srcPath = $srcDir -replace '\\', '/'
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_fast","arguments":{"pattern":"*","dir":"' + $srcPath + '"}}}')
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext cs 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1

        & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null

        $errors = @()
        if (-not $jsonLine) { $errors += "no JSON-RPC response" }
        elseif ($jsonLine -match '"isError"\s*:\s*true') { $errors += "isError=true" }

        # Verify results contain App.cs (in src/)
        if ($jsonLine -and $jsonLine -notmatch 'App\.cs') { $errors += "App.cs not found in results" }
        # Verify results do NOT contain Test.cs (in tests/, outside src/)
        if ($jsonLine -and $jsonLine -match 'Test\.cs') { $errors += "Test.cs should NOT be in results (outside src/)" }

        # Verify no orphan file-list index was created
        $countAfter = (Get-ChildItem -Path $idxDir -Filter "*.file-list" -ErrorAction SilentlyContinue).Count
        if ($countAfter -gt $countBefore) { $errors += "orphan file-list index created (before=$countBefore, after=$countAfter)" }

        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue

        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK (subdir results scoped, no orphan index)" }
    } catch {
        if (Test-Path $tmpDir) { & $Bin cleanup --dir $tmpDir 2>&1 | Out-Null; Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}


# T-INTENT-MAPPING: Verify MCP initialize response contains INTENT -> TOOL MAPPING in instructions.
# (TASK ROUTING was removed as a 100% duplicate of INTENT -> TOOL MAPPING during Part 4 slimming.)
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-INTENT-MAPPING mcp-instructions-intent-mapping"
    try {
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*1' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no initialize response)" } }
        $errors = @()
        if ($jsonLine -notmatch 'INTENT -\u003e TOOL MAPPING') { $errors += "missing INTENT -> TOOL MAPPING in instructions" }
        if ($jsonLine -notmatch 'NEVER READ') { $errors += "missing NEVER READ decision trigger" }
        if ($jsonLine -notmatch 'DECISION TRIGGER') { $errors += "missing DECISION TRIGGER" }
        if ($jsonLine -notmatch 'uncertain') { $errors += "missing fallback rule" }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# --- Launch all parallel jobs ---

# T-POLICY-REMINDER: verify policyReminder and nextStepHint in MCP tool response
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-POLICY-REMINDER policy-reminder-in-response"
    try {
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_grep","arguments":{"terms":"fn"}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no JSON-RPC response)" } }
        $errors = @()
        if ($jsonLine -notmatch 'policyReminder') { $errors += "missing policyReminder" }
        if ($jsonLine -notmatch 'nextStepHint') { $errors += "missing nextStepHint" }
        if ($jsonLine -match '"isError"\s*:\s*true') { $errors += "isError=true" }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-EDIT-CREATE: xray_edit auto-creates new files
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-EDIT-CREATE search-edit-auto-create-file"
    try {
        $tmpDir = Join-Path $env:TEMP "search_par_edit_create_$PID"
        if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
        # Do NOT create any files — test auto-creation
        $newFilePath = ($tmpDir -replace '\\', '/') + '/brand_new_file.txt'
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            ('{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_edit","arguments":{"path":"' + $newFilePath + '","operations":[{"startLine":1,"endLine":0,"content":"hello world\nsecond line"}]}}}')
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $tmpDir --ext txt 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        $errors = @()
        if (-not $jsonLine) { $errors += "no response" }
        elseif ($jsonLine -match '"isError"\s*:\s*true') { $errors += "isError=true" }
        if ($jsonLine -and $jsonLine -notmatch 'fileCreated') { $errors += "missing fileCreated field" }
        # Verify file was actually created
        $createdFile = Join-Path $tmpDir "brand_new_file.txt"
        if (-not (Test-Path $createdFile)) { $errors += "file not created on disk" }
        else {
            $content = Get-Content -Path $createdFile -Raw
            if ($content -notmatch 'hello world') { $errors += "file content wrong" }
        }
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK (new file created with fileCreated=true)" }
    } catch {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
        return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" }
    }
}

# T-SORTBY-NO-AUTOSUMMARY: sortBy bypasses autoSummary, returns individual results
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-SORTBY-NO-AUTOSUMMARY definitions-sortby-bypasses-autosummary"
    try {
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"sortBy":"cognitiveComplexity","maxResults":3}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
        $errors = @()
        # Should NOT have autoSummary
        if ($jsonLine -match 'autoSummary') { $errors += "autoSummary should NOT trigger with sortBy" }
        # Should have individual definitions with codeStats
        if ($jsonLine -notmatch 'definitions') { $errors += "missing definitions array" }
        if ($jsonLine -notmatch 'codeStats') { $errors += "missing codeStats in results" }
        if ($jsonLine -notmatch 'cognitiveComplexity') { $errors += "missing cognitiveComplexity metric" }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK (sortBy returned individual ranked results, no autoSummary)" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

# T-HINT-F: File fuzzy-match hint — file filter with slashes finds near-miss path
$testBlocks += , {
    param($Bin, $Dir, $Ext)
    $name = "T-HINT-F-FUZZY definitions-file-fuzzy-match-hint"
    try {
        # Query xray_definitions with a file path containing extra slashes
        # Real path: definitions/tree_sitter_utils.rs. Query: tree/sitter/utils (slashes instead of underscores)
        $msgs = @(
            '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}',
            '{"jsonrpc":"2.0","method":"notifications/initialized"}',
            '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"xray_definitions","arguments":{"file":"tree/sitter/utils"}}}'
        ) -join "`n"
        $output = ($msgs | & $Bin serve --dir $Dir --ext $Ext --definitions 2>$null) | Out-String
        $jsonLine = $output -split "`n" | Where-Object { $_ -match '"id"\s*:\s*5' } | Select-Object -Last 1
        if (-not $jsonLine) { return @{ Name = $name; Passed = $false; Output = "FAILED (no response)" } }
        $errors = @()
        # Should have hint with "Nearest match"
        if ($jsonLine -notmatch 'Nearest match') { $errors += "missing 'Nearest match' in hint" }
        if ($jsonLine -notmatch 'tree_sitter_utils') { $errors += "missing 'tree_sitter_utils' suggestion" }
        if ($jsonLine -notmatch 'Retry with file') { $errors += "missing 'Retry with file' advice" }
        if ($errors.Count -gt 0) { return @{ Name = $name; Passed = $false; Output = "FAILED ($($errors -join '; '))" } }
        return @{ Name = $name; Passed = $true; Output = "OK (fuzzy-match hint suggests tree_sitter_utils.rs)" }
    } catch { return @{ Name = $name; Passed = $false; Output = "FAILED (exception: $_)" } }
}

$parallelJobs = @()
foreach ($block in $testBlocks) {
    $parallelJobs += Start-Job -ScriptBlock $block -ArgumentList $searchBinAbs, $projectDirAbs, $TestExt
}
Write-Host "  Launched $($parallelJobs.Count) parallel test jobs, waiting..."

# Wait with timeout (120s should be more than enough)
$null = $parallelJobs | Wait-Job -Timeout 120

# --- Collect and display results ---
foreach ($job in $parallelJobs) {
    $total++
    if ($job.State -eq 'Running') {
        Stop-Job -Job $job
        Write-Host "  (timed out job) ... FAILED (timeout after 120s)" -ForegroundColor Red
        $failed++
    }
    elseif ($job.State -eq 'Failed') {
        $errMsg = $job.ChildJobs[0].JobStateInfo.Reason.Message
        Write-Host "  (crashed job) ... FAILED (job error: $errMsg)" -ForegroundColor Red
        $failed++
    }
    else {
        $result = Receive-Job -Job $job -ErrorAction SilentlyContinue
        if ($result -and $result.Name) {
            Write-Host -NoNewline "  $($result.Name) ... "
            if ($result.Passed) {
                Write-Host "$($result.Output)" -ForegroundColor Green
                $passed++
            }
            else {
                Write-Host "$($result.Output)" -ForegroundColor Red
                $failed++
            }
        }
        else {
            Write-Host "  (unknown job) ... FAILED (no result returned)" -ForegroundColor Red
            $failed++
        }
    }
    Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
}

$parallelTimer.Stop()
Write-Host "`n  Parallel batch: $($parallelJobs.Count) tests in $([math]::Round($parallelTimer.Elapsed.TotalSeconds, 1))s"

# T25-T52: serve (MCP)
Write-Host "  T25-T52: MCP serve tests - run manually (see e2e-test-plan.md)"

# T53-T58: TypeScript callers (MCP)
Write-Host "  T53-T58: TypeScript callers MCP tests - run manually (see e2e-test-plan.md)"

# Cleanup: remove index files created during E2E tests
Write-Host "`nCleaning up test indexes..."
$ErrorActionPreference = "Continue"
Invoke-Expression "$Binary cleanup --dir $TestDir 2>&1" | Out-Null
Invoke-Expression "$Binary cleanup 2>&1" | Out-Null
$ErrorActionPreference = "Stop"

Write-Host "`n=== Results: $passed passed, $failed failed, $total total ===`n"
if ($failed -gt 0) { exit 1 }