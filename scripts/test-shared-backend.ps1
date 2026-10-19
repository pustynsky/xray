# Run positive lifecycle tests under the same process restrictions as Cargo/CI.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    & cargo test --locked --test shared_backend_stdio -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Backend process tests failed: $LASTEXITCODE" }
} finally {
    Pop-Location
}
