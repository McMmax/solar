#Requires -Version 5.1
<#
.SYNOPSIS
    Detailed MCPClient packet streaming diagnostic script for Solar.

.DESCRIPTION
    Launches (or reuses) the Solar environment, connects MCPClient to port 25566,
    monitors initial chunk streaming, executes player movement across chunk boundaries,
    probes client-side block states across target chunk coordinates, and records metrics.

.EXAMPLE
    .\mcp-solar-packet-debug.ps1
#>
[CmdletBinding()]
param(
    [string]$HostName = '127.0.0.1',
    [int]$Port = 25566,
    [switch]$NoBuild,
    [switch]$SkipEnvCheck
)
$ErrorActionPreference = 'Stop'

$scriptRoot = $PSScriptRoot
$solarRoot  = Split-Path $scriptRoot -Parent
$workspace  = Split-Path (Split-Path $solarRoot -Parent) -Parent
$logDir     = Join-Path $scriptRoot '.logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$ts         = (Get-Date -Format 'yyyyMMdd-HHmmss')
$jsonReport = Join-Path $logDir "mcp-solar-packet-debug.$ts.json"

Write-Host "[mcp-debug] Starting Solar MCPClient packet diagnostic..."

$startedPids = @()

if (-not $SkipEnvCheck) {
    # Start Solar environment if Stomzy port 25566 is not listening
    $conn = Test-NetConnection -ComputerName $HostName -Port $Port -WarningAction SilentlyContinue
    if (-not $conn.TcpTestSucceeded) {
        Write-Host "[mcp-debug] Server not running on ${HostName}:${Port}. Launching environment via run.ps1..."
        $runArgs = @()
        if ($NoBuild) { $runArgs += '-NoBuild' }
        & (Join-Path $scriptRoot 'run.ps1') @runArgs
        Start-Sleep -Seconds 5
    }
}

$pyScript = @"
import sys
import time
import json

sys.path.insert(0, r'$workspace\eco\tools\mcpclient\harness')
from mcpc import McpClient, McpError

mc = McpClient(host='$HostName', port=47800)
results = {
    'connected': False,
    'initial_position': None,
    'spawn_blocks': {},
    'neighbor_blocks': {},
    'movement_test': False,
    'moved_position': None,
    'moved_blocks': {},
    'errors': []
}

try:
    try:
        mc.disconnect()
    except Exception:
        pass
    time.sleep(1.0)

    print('[python] Connecting MCPClient to $HostName:$Port...')
    mc.connect('$HostName', $Port)
    
    connected = False
    for i in range(15):
        time.sleep(0.5)
        st = mc.get_state()
        if st.get('connected'):
            connected = True
            break

    if not connected:
        results['errors'].append('Failed to connect within timeout')
        print(json.dumps(results, indent=2))
        sys.exit(1)

    results['connected'] = True
    st = mc.get_state()
    results['initial_position'] = st.get('pos') or st.get('latest_tick_record', {}).get('pos')
    print(f"[python] Connected at position: {results['initial_position']}")

    # Probe spawn column (0, Y, 0)
    for y in range(48, 54):
        try:
            blk = mc.get_block(0, y, 0)
            results['spawn_blocks'][f"0,{y},0"] = blk.get('block')
        except Exception as e:
            results['spawn_blocks'][f"0,{y},0"] = f"Error: {e}"

    # Probe neighbor chunks: (16,51,0), (-16,48,0), (0,52,16), (0,52,-16)
    neighbor_coords = [(16, 51, 0), (-16, 48, 0), (0, 52, 16), (0, 52, -16), (32, 52, 0), (-32, 52, 0)]
    for x, y, z in neighbor_coords:
        try:
            blk = mc.get_block(x, y, z)
            results['neighbor_blocks'][f"{x},{y},{z}"] = blk.get('block')
        except Exception as e:
            results['neighbor_blocks'][f"{x},{y},{z}"] = f"Error: {e}"

    # Move player east across chunk boundary
    print('[python] Executing movement script across chunk boundary...')
    mc.run_script([{'ticks': 40, 'keys': ['forward'], 'yaw': -90.0, 'pitch': 0.0}])
    time.sleep(3.0)

    st2 = mc.get_state()
    results['moved_position'] = st2.get('pos') or st2.get('latest_tick_record', {}).get('pos')
    results['movement_test'] = True
    print(f"[python] Moved position: {results['moved_position']}")

    if results['moved_position']:
        px, py, pz = int(results['moved_position'][0]), int(results['moved_position'][1]), int(results['moved_position'][2])
        for y in range(py - 2, py + 3):
            try:
                blk = mc.get_block(px, y, pz)
                results['moved_blocks'][f"{px},{y},{pz}"] = blk.get('block')
            except Exception as e:
                results['moved_blocks'][f"{px},{y},{pz}"] = f"Error: {e}"

    mc.disconnect()

except Exception as e:
    results['errors'].append(str(e))

with open(r'$jsonReport', 'w') as f:
    json.dump(results, f, indent=2)

print('[python] Diagnostic completed.')
"@

$pyFile = Join-Path $logDir "tmp_packet_debug.$ts.py"
Set-Content -Path $pyFile -Value $pyScript -Encoding UTF8

try {
    python $pyFile
} finally {
    if (Test-Path $pyFile) { Remove-Item $pyFile -Force }
}

if (Test-Path $jsonReport) {
    $report = Get-Content $jsonReport | ConvertFrom-Json
    Write-Host "[mcp-debug] Report summary:"
    Write-Host "  Connected: $($report.connected)"
    Write-Host "  Initial Position: $($report.initial_position)"
    Write-Host "  Spawn Blocks Sample Count: $($report.spawn_blocks.PSObject.Properties.Count)"
    Write-Host "  Neighbor Chunks Sample Count: $($report.neighbor_blocks.PSObject.Properties.Count)"
    Write-Host "  Moved Position: $($report.moved_position)"
    
    if ($report.errors.Count -gt 0) {
        Write-Error "[mcp-debug] Diagnostic finished with errors: $($report.errors -join ', ')"
    } else {
        Write-Host "[mcp-debug] SUCCESS: Packet diagnostic completed cleanly."
    }
}
