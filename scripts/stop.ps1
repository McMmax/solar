$ErrorActionPreference = 'SilentlyContinue'
Get-CimInstance Win32_Process -Filter "name = 'spacetime.exe'" | Where-Object {$_.CommandLine -like '*solar\\scripts\\.data*'} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force}
$pidFile = Join-Path $PSScriptRoot '.logs\pids.json'
if (Test-Path $pidFile) {
  $pids = Get-Content $pidFile | ConvertFrom-Json
  foreach ($id in @($pids.stomzy, $pids.spacetime)) { if ($id) { Stop-Process -Id ([int]$id) -Force } }
  Remove-Item $pidFile -Force
}
