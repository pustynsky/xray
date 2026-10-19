# Cargo puts Windows test processes in a job that prohibits explicit breakaway.
# Compile with Cargo, then execute the test artifact directly from this shell.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $messages = & cargo test --locked --test shared_backend_stdio --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw "Backend test build failed: $LASTEXITCODE" }
    $executables = @($messages | ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'shared_backend_stdio' -and $_.executable } |
        Select-Object -ExpandProperty executable)
    if ($executables.Count -ne 1) { throw "Expected one backend test executable; found $($executables.Count)" }
    $probe = & $executables[0] windows_backend_spawn_diagnostic --exact --include-ignored --nocapture
    $probeExit = $LASTEXITCODE
    $probe | ForEach-Object { Write-Output $_ }
    if ($probeExit -ne 0) { throw "Backend spawn preflight failed: $probeExit" }
    $supported = @($probe | Where-Object { $_ -eq 'XRAY_TEST_BREAKAWAY=supported' }).Count
    $unsupported = @($probe | Where-Object { $_ -eq 'XRAY_TEST_BREAKAWAY=unsupported' }).Count
    if ($supported -eq 1 -and $unsupported -eq 0) {
        & $executables[0] --include-ignored --nocapture --skip windows_backend_spawn_diagnostic --skip windows_backend_rejects_forbidden_breakaway
        if ($LASTEXITCODE -ne 0) { throw "Backend process tests failed: $LASTEXITCODE" }
        $summary = 'Windows backend: spawn preflight and positive lifecycle/memory tests passed.'
    } elseif ($unsupported -eq 1 -and $supported -eq 0) {
        & $executables[0] windows_backend_rejects_forbidden_breakaway --exact --include-ignored --nocapture
        if ($LASTEXITCODE -ne 0) { throw "Backend unsupported-host contract test failed: $LASTEXITCODE" }
        $summary = 'Windows backend: host denies process breakaway. Explicit startup refusal verified. Positive lifecycle/memory tests NOT RUN on this runner; they require a host that permits independent processes.'
        Write-Warning $summary
        if ($env:GITHUB_ACTIONS -eq 'true') { Write-Output "::warning::$summary" }
    } else {
        throw 'Backend spawn preflight did not report exactly one supported/unsupported result.'
    }
    if ($env:GITHUB_STEP_SUMMARY) {
        Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $summary -Encoding utf8
    }
} finally {
    Pop-Location
}
