[CmdletBinding()]
param([switch]$NoBuild, [switch]$SeedWorld)
$ErrorActionPreference = 'Stop'
$solarRoot = Split-Path $PSScriptRoot -Parent
$workspace = Split-Path (Split-Path $solarRoot -Parent) -Parent
$stomzy = if ($env:SOLAR_STOMZY_ROOT) {$env:SOLAR_STOMZY_ROOT} else {Join-Path $workspace 'stomzy'}
$defaultHot = Join-Path $workspace 'local-dev\spacetime\hot'
if (-not (Test-Path (Join-Path $defaultHot 'Cargo.toml'))) {$defaultHot = Join-Path $workspace 'spacetime\hot'}
$hot = if ($env:SOLAR_HOT_MODULE) {$env:SOLAR_HOT_MODULE} else {$defaultHot}
$db = if ($env:SOLAR_HOT_DB) {$env:SOLAR_HOT_DB} else {'solar-hot'}
$world = if ($env:SOLAR_WORLD_NAME) {$env:SOLAR_WORLD_NAME} else {'solar-dev'}
$key = if ($env:SOLAR_WORLD_KEY) {$env:SOLAR_WORLD_KEY} else {'1'}
$data = if ($env:SOLAR_DATA_DIR) {$env:SOLAR_DATA_DIR} else {Join-Path $PSScriptRoot '.data'}
$logs = Join-Path $PSScriptRoot '.logs'; New-Item -ItemType Directory -Force -Path $data,$logs | Out-Null
if (-not (Get-Command spacetime -ErrorAction SilentlyContinue)) {throw 'spacetime CLI is required on PATH'}
if (-not (Test-Path (Join-Path $stomzy 'gradlew.bat'))) {throw "Stomzy checkout not found: $stomzy"}
if (-not (Test-Path (Join-Path $hot 'Cargo.toml'))) {throw "Hot module not found: $hot"}
$stdb = Start-Process spacetime -ArgumentList @('start','--listen-addr','127.0.0.1:3000','--data-dir',$data) -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs 'spacetime.out.log') -RedirectStandardError (Join-Path $logs 'spacetime.err.log') -PassThru
try {
  for ($i=0; $i -lt 30; $i++) {
    $ready = Test-NetConnection -ComputerName 127.0.0.1 -Port 3000 -InformationLevel Quiet -WarningAction SilentlyContinue
    if ($ready) { break }
    Start-Sleep 1
    if ($i -eq 29) { throw 'SpacetimeDB did not become ready' }
  }
  & spacetime publish -s http://127.0.0.1:3000 -p $hot $db --delete-data=on-conflict -y
  if ($SeedWorld -or $env:SOLAR_SEED_WORLD -eq 'true') {& (Join-Path $PSScriptRoot 'seed-world.ps1') -Database $db -WorldName $world -WorldKey ([UInt64]$key)}
  $env:SPACETIME_ENABLED='true'; $env:SPACETIME_URI='ws://127.0.0.1:3000'; $env:SPACETIME_DB=$db
  $env:STOMZY_SOLAR_ONLY='true'
  $env:STOMZY_HOT_MODE='local'; $env:SPACETIME_HOT_URI='ws://127.0.0.1:3000'; $env:SPACETIME_HOT_DB=$db
  $env:MINESTOM_SERVER_ID='solar-dev'; $env:MINESTOM_WORLD_SOURCE='solar'; $env:MINESTOM_SPAWN_WORLD=$world
  $env:SOLAR_WORLD_KEY=$key; $env:SOLAR_IMPORT_FROM_POLAR='false'
  $env:STDB_MAINCLOUD_URI=''
  $demoLog = Join-Path $logs 'stomzy.out.log'
  $demoErr = Join-Path $logs 'stomzy.err.log'
  $classpath = (Get-ChildItem -Path (Join-Path $stomzy 'demo\build\install\demo\lib') -Filter "*.jar" | Select-Object -ExpandProperty FullName) -join ';'
  $javaArgs = @(
    "-ea",
    "--enable-native-access=ALL-UNNAMED",
    "-Dminestom.registry.unsafe-ops=true",
    "-Dminestom.packet-per-tick=300",
    "-XX:MaxRAMPercentage=70.0",
    "-Xss4m",
    "-Djava.library.path=$(Join-Path $stomzy 'spacetime\native\target\release')",
    "-classpath", $classpath,
    "net.minestom.demo.Main"
  )
  $javaBin = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME 'bin\java.exe' } else { "C:\Users\trapaholics\.jdks\temurin-26.0.1\bin\java.exe" }
  $demo = Start-Process $javaBin -ArgumentList $javaArgs -WorkingDirectory $stomzy -WindowStyle Hidden -RedirectStandardOutput $demoLog -RedirectStandardError $demoErr -PassThru
  Set-Content -LiteralPath (Join-Path $logs 'pids.json') -Value (@{spacetime=$stdb.Id; stomzy=$demo.Id} | ConvertTo-Json)
  Write-Host "Solar environment running in background (SpacetimeDB PID $($stdb.Id), Stomzy PID $($demo.Id)). Logs: $logs"
} catch {
  if ($stdb -and -not $stdb.HasExited) { Stop-Process $stdb.Id -Force }
  throw
}
