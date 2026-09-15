. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$service=Get-CimInstance Win32_Service -Filter "Name='PatchworkPostgres'"
if(!$service -or $service.PathName -notlike '*D:\deploy_patchwork\data\postgresql*'){throw 'Unexpected PostgreSQL service ownership'}
if($service.State -ne 'Running'){Start-Service PatchworkPostgres}
Start-ScheduledTask -TaskName PatchworkBackend
Wait-Backend
Write-Output 'Patchwork ready at http://127.0.0.1:18120 (on 192.168.5.9)'
