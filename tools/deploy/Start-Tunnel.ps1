. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$edge="$DeployRoot\edge"
if(!(Test-Path "$edge\deployment.json")){throw 'Configure a named tunnel first'}
Wait-Backend
$task=Get-ScheduledTask -TaskName PatchworkTunnel
if($task.State -ne 'Running'){Start-ScheduledTask -TaskName PatchworkTunnel}
$deadline=[DateTime]::UtcNow.AddSeconds(60)
do {
    try {
        $response=Invoke-WebRequest 'http://127.0.0.1:20242/ready' -UseBasicParsing -TimeoutSec 2
        if($response.StatusCode -eq 200){Write-Output '{"tunnel_connected":true}';exit 0}
    } catch {}
    Start-Sleep -Milliseconds 500
} while([DateTime]::UtcNow -lt $deadline)
throw 'Cloudflare edge readiness timed out; inspect connector state and outbound TCP/UDP 7844'
