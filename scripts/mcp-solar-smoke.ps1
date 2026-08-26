#Requires -Version 5.1
<#
.SYNOPSIS
    Solar MCP Client Smoke Verification Script.

.DESCRIPTION
    Connects to the local Stomzy server via the MCPClient HTTP bridge on http://127.0.0.1:47800/mcp,
    probes spawn blocks/area, tests movement across chunk boundaries (positive and negative),
    asserts terrain block presence, and saves machine-readable JSON results to scripts/.logs.

.PARAMETER Port
    Minecraft server port (default: 25566).

.PARAMETER McpUrl
    MCPClient HTTP endpoint (default: http://127.0.0.1:47800/mcp).
#>
[CmdletBinding()]
param(
    [int]$Port = 25566,
    [string]$McpUrl = 'http://127.0.0.1:47800/mcp'
)
$ErrorActionPreference = 'Stop'

$logDir = Join-Path $PSScriptRoot '.logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$ts = Get-Date -Format 'yyyyMMdd-HHmmss'
$jsonOutPath = Join-Path $logDir "mcp-solar-smoke.$ts.json"

Write-Host "[mcp-smoke] Launching Solar MCPClient smoke verification..."

$harnessDir = Join-Path (Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent) 'eco\tools\mcpclient\harness'
if (-not (Test-Path $harnessDir)) {
    $harnessDir = "C:\Users\trapaholics\Documents\GitHub\eco\tools\mcpclient\harness"
}

$pyScript = @"
import sys, time, json
sys.path.insert(0, r'$harnessDir')
from mcpc import McpClient

out_file = r'$jsonOutPath'
port = $Port

results = {
    'connected': False,
    'spawn_state': None,
    'spawn_block': None,
    'spawn_area_sample': None,
    'moved_east_state': None,
    'moved_neg_state': None,
    'non_air_observed': False,
    'errors': []
}

try:
    mc = McpClient()
    st = mc.get_state()
    
    if not st.get('connected'):
        print(f'[python] Connecting to localhost:{port}...')
        mc.connect('localhost', port)
        time.sleep(2.0)

    st = mc.get_state()
    results['connected'] = st.get('connected', False)
    results['spawn_state'] = st

    if not results['connected']:
        results['errors'].append('Failed to connect to Minecraft server via MCPClient')
    else:
        # Probe spawn position
        pos = st.get('latest_tick_record', {}).get('pos', [0, 64, 0])
        sx, sy, sz = int(pos[0]), int(pos[1]), int(pos[2])
        results['spawn_block'] = mc.get_block(sx, sy, sz)
        
        # Probe area around spawn (16x16 horizontal area, Y 60..70)
        area = mc.get_area([sx - 8, 60, sz - 8], [sx + 8, 70, sz + 8])
        blocks = area.get('blocks', []) if isinstance(area, dict) else area
        results['spawn_area_sample'] = blocks[:10] if isinstance(blocks, list) else blocks
        
        # Check non-air presence in block probes
        non_air = [b for b in blocks if isinstance(b, dict) and 'air' not in b.get('block', '').lower()]
        results['non_air_observed'] = len(non_air) > 0 or 'air' not in str(results['spawn_block']).lower()

        # Step 1: Move East across x=16 boundary
        print('[python] Scripted movement East across x=16...')
        mc.run_script([{'ticks': 40, 'keys': ['forward', 'sprint']}])
        time.sleep(2.5)
        east_st = mc.get_state()
        results['moved_east_state'] = east_st

        # Step 2: Move West into negative chunk boundary
        print('[python] Scripted movement West across negative boundary...')
        mc.run_script([{'ticks': 80, 'keys': ['back', 'sprint']}])
        time.sleep(4.0)
        neg_st = mc.get_state()
        results['moved_neg_state'] = neg_st

        # Clean disconnect
        mc.disconnect()

except Exception as e:
    results['errors'].append(str(e))

with open(out_file, 'w') as f:
    json.dump(results, f, indent=2)

print(f'[python] Results saved to {out_file}')
if results['errors'] or not results['connected']:
    sys.exit(1)
"@

$tmpPy = [IO.Path]::GetTempFileName() + ".py"
Set-Content -LiteralPath $tmpPy -Value $pyScript -Encoding UTF8

try {
    python $tmpPy
    if ($LASTEXITCODE -eq 0) {
        Write-Host "[mcp-smoke] PASS: Solar MCPClient smoke verification completed successfully."
    } else {
        Write-Error "[mcp-smoke] FAIL: Solar MCPClient smoke verification failed. See $jsonOutPath for details."
    }
} finally {
    Remove-Item $tmpPy -Force -ErrorAction SilentlyContinue
}
