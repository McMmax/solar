#Requires -Version 5.1
<#
.SYNOPSIS
    Import a Minecraft Anvil world directly into the local Solar SpacetimeDB environment.

.DESCRIPTION
    Starts (or reuses) the hidden local Solar SpacetimeDB instance, then triggers the
    SolarAnvilImportService to stream KitMap1 (or any specified Anvil world) into the
    solar_chunk_base table of the solar-hot database.

    Constraints:
      - Never deletes or modifies the source Minecraft save directory.
      - Never creates a Polar file.
      - Never opens a visible terminal window.
      - All output is written to scripts/.logs/.
      - Cleans up only processes it started (identified in pids.json).
      - Runs Stomzy in import-only mode (SOLAR_IMPORT_ANVIL_ONLY=true).

.PARAMETER AnvilWorld
    Full path to the Minecraft world save directory to import.
    Defaults to the KitMap1 save.

.PARAMETER Database
    SpacetimeDB database name (default: solar-hot).

.PARAMETER WorldKey
    Solar world_key as an unsigned 64-bit integer string (default: 1).

.PARAMETER WorldName
    Solar world_name to register in solar_world (default: solar-dev).

.PARAMETER SpacetimeUri
    SpacetimeDB WebSocket/HTTP endpoint (default: http://127.0.0.1:3000).

.PARAMETER NoBuild
    Skip rebuilding Stomzy before launching.

.PARAMETER SkipEnvCheck
    Skip waiting for a running SpacetimeDB (use if already started via run.ps1).

.EXAMPLE
    .\import-anvil.ps1
    .\import-anvil.ps1 -AnvilWorld "C:\Users\trapaholics\AppData\Roaming\.minecraft\saves\KitMap1" -NoBuild
#>
[CmdletBinding()]
param(
    [string]$AnvilWorld   = (Join-Path $env:APPDATA '.minecraft\saves\KitMap1'),
    [string]$Database     = 'solar-hot',
    [string]$WorldKey     = '1',
    [string]$WorldName    = 'solar-dev',
    [string]$SpacetimeUri = 'http://127.0.0.1:3000',
    [switch]$NoBuild,
    [switch]$SkipEnvCheck
)
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
$solarRoot  = Split-Path $PSScriptRoot -Parent
$workspace  = Split-Path (Split-Path $solarRoot -Parent) -Parent
$stomzy     = if ($env:SOLAR_STOMZY_ROOT) { $env:SOLAR_STOMZY_ROOT } else { Join-Path $workspace 'stomzy' }
$defaultHot = Join-Path $workspace 'local-dev\spacetime\hot'
if (-not (Test-Path (Join-Path $defaultHot 'Cargo.toml'))) {
    $defaultHot = Join-Path $workspace 'spacetime\hot'
}
$hot        = if ($env:SOLAR_HOT_MODULE) { $env:SOLAR_HOT_MODULE } else { $defaultHot }
$logDir     = Join-Path $PSScriptRoot '.logs'
$dataDir    = Join-Path $PSScriptRoot '.data'
New-Item -ItemType Directory -Force -Path $logDir, $dataDir | Out-Null

$ts         = (Get-Date -Format 'yyyyMMdd-HHmmss')
$importLog  = Join-Path $logDir "import-anvil.$ts.log"
$importErr  = Join-Path $logDir "import-anvil.$ts.err.log"
$pidsFile   = Join-Path $logDir 'import-pids.json'

$startedPids = @()

function Log([string]$msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    $line | Add-Content -LiteralPath $importLog
    Write-Host $line
}

function Cleanup {
    foreach ($pid in $startedPids) {
        if (Get-Process -Id $pid -ErrorAction SilentlyContinue) {
            Stop-Process -Id $pid -Force -ErrorAction SilentlyContinue
            Log "Stopped process PID $pid"
        }
    }
}

# ---------------------------------------------------------------------------
# Guard: never touch the Minecraft save
# ---------------------------------------------------------------------------
if (-not (Test-Path $AnvilWorld)) {
    throw "Anvil world not found: $AnvilWorld"
}
if (-not (Test-Path (Join-Path $AnvilWorld 'level.dat'))) {
    throw "Directory does not look like a Minecraft world (no level.dat): $AnvilWorld"
}
Log "=== Solar Anvil import started ==="
Log "  Source:   $AnvilWorld"
Log "  Database: $Database  WorldKey: $WorldKey  WorldName: $WorldName"

# ---------------------------------------------------------------------------
# Validate Anvil region layout
# ---------------------------------------------------------------------------
$classicRegion = Join-Path $AnvilWorld 'region'
$modernRegion  = Join-Path $AnvilWorld 'dimensions\minecraft\overworld\region'
if (-not (Test-Path $classicRegion) -and -not (Test-Path $modernRegion)) {
    throw "No Anvil region directory found under $AnvilWorld"
}
$regionPath = if (Test-Path $classicRegion) { $classicRegion } else { $modernRegion }
$mcaFiles   = Get-ChildItem $regionPath -Filter '*.mca' -ErrorAction SilentlyContinue
Log "  Region path: $regionPath  MCA count: $($mcaFiles.Count)"
if ($mcaFiles.Count -eq 0) {
    throw "No .mca files found in $regionPath - nothing to import"
}

# ---------------------------------------------------------------------------
# Start SpacetimeDB if not already running
# ---------------------------------------------------------------------------
$ownedStdb = $null
if (-not $SkipEnvCheck) {
    $ready = Test-NetConnection -ComputerName 127.0.0.1 -Port 3000 -InformationLevel Quiet -WarningAction SilentlyContinue
    if (-not $ready) {
        if (-not (Get-Command spacetime -ErrorAction SilentlyContinue)) {
            throw 'spacetime CLI is required on PATH'
        }
        Log "Starting local SpacetimeDB..."
        $stdbOut = Join-Path $logDir 'spacetime.out.log'
        $stdbErr = Join-Path $logDir 'spacetime.err.log'
        $ownedStdb = Start-Process spacetime `
            -ArgumentList @('start', '--listen-addr', '127.0.0.1:3000', '--data-dir', $dataDir) `
            -WindowStyle Hidden `
            -RedirectStandardOutput $stdbOut `
            -RedirectStandardError  $stdbErr `
            -PassThru
        $startedPids += $ownedStdb.Id
        for ($i = 0; $i -lt 30; $i++) {
            Start-Sleep 1
            $ready = Test-NetConnection -ComputerName 127.0.0.1 -Port 3000 -InformationLevel Quiet -WarningAction SilentlyContinue
            if ($ready) { break }
            if ($i -eq 29) { Cleanup; throw 'SpacetimeDB did not become ready within 30s' }
        }
        Log "SpacetimeDB ready (PID $($ownedStdb.Id))"

        # Publish Hot WASM
        Log "Publishing Hot WASM to $Database..."
        & spacetime publish -s $SpacetimeUri -p $hot $Database --delete-data=on-conflict -y
        if ($LASTEXITCODE -ne 0) { Cleanup; throw "spacetime publish failed (exit $LASTEXITCODE)" }
        Log "Hot WASM published."
    } else {
        Log "SpacetimeDB already running at $SpacetimeUri - skipping start."
    }
}

# ---------------------------------------------------------------------------
# Build Stomzy (unless -NoBuild)
# ---------------------------------------------------------------------------
if (-not $NoBuild) {
    if (-not (Test-Path (Join-Path $stomzy 'gradlew.bat'))) {
        throw "Stomzy checkout not found: $stomzy"
    }
    Log "Building Stomzy (:demo:installDist)..."
    Push-Location $stomzy
    try {
        & .\gradlew.bat :demo:installDist --no-daemon
        if ($LASTEXITCODE -ne 0) { throw "Gradle build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
    Log "Stomzy build complete."
}

# ---------------------------------------------------------------------------
# Launch Stomzy in import-only mode
# ---------------------------------------------------------------------------
$demoLauncher = Join-Path $stomzy 'demo\build\install\demo\bin\demo.bat'
if (-not (Test-Path $demoLauncher)) {
    throw "Demo launcher not found: $demoLauncher (run with -NoBuild false first)"
}

$env:SPACETIME_ENABLED         = 'true'
$env:SPACETIME_URI             = 'ws://127.0.0.1:3000'
$env:SPACETIME_DB              = $Database
$env:STOMZY_SOLAR_ONLY         = 'true'
$env:STOMZY_HOT_MODE           = 'local'
$env:SPACETIME_HOT_URI         = 'ws://127.0.0.1:3000'
$env:SPACETIME_HOT_DB          = $Database
$env:MINESTOM_SERVER_ID        = 'solar-import'
$env:MINESTOM_WORLD_SOURCE     = 'solar'
$env:MINESTOM_SPAWN_WORLD      = $WorldName
$env:SOLAR_WORLD_KEY           = $WorldKey
$env:SOLAR_WORLD_NAME          = $WorldName
$env:SOLAR_IMPORT_FROM_POLAR   = 'false'
# Signal import-only: import KitMap1 then exit cleanly.
$env:SOLAR_IMPORT_ANVIL_WORLD  = $AnvilWorld
$env:SOLAR_IMPORT_ANVIL_ONLY   = 'true'
$env:STDB_MAINCLOUD_URI        = ''
$env:JAVA_OPTS                 = "-Djava.library.path=$(Join-Path $stomzy 'spacetime\native\target\release')"

Log "Launching Stomzy in import-only mode..."
$stomzyLogFile = Join-Path $logDir "import-stomzy.$ts.log"
$stomzyErrFile = Join-Path $logDir "import-stomzy.err.log"
$stomzyProc = Start-Process cmd.exe `
    -ArgumentList @('/c', $demoLauncher) `
    -WorkingDirectory $stomzy `
    -WindowStyle Hidden `
    -RedirectStandardOutput $stomzyLogFile `
    -RedirectStandardError  $stomzyErrFile `
    -PassThru
$startedPids += $stomzyProc.Id

# Save pids so smoke-verify can cross-reference
@{ spacetime = if ($ownedStdb) { $ownedStdb.Id } else { $null }; stomzy = $stomzyProc.Id } |
    ConvertTo-Json | Set-Content -LiteralPath $pidsFile

Log "Stomzy import process started (PID $($stomzyProc.Id))."
Log "Import log:  $importLog"
Log "Import err:  $importErr"

# ---------------------------------------------------------------------------
# Wait for import completion signal in log
# ---------------------------------------------------------------------------
$timeout  = 300   # seconds
$interval = 2
$elapsed  = 0
$completed = $false
$imported  = 0
$failed    = 0

while ($elapsed -lt $timeout) {
    Start-Sleep $interval
    $elapsed += $interval

    if ($stomzyProc.HasExited) {
        Log "Stomzy process exited (code $($stomzyProc.ExitCode))."
        break
    }

    $content = Get-Content $stomzyLogFile -Raw -ErrorAction SilentlyContinue
    if ($content) {
        if ($content -match '\[solar-import\].*complete.*imported=(\d+)') {
            $imported = [int]$Matches[1]
            $completed = $true
            break
        }
        if ($content -match '\[solar-import\].*ImportResult\{imported=(\d+).*failed=(\d+)') {
            $imported = [int]$Matches[1]
            $failed   = [int]$Matches[2]
            $completed = $true
            break
        }
    }
}

# ---------------------------------------------------------------------------
# Post-import SQL verification
# ---------------------------------------------------------------------------
Log ""
Log "=== Post-import SQL verification ==="
if (Get-Command spacetime -ErrorAction SilentlyContinue) {
    try {
        $worldRes  = & spacetime sql -s $SpacetimeUri $Database "SELECT status, completed_chunks FROM solar_world WHERE world_key = $WorldKey" 2>&1
        $chunkRes  = & spacetime sql -s $SpacetimeUri $Database "SELECT COUNT(*) FROM solar_chunk_base WHERE world_key = $WorldKey" 2>&1
        Log "solar_world row:       $worldRes"
        Log "solar_chunk_base count: $chunkRes"
    } catch {
        Log "SQL query failed (SpacetimeDB may have exited): $_"
    }
} else {
    Log "SKIP: spacetime CLI not on PATH - run smoke-verify.ps1 manually after starting the environment"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Log ""
Log "=== Import summary ==="
Log "  Imported chunks : $imported"
Log "  Failed chunks   : $failed"
Log "  Completed       : $completed"
Log "  Logs            : $logDir"

# Only stop Stomzy (not SpacetimeDB if user was already running it)
if (Get-Process -Id $stomzyProc.Id -ErrorAction SilentlyContinue) {
    Stop-Process -Id $stomzyProc.Id -Force -ErrorAction SilentlyContinue
    Log "Stopped Stomzy import process."
}
if ($ownedStdb -and (Get-Process -Id $ownedStdb.Id -ErrorAction SilentlyContinue)) {
    # Leave SpacetimeDB running so the data persists for Stomzy / smoke-verify
    Log "SpacetimeDB (PID $($ownedStdb.Id)) left running - stop manually or via stop.ps1"
}

if (-not $completed) {
    Write-Warning "Import did not produce a completion log line within ${timeout}s. Check $importLog for details."
    exit 1
}
if ($failed -gt 0) {
    Write-Warning "$failed chunk(s) failed during import. See $importErr for details."
    exit 2
}

Log "=== Import complete -- $imported chunks committed to $Database/$WorldName ==="
