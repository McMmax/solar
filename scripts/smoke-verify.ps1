#Requires -Version 5.1
<#
.SYNOPSIS
    Solar local smoke-verification helper.

.DESCRIPTION
    Queries the local SpacetimeDB solar-hot database (or any database named by
    -Database) for the presence of solar_world and solar_chunk_base rows, then
    checks the Stomzy log for the expected non-materializing Solar startup line.

    Rules enforced:
      - Never opens a visible terminal window.
      - All output is written to scripts/.logs/smoke-verify.*.log.
      - Cleans up only processes it started (never user worlds or broad dirs).
      - Does not require Docker, network, SpacetimeDB server to be running
        (gracefully reports if it is not).

.PARAMETER Database
    Name of the SpacetimeDB database to query (default: solar-hot).

.PARAMETER SpacetimeUri
    SpacetimeDB HTTP endpoint (default: http://127.0.0.1:3000).

.PARAMETER StomzyLog
    Path to the Stomzy stdout log produced by run.ps1 (optional; if omitted
    only the SQL checks run).

.EXAMPLE
    .\smoke-verify.ps1
    .\smoke-verify.ps1 -Database solar-hot -StomzyLog .logs\stomzy.out.log
#>
[CmdletBinding()]
param(
    [string]$Database    = 'solar-hot',
    [string]$SpacetimeUri = 'http://127.0.0.1:3000',
    [string]$StomzyLog   = ''
)
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------
$logDir = Join-Path $PSScriptRoot '.logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$ts      = (Get-Date -Format 'yyyyMMdd-HHmmss')
$outLog  = Join-Path $logDir "smoke-verify.$ts.log"
$errLog  = Join-Path $logDir "smoke-verify.$ts.err.log"

function Log([string]$msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    $line | Add-Content -LiteralPath $outLog
    Write-Host $line
}

$startedPids = [System.Collections.Generic.List[int]]::new()
$allPassed   = $true

Log "=== Solar smoke-verify start (db=$Database uri=$SpacetimeUri) ==="

# ---------------------------------------------------------------------------
# Helper: run a spacetime SQL query hidden, capture stdout, log stderr
# ---------------------------------------------------------------------------
function Invoke-SpacetimeSql([string]$Sql) {
    $tmpOut = [IO.Path]::GetTempFileName()
    $tmpErr = [IO.Path]::GetTempFileName()
    try {
        $p = Start-Process spacetime `
            -ArgumentList @('sql', '-s', $SpacetimeUri, $Database, $Sql) `
            -WindowStyle Hidden `
            -RedirectStandardOutput $tmpOut `
            -RedirectStandardError  $tmpErr `
            -PassThru
        $startedPids.Add($p.Id)
        $p.WaitForExit(15000) | Out-Null
        $stdout = Get-Content $tmpOut -Raw -ErrorAction SilentlyContinue
        $stderr = Get-Content $tmpErr -Raw -ErrorAction SilentlyContinue
        if ($stderr) { $stderr | Add-Content -LiteralPath $errLog }
        return [PSCustomObject]@{ ExitCode = $p.ExitCode; Output = $stdout }
    } finally {
        Remove-Item $tmpOut, $tmpErr -Force -ErrorAction SilentlyContinue
        $startedPids.Remove($p.Id) | Out-Null
    }
}

# ---------------------------------------------------------------------------
# Check 1: spacetime CLI available
# ---------------------------------------------------------------------------
if (-not (Get-Command spacetime -ErrorAction SilentlyContinue)) {
    Log "SKIP: spacetime CLI not on PATH — SQL checks skipped"
    $allPassed = $false
} else {
    # -----------------------------------------------------------------------
    # Check 2: solar_world row count
    # -----------------------------------------------------------------------
    Log "CHECK solar_world row count..."
    $res = Invoke-SpacetimeSql "SELECT COUNT(*) FROM solar_world"
    if ($res.ExitCode -ne 0) {
        Log "FAIL: spacetime SQL exited $($res.ExitCode) — is solar-hot running? Check $errLog"
        $allPassed = $false
    } else {
        $count = ($res.Output -split '\r?\n' | Where-Object { $_ -match '^\s*\d' } | Select-Object -First 1).Trim()
        if ([int]$count -ge 1) {
            Log "PASS: solar_world has $count row(s)"
        } else {
            Log "FAIL: solar_world is empty — world not seeded yet (run seed-world.ps1)"
            $allPassed = $false
        }
    }

    # -----------------------------------------------------------------------
    # Check 3: solar_chunk_base row count
    # -----------------------------------------------------------------------
    Log "CHECK solar_chunk_base row count..."
    $res2 = Invoke-SpacetimeSql "SELECT COUNT(*) FROM solar_chunk_base"
    if ($res2.ExitCode -ne 0) {
        Log "FAIL: solar_chunk_base query failed (exit $($res2.ExitCode))"
        $allPassed = $false
    } else {
        $count2 = ($res2.Output -split '\r?\n' | Where-Object { $_ -match '^\s*\d' } | Select-Object -First 1).Trim()
        if ([int]$count2 -ge 1) {
            Log "PASS: solar_chunk_base has $count2 row(s)"
        } else {
            Log "INFO: solar_chunk_base is empty — no chunks imported yet (expected before first import)"
        }
    }
}

# ---------------------------------------------------------------------------
# Check 4: Stomzy log — non-materializing Solar startup line
# ---------------------------------------------------------------------------
if ($StomzyLog -and (Test-Path $StomzyLog)) {
    Log "CHECK Stomzy log for non-materializing Solar startup..."
    $pattern = "loaded Solar world '.*' .* non-materializing runtime"
    $match = Select-String -Path $StomzyLog -Pattern $pattern -Quiet
    if ($match) {
        Log "PASS: non-materializing Solar startup line found in $StomzyLog"
    } else {
        Log "FAIL: non-materializing startup line NOT found in $StomzyLog"
        $allPassed = $false
    }
} elseif ($StomzyLog) {
    Log "SKIP: Stomzy log not found at $StomzyLog"
} else {
    Log "SKIP: -StomzyLog not provided; skipping startup log check"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Log "=== Solar smoke-verify complete — $(if ($allPassed) {'ALL PASSED'} else {'SOME CHECKS FAILED'}) ==="
Log "Full log: $outLog"
if (-not $allPassed) { exit 1 }
