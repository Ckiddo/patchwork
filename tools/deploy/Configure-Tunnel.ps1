param(
    [Parameter(Mandatory)][ValidatePattern('^(?=.{1,253}$)([a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$')][string]$ApiHostname,
    [Parameter(Mandatory)][Guid]$TunnelId,
    [Parameter(Mandatory)][ValidatePattern('^\d{4}\.\d{1,2}\.\d+$')][string]$RuntimeVersion
)
. "$PSScriptRoot\Common.ps1"
$null=Get-Deployment
$edge="$DeployRoot\edge"
if(Test-Path "$edge\deployment.json"){throw 'Tunnel already configured; inspect existing configuration before changing'}
if(Get-ScheduledTask -TaskName PatchworkTunnel -ErrorAction SilentlyContinue){throw 'Tunnel task already exists'}
if(Get-LocalUser -Name PatchworkTunnel -ErrorAction SilentlyContinue){throw 'Tunnel account already exists'}
if(Get-NetTCPConnection -State Listen -LocalPort 20242 -ErrorAction SilentlyContinue){throw 'Tunnel metrics port occupied'}
$runtime="D:\Tools\cloudflared\$RuntimeVersion"
$manifest=Get-Content "$runtime\manifest.json" -Raw | ConvertFrom-Json
if($manifest.Version -ne $RuntimeVersion -or (Get-FileHash "$runtime\cloudflared.exe").Hash -ne $manifest.Sha256){throw 'Connector integrity mismatch'}
$credentialPath="$DeployRoot\secrets\cloudflared-tunnel.json"
$credential=Get-Content -LiteralPath $credentialPath -Raw | ConvertFrom-Json
if($credential.TunnelID -ne $TunnelId.ToString() -or !$credential.AccountTag -or !$credential.TunnelSecret){throw 'Named tunnel credentials are missing or do not match'}
$credential=$null
New-Item -ItemType Directory -Path "$edge\secrets","$edge\control","$edge\logs" -Force | Out-Null
Set-PrivateAcl $edge
Copy-Item -LiteralPath $credentialPath -Destination "$edge\secrets\tunnel.json"
Copy-Item -LiteralPath "$PSScriptRoot\Run-Tunnel.ps1" -Destination "$edge\Run-Tunnel.ps1"
# A locally managed tunnel has an explicit API-only allowlist and terminal 404.
$config=@"
tunnel: $TunnelId
credentials-file: D:/deploy_patchwork/edge/secrets/tunnel.json
no-autoupdate: true
metrics: 127.0.0.1:20242
loglevel: fatal
protocol: auto
originRequest:
  connectTimeout: 5s
ingress:
  - hostname: $ApiHostname
    path: ^/api(/.*)?$
    service: http://127.0.0.1:18120
  - service: http_status:404
"@
Write-Utf8 "$edge\config.yml" $config
$null=Invoke-Native "$runtime\cloudflared.exe" @('tunnel','--config',"$edge\config.yml",'ingress','validate') "$edge\logs\validation.log"
foreach($case in @(@('/api/ws',0),@('/api/me',0),@('/healthz',1),@('/readyz',1),@('/metrics',1),@('/apix',1),@('/',1))){
    $matched=Invoke-Native "$runtime\cloudflared.exe" @('tunnel','--config',"$edge\config.yml",'ingress','rule',"https://$ApiHostname$($case[0])") "$edge\logs\validation.log"
    if(($matched -join "`n") -notmatch "Matched rule #$($case[1])"){throw 'Ingress allowlist verification failed'}
}
$bytes=New-Object byte[] 32
$rng=[Security.Cryptography.RandomNumberGenerator]::Create()
try {$rng.GetBytes($bytes)} finally {$rng.Dispose()}
$password='Pw!'+([BitConverter]::ToString($bytes)).Replace('-','')
$user=New-LocalUser -Name PatchworkTunnel -Password (ConvertTo-SecureString $password -AsPlainText -Force) -AccountNeverExpires -PasswordNeverExpires -UserMayNotChangePassword -Description 'Patchwork outbound Cloudflare connector only'
$sid=$user.SID.Value
& "$PSScriptRoot\Grant-BatchLogon.ps1" -Sid $sid
Write-Utf8 "$DeployRoot\secrets\tunnel-task.key" $password
Set-PrivateAcl "$DeployRoot\secrets\tunnel-task.key"
# Traverse only on the deployment root: this account cannot read backend secrets,
# PGDATA or backups. Its readable footprint is limited to edge/ and the runtime.
$acl=Get-Acl -LiteralPath $DeployRoot
$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($user.SID,'Traverse','None','None','Allow')))
Set-Acl -LiteralPath $DeployRoot -AclObject $acl
$read=@{};$read[$sid]='ReadAndExecute'
$modify=@{};$modify[$sid]='Modify'
Set-PrivateAcl $edge $read
Set-PrivateAcl "$edge\secrets" $read
Set-PrivateAcl "$edge\control" $modify
Set-PrivateAcl "$edge\logs" $modify
$record=@{Project='patchwork';Host=$env:COMPUTERNAME;ApiHostname=$ApiHostname;TunnelId=$TunnelId.ToString();Executable="$runtime\cloudflared.exe";Sha256=$manifest.Sha256;ConfigSha256=(Get-FileHash "$edge\config.yml").Hash;UserSid=$sid;Metrics='127.0.0.1:20242';ConfiguredUtc=[DateTime]::UtcNow.ToString('o')}
Write-Utf8 "$edge\deployment.json" ($record|ConvertTo-Json)
$action=New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:\deploy_patchwork\edge\Run-Tunnel.ps1' -WorkingDirectory $edge
$settings=New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$null=Register-ScheduledTask -TaskName PatchworkTunnel -Action $action -Trigger (New-ScheduledTaskTrigger -AtStartup) -Settings $settings -User "$env:COMPUTERNAME\PatchworkTunnel" -Password $password -RunLevel Limited -Description 'Patchwork named Cloudflare Tunnel, API only, bounded restart'
$password=$null
@{Configured=$true;Hostname=$ApiHostname;TunnelId=$TunnelId.ToString();Autostart=$true;Started=$false}|ConvertTo-Json
