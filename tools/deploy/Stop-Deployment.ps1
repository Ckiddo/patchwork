param([switch]$WithDatabase)
. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
& "$PSScriptRoot\Stop-Backend.ps1"
if($WithDatabase){
    $service=Get-CimInstance Win32_Service -Filter "Name='PatchworkPostgres'"
    if(!$service -or $service.PathName -notlike '*D:\deploy_patchwork\data\postgresql*'){throw 'Unexpected PostgreSQL service ownership'}
    Stop-Service PatchworkPostgres
    Write-Output 'Patchwork PostgreSQL stopped; scheduled backups require it running'
}
