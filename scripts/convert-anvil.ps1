[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$InputWorld,[string]$OutputPolar=(Join-Path $PSScriptRoot 'converted.polar'),[string]$StomzyRoot=(Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'eco\local-dev\servers\lobby'),[switch]$Background)
$ErrorActionPreference='Stop'
$launcher=Join-Path $StomzyRoot 'build\install\games-lobby\bin\games-lobby.bat'
if(-not(Test-Path $launcher)){throw "Stomzy demo launcher not found: $launcher"}
if(-not(Test-Path (Join-Path $InputWorld 'region')) -and -not(Test-Path (Join-Path $InputWorld 'dimensions\minecraft\overworld\region'))){throw "No Anvil region directory found under $InputWorld"}
$logDir=Join-Path $PSScriptRoot '.logs'; New-Item -ItemType Directory -Force $logDir|Out-Null
$env:CRUISER_CONVERT_ANVIL_IN=(Resolve-Path $InputWorld).Path; $env:CRUISER_CONVERT_ANVIL_OUT=[IO.Path]::GetFullPath($OutputPolar); $env:SPACETIME_ENABLED='false'; $env:MINESTOM_SERVER_ID=''
$outLog=Join-Path $logDir 'anvil-convert.out.log'; $errLog=Join-Path $logDir 'anvil-convert.err.log'
if($Background){$p=Start-Process $launcher -WorkingDirectory $StomzyRoot -WindowStyle Hidden -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru; Write-Host "Anvil conversion running in background (PID $($p.Id)). Logs: $logDir"}else{& $launcher *> $outLog; if($LASTEXITCODE -ne 0){throw "Anvil conversion failed; see $outLog"}; if(-not(Test-Path $OutputPolar)){throw "Converter produced no output: $OutputPolar"}; Get-Item $OutputPolar|Select-Object FullName,Length}
