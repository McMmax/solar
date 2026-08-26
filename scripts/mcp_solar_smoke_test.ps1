# Solar MCP Terrain Smoke Test Script
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$PythonScript = Join-Path $ScriptDir "mcp_solar_smoke_test.py"

python $PythonScript
exit $LASTEXITCODE
