$ErrorActionPreference = 'Stop'
try {
    Get-CimInstance Win32_Process -Filter "name = 'spacetime.exe'" -ErrorAction SilentlyContinue | Where-Object {$_.CommandLine -like '*solar\\scripts\\.data*'} | ForEach-Object {
        Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    }
    $pidFile = Join-Path $PSScriptRoot '.logs\pids.json'
    if (Test-Path $pidFile) {
        $pids = Get-Content $pidFile | ConvertFrom-Json
        foreach ($id in @($pids.stomzy, $pids.spacetime)) {
            if ($id) { Stop-Process -Id ([int]$id) -Force -ErrorAction SilentlyContinue }
        }
        Remove-Item $pidFile -Force -ErrorAction SilentlyContinue
    }
    exit 0
} catch {
    Write-Error "Failed to stop solar: $_"
    exit 1
}
