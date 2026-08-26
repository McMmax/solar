[CmdletBinding()]
param([string]$Server='http://127.0.0.1:3000',[string]$Database='solar-hot',[string]$WorldName='solar-dev',[UInt64]$WorldKey=1)
$ErrorActionPreference='Stop'; $base="$Server/v1/database/$Database/call"
$create=@($WorldKey,$WorldName,'SOLAR','solar-dev',0,-64,384,0.5,64.0,0.5,0.0,0.0,0,'dev',0,'')|ConvertTo-Json -Compress
Invoke-RestMethod "$base/solar_world_create_or_import" -Method Post -ContentType 'application/json' -Body $create|Out-Null
$finish=@($WorldKey,$true,'')|ConvertTo-Json -Compress
Invoke-RestMethod "$base/solar_world_finish_import" -Method Post -ContentType 'application/json' -Body $finish|Out-Null
Write-Host "Solar world '$WorldName' ($WorldKey) is active in $Database"
