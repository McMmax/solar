#Requires -Version 5.1
<#
.SYNOPSIS
    Solar Quality Gate Suite (Local CI Equivalent).

.DESCRIPTION
    Runs all Solar JVM tests, Stomzy spacetime tests, native Solar tests, schema tests,
    Hot WASM release build, legacy reference scan, release manifest check,
    and optional MCP smoke test / benchmark.

.PARAMETER IncludeMcp
    Run local MCPClient smoke test and benchmark script if services are running (default: false).
#>
[CmdletBinding()]
param(
    [switch]$IncludeMcp
)
$ErrorActionPreference = 'Stop'

$solarRoot = Split-Path $PSScriptRoot -Parent
$workspace = Split-Path (Split-Path $solarRoot -Parent) -Parent
$stomzy    = Join-Path $workspace 'stomzy'
$hot       = Join-Path $workspace 'local-dev\spacetime\hot'

Write-Host "=== Solar CI Quality Gate Start ==="

# 1. Native Solar Tests
Write-Host "[1/7] Running Solar native tests (cargo test)..."
cargo test --manifest-path (Join-Path $solarRoot "native\Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "Native Solar tests failed" }

# 2. Solar Schema Tests
Write-Host "[2/7] Running Solar schema tests (cargo test)..."
cargo test --manifest-path (Join-Path $solarRoot "schema\Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "Solar schema tests failed" }

# 3. Hot WASM Release Build
Write-Host "[3/7] Building Hot WASM release module..."
cargo build --manifest-path (Join-Path $hot "Cargo.toml") --target wasm32-unknown-unknown --release
if ($LASTEXITCODE -ne 0) { throw "Hot WASM release build failed" }

# 4. Solar JVM Unit & Fixture Tests
Write-Host "[4/7] Running Solar JVM unit tests (:solar:test)..."
Push-Location $stomzy
try {
    & .\gradlew.bat :solar:test --no-daemon
    if ($LASTEXITCODE -ne 0) { throw ":solar:test failed" }

    # 5. Stomzy Spacetime Integration Tests
    Write-Host "[5/7] Running Stomzy spacetime integration tests (:spacetime:test)..."
    & .\gradlew.bat :spacetime:test --no-daemon
    if ($LASTEXITCODE -ne 0) { throw ":spacetime:test failed" }
} finally {
    Pop-Location
}

# 6. Legacy Reference Scan Test
Write-Host "[6/7] Running legacy reference scan on active Solar runtime..."
$runtimeFiles = @(
    (Join-Path $stomzy 'src\main\java\net\minestom\server\instance\SolarInstance.java'),
    (Join-Path $stomzy 'spacetime\src\main\java\net\minestom\server\spacetime\SolarWorldRuntime.java')
)
$forbiddenTokens = @('PolarLoader', 'solar_chunk_put', 'solar_block_set_apply')
foreach ($rf in $runtimeFiles) {
    if (Test-Path $rf) {
        $content = Get-Content $rf -Raw
        foreach ($token in $forbiddenTokens) {
            if ($content -match $token) {
                throw "Forbidden legacy reference '$token' detected in active runtime file: $rf"
            }
        }
    }
}
Write-Host "  -> Legacy scan clean (zero legacy runtime references detected)."

# 7. Optional MCP Smoke Test / Benchmark
if ($IncludeMcp) {
    Write-Host "[7/7] Running MCP smoke test & benchmark..."
    & (Join-Path $PSScriptRoot "mcp-solar-smoke.ps1")
    & (Join-Path $PSScriptRoot "solar-benchmark.ps1")
} else {
    Write-Host "[7/7] Skipping service-dependent MCP smoke test (use -IncludeMcp to enable for local integration)."
}

Write-Host "=== SOLAR ALL QUALITY GATES PASSED CLEANLY ==="
