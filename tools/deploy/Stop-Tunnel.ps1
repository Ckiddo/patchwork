. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$edge="$DeployRoot\edge"
if(!(Test-Path "$edge\deployment.json")){throw 'No project tunnel is configured'}
Write-Utf8 "$edge\control\stop" 'stop'
$deadline=[DateTime]::UtcNow.AddSeconds(15)
do {
    $task=Get-ScheduledTask -TaskName PatchworkTunnel
    if($task.State -ne 'Running' -and !(Get-NetTCPConnection -State Listen -LocalPort 20242 -ErrorAction SilentlyContinue)){
        Write-Output '{"tunnel_stopped":true}';exit 0
    }
    Start-Sleep -Milliseconds 500
} while([DateTime]::UtcNow -lt $deadline)
throw 'Tunnel stop timed out; inspect its recorded process before taking further action'
