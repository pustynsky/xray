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
    & $executables[0] --include-ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Backend process tests failed: $LASTEXITCODE" }
} finally {
    Pop-Location
}
