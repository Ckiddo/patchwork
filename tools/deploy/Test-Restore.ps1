param(
    [Parameter(Mandatory)][ValidateSet('Setup','Start','Stop','Capture','Logical','Base','Recover','Inspect','Cleanup')][string]$Operation,
    [string]$TargetTime
)
. "$PSScriptRoot\Common.ps1"
$c=Get-Deployment
$contextPath="$DeployRoot\control\acceptance-context.json"
function Stop-Acceptance {
    Write-Utf8 "$DeployRoot\control\acceptance.stop" 'stop'
    $deadline=[DateTime]::UtcNow.AddSeconds(40)
    do {
        $listening=@(Get-NetTCPConnection -State Listen -LocalPort 18121 -ErrorAction SilentlyContinue).Count
        $task=Get-ScheduledTask -TaskName PatchworkAcceptance -ErrorAction SilentlyContinue
        if(!$listening -and (!$task -or $task.State -ne 'Running')){return}
        Start-Sleep -Milliseconds 300
    } while([DateTime]::UtcNow -lt $deadline)
    throw 'Acceptance backend stop deadline exceeded'
}
function Start-Acceptance {
    if(@(Get-NetTCPConnection -State Listen -LocalPort 18120 -ErrorAction SilentlyContinue).Count){throw 'Stop production backend before isolated acceptance'}
    Start-ScheduledTask -TaskName PatchworkAcceptance
    $deadline=[DateTime]::UtcNow.AddSeconds(40)
    do {
        try {if((Invoke-RestMethod http://127.0.0.1:18121/readyz -TimeoutSec 2).database -eq 'ok'){return}} catch {}
        Start-Sleep -Milliseconds 300
    } while([DateTime]::UtcNow -lt $deadline)
    throw 'Acceptance readiness deadline exceeded'
}
function Get-State {
    Invoke-Sql @'
SELECT json_build_object(
 'games',(SELECT coalesce(json_agg(json_build_object('id',game_id,'version',state_version,'hash',md5(snapshot::text),'phase',phase) ORDER BY game_id),'[]') FROM patchwork.games),
 'results',(SELECT coalesce(json_agg(json_build_object('id',game_id,'score0',score0,'score1',score1,'winner',winner_seat,'reason',reason) ORDER BY game_id),'[]') FROM patchwork.game_results),
 'receipts',(SELECT count(*) FROM patchwork.command_receipts),
 'events',(SELECT count(*) FROM patchwork.game_events));
'@
}
if($Operation -eq 'Setup'){
    if(Test-Path $contextPath){throw 'Existing acceptance context requires cleanup first'}
    if(Get-ScheduledTask -TaskName PatchworkAcceptance -ErrorAction SilentlyContinue){throw 'Acceptance task already exists'}
    if(@(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue|Where-Object {$_.LocalPort -in 15433,18121}).Count){throw 'Restore ports occupied'}
    $stamp=[DateTime]::UtcNow.ToString('yyyyMMddHHmmss')
    $ctx=@{Id=$stamp;Database="patchwork_test_deploy_$stamp";LogicalDatabase="patchwork_test_restore_$stamp";Directory="$DeployRoot\restore-tests\$stamp";Port=15432}
    New-Item -ItemType Directory -Path $ctx.Directory | Out-Null
    Write-Utf8 $contextPath ($ctx|ConvertTo-Json)
    Use-Pg 'admin' 'postgres'
    $null=Invoke-Sql "CREATE DATABASE $($ctx.Database) OWNER patchwork_owner"
    Use-Pg 'admin' $ctx.Database
    $null=Invoke-Sql "REVOKE ALL ON DATABASE $($ctx.Database) FROM PUBLIC; GRANT CONNECT ON DATABASE $($ctx.Database) TO patchwork_app,patchwork_migrate,patchwork_backup; REVOKE CREATE ON SCHEMA public FROM PUBLIC; CREATE SCHEMA patchwork AUTHORIZATION patchwork_owner; GRANT USAGE ON SCHEMA patchwork TO patchwork_app; ALTER DEFAULT PRIVILEGES FOR ROLE patchwork_owner IN SCHEMA patchwork GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO patchwork_app; ALTER DEFAULT PRIVILEGES FOR ROLE patchwork_owner IN SCHEMA patchwork GRANT USAGE,SELECT ON SEQUENCES TO patchwork_app;"
    $hba="$DeployRoot\config\postgresql\pg_hba.conf"
    Copy-Item -LiteralPath $hba -Destination "$($ctx.Directory)\original-pg_hba.conf"
    Write-Utf8 $hba ((Get-Content $hba -Raw)+"`nhost $($ctx.Database),$($ctx.LogicalDatabase) patchwork_app,patchwork_migrate,patchwork_backup 127.0.0.1/32 scram-sha-256`n")
    $null=Invoke-Sql 'SELECT pg_reload_conf()'
    $app=(Get-Content "$DeployRoot\config\backend.toml" -Raw).Replace("name = 'patchwork'","name = '$($ctx.Database)'").Replace('127.0.0.1:18120','127.0.0.1:18121').Replace('control/backend.stop','control/acceptance.stop')
    $app=$app.Replace('[database]',"[database]`nlog_pool_acquire = true")
    Write-Utf8 "$DeployRoot\config\acceptance.toml" $app
    $migration=(Get-Content "$DeployRoot\config\migration.toml" -Raw).Replace("name = 'patchwork'","name = '$($ctx.Database)'")
    Write-Utf8 "$DeployRoot\config\acceptance-migration.toml" $migration
    $release=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
    $null=Invoke-Native "$DeployRoot\releases\$release\patchwork-migrate.exe" @('--config',"$DeployRoot\config\acceptance-migration.toml") "$($ctx.Directory)\migrate.log"
    $action=New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Start-Acceptance.ps1' -WorkingDirectory $DeployRoot
    $settings=New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    $password=Get-Content "$DeployRoot\secrets\backend-task.key" -Raw
    $null=Register-ScheduledTask -TaskName PatchworkAcceptance -Action $action -Settings $settings -User "$env:COMPUTERNAME\PatchworkBackend" -Password $password -RunLevel Limited
    $password=$null
    & "$PSScriptRoot\Stop-Backend.ps1" | Out-Null
    Start-Acceptance
    $ctx|ConvertTo-Json
    exit 0
}
$ctx=Get-Content $contextPath -Raw|ConvertFrom-Json
$dir=Assert-ProjectPath $ctx.Directory
if($ctx.Database -notmatch '^patchwork_test_deploy_[0-9]{14}$' -or $ctx.LogicalDatabase -notmatch '^patchwork_test_restore_[0-9]{14}$'){throw 'Invalid isolated restore database'}
switch($Operation){
    'Start' {Start-Acceptance;Write-Output '{"started":true}'}
    'Stop' {Stop-Acceptance;Write-Output '{"stopped":true}'}
    'Capture' {Use-Pg 'admin' $ctx.Database $ctx.Port;Get-State}
    'Logical' {
        Stop-Acceptance
        $timer=[Diagnostics.Stopwatch]::StartNew()
        Use-Pg 'backup' $ctx.Database
        $null=Invoke-Native "$PgBin\pg_dump.exe" @('--format=custom','--file',"$dir\acceptance.dump",$ctx.Database) "$dir\logical.log"
        Use-Pg 'admin' 'postgres'
        $null=Invoke-Sql "CREATE DATABASE $($ctx.LogicalDatabase) OWNER patchwork_owner"
        $null=Invoke-Native "$PgBin\pg_restore.exe" @('--exit-on-error','--dbname',$ctx.LogicalDatabase,"$dir\acceptance.dump") "$dir\logical.log"
        Use-Pg 'admin' $ctx.Database
        $source=Get-State
        Use-Pg 'admin' $ctx.LogicalDatabase
        $restored=Get-State
        if($source -ne $restored){throw 'Logical restore state differs'}
        $report=@{Kind='LogicalRestore';Database=$ctx.LogicalDatabase;State=($restored|ConvertFrom-Json);Sha256=(Get-FileHash "$dir\acceptance.dump" -Algorithm SHA256).Hash;Seconds=$timer.Elapsed.TotalSeconds;Passed=$true}
        Write-Utf8 "$dir\logical-result.json" ($report|ConvertTo-Json -Depth 9)
        Start-Acceptance
        $report|ConvertTo-Json -Depth 9
    }
    'Base' {
        & "$PSScriptRoot\New-Backup.ps1" -Kind Base | Out-Null
        $latest=Get-ChildItem "$DeployRoot\backups\base" -Directory | Where-Object {Test-Path "$($_.FullName)\manifest.json"}|Sort-Object Name -Descending|Select-Object -First 1
        $ctx|Add-Member -NotePropertyName Base -NotePropertyValue $latest.FullName -Force
        Write-Utf8 $contextPath ($ctx|ConvertTo-Json)
        @{Base=$latest.FullName}|ConvertTo-Json
    }
    'Recover' {
        if(!$TargetTime -or $TargetTime -notmatch '^\d{4}-\d{2}-\d{2}T[0-9:.]+Z$'){throw 'Explicit UTC recovery target required'}
        $pgTarget=[DateTimeOffset]::Parse($TargetTime).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss.ffffffzzz')
        Stop-Acceptance
        Use-Pg 'admin' $ctx.Database
        $null=Invoke-Sql 'SELECT pg_switch_wal()'
        $deadline=[DateTime]::UtcNow.AddSeconds(90)
        do {
            if(!(Get-ChildItem "$DeployRoot\data\postgresql\pg_wal\archive_status" -Filter '*.ready')){break}
            if([DateTime]::UtcNow -ge $deadline){throw 'WAL archive deadline exceeded'}
            Start-Sleep -Milliseconds 500
        } while($true)
        $timer=[Diagnostics.Stopwatch]::StartNew()
        $base=Assert-ProjectPath $ctx.Base
        $null=Invoke-Native "$PgBin\pg_verifybackup.exe" @("$base\data") "$dir\pitr.log"
        $restore=Assert-ProjectPath "$dir\pitr-data"
        if(Test-Path $restore){throw 'Refusing to overwrite an existing restore cluster'}
        Copy-Item -LiteralPath "$base\data" -Destination $restore -Recurse
        Write-Utf8 "$dir\restore-hba.conf" "host all patchwork_admin,patchwork_app 127.0.0.1/32 scram-sha-256`n"
        $restoreSlash=$restore.Replace('\','/');$dirSlash=$dir.Replace('\','/')
        Write-Utf8 "$dir\restore.conf" @"
data_directory = '$restoreSlash'
hba_file = '$dirSlash/restore-hba.conf'
listen_addresses = '127.0.0.1'
port = 15433
max_connections = 32
shared_buffers = '64MB'
archive_mode = off
logging_collector = off
log_min_error_statement = panic
restore_command = 'copy /Y D:\\deploy_patchwork\\backups\\wal\\%f "%p" >NUL'
recovery_target_time = '$pgTarget'
recovery_target_action = 'promote'
recovery_target_timeline = 'latest'
"@
        Write-Utf8 "$restore\recovery.signal" ''
        # OpenSSH terminates ordinary child processes when its job closes. Use a
        # temporary, manual-start service so recovered PG survives the SSH command.
        if(Get-Service PatchworkRestore -ErrorAction SilentlyContinue){throw 'Restore service already exists'}
        $null=Invoke-Native "$PgBin\pg_ctl.exe" @('register','-D',$restore,'-N','PatchworkRestore','-U','NT SERVICE\PatchworkRestore','-S','demand','-o',"-c config_file=$dirSlash/restore.conf") "$dir\pitr.log"
        $restoreSid=([Security.Principal.NTAccount]'NT SERVICE\PatchworkRestore').Translate([Security.Principal.SecurityIdentifier]).Value
        $access=@{};$access[$restoreSid]='Modify';Set-PrivateAcl $dir $access
        $walAccess=@{};$walAccess[$restoreSid]='ReadAndExecute'
        $walAccess[([Security.Principal.NTAccount]'NT SERVICE\PatchworkPostgres').Translate([Security.Principal.SecurityIdentifier]).Value]='Modify'
        Set-PrivateAcl "$DeployRoot\backups\wal" $walAccess
        # Service stderr uses the collector instead of an SSH pipe.
        [IO.File]::AppendAllText("$dir\restore.conf","`nlogging_collector = on`nlog_directory = '$dirSlash'`nlog_filename = 'pitr-server.log'`n",$Utf8)
        Start-Service PatchworkRestore
        $ctx.Port=15433
        Write-Utf8 $contextPath ($ctx|ConvertTo-Json)
        Use-Pg 'admin' $ctx.Database 15433
        # pg_ctl -w can finish at hot-standby consistency, before target promotion.
        $deadline=[DateTime]::UtcNow.AddSeconds(60)
        do {
            if((Invoke-Sql 'SELECT pg_is_in_recovery()') -eq 'f'){break}
            if([DateTime]::UtcNow -ge $deadline){throw 'PITR did not reach promotion'}
            Start-Sleep -Milliseconds 250
        } while($true)
        $state=Get-State
        Write-Utf8 "$DeployRoot\config\acceptance.toml" ((Get-Content "$DeployRoot\config\acceptance.toml" -Raw).Replace('port = 15432','port = 15433'))
        Start-Acceptance
        $report=@{Kind='PITR';Target=$TargetTime;State=($state|ConvertFrom-Json);Base=$base;Seconds=$timer.Elapsed.TotalSeconds;Port=15433;Ready=$true}
        Write-Utf8 "$dir\pitr-result.json" ($report|ConvertTo-Json -Depth 9)
        $report|ConvertTo-Json -Depth 9
    }
    'Inspect' {
        Use-Pg 'admin' $ctx.Database $ctx.Port
        @{Context=$ctx;State=((Get-State)|ConvertFrom-Json)}|ConvertTo-Json -Depth 9
    }
    'Cleanup' {
        Stop-Acceptance
        $restore=Assert-ProjectPath "$dir\pitr-data"
        Use-Pg 'admin' 'postgres'
        $restoreService=Get-CimInstance Win32_Service -Filter "Name='PatchworkRestore'"
        if($restoreService){
            if($restoreService.PathName -notlike "*$restore*"){throw 'Unexpected restore service path'}
            if($restoreService.State -ne 'Stopped'){Stop-Service PatchworkRestore}
            $null=Invoke-Native "$PgBin\pg_ctl.exe" @('unregister','-N','PatchworkRestore') "$dir\pitr.log"
        } elseif(@(Get-NetTCPConnection -State Listen -LocalPort 15433 -ErrorAction SilentlyContinue).Count){
            throw 'Unexpected listener remains on restore port; inspect before cleanup'
        }
        # Only the two databases created by this recorded acceptance context.
        foreach($db in @($ctx.Database,$ctx.LogicalDatabase)){$null=Invoke-Sql "DROP DATABASE IF EXISTS $db WITH (FORCE)"}
        Copy-Item -LiteralPath "$dir\original-pg_hba.conf" -Destination "$DeployRoot\config\postgresql\pg_hba.conf" -Force
        $null=Invoke-Sql 'SELECT pg_reload_conf()'
        Unregister-ScheduledTask -TaskName PatchworkAcceptance -Confirm:$false -ErrorAction SilentlyContinue
        foreach($file in @($contextPath,"$DeployRoot\config\acceptance.toml","$DeployRoot\config\acceptance-migration.toml")){Remove-Item -LiteralPath (Assert-ProjectPath $file) -ErrorAction SilentlyContinue}
        Set-PrivateAcl $dir
        $walAccess=@{};$walAccess[([Security.Principal.NTAccount]'NT SERVICE\PatchworkPostgres').Translate([Security.Principal.SecurityIdentifier]).Value]='Modify'
        Set-PrivateAcl "$DeployRoot\backups\wal" $walAccess
        Start-ScheduledTask -TaskName PatchworkBackend
        Wait-Backend
        Write-Output '{"cleanup":true,"production_ready":true}'
    }
}
