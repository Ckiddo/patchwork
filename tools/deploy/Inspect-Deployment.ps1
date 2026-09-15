. "$PSScriptRoot\Common.ps1"
$c=Get-Deployment
Use-Pg admin
$release=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
$manifest=Test-Release "$DeployRoot\releases\$release"
Assert-Schema $manifest
$database=(Invoke-Sql @'
SELECT json_build_object('version',current_setting('server_version'),'data_directory',current_setting('data_directory'),
'listen',current_setting('listen_addresses'),'port',current_setting('port'),'encoding',current_setting('server_encoding'),
'checksums',current_setting('data_checksums'),'archive_mode',current_setting('archive_mode'),'archive_timeout',current_setting('archive_timeout'),
'schemas',(SELECT json_agg(json_build_object('version',version,'success',success) ORDER BY version) FROM patchwork._sqlx_migrations),
'roles',(SELECT json_agg(json_build_object('name',rolname,'superuser',rolsuper,'createdb',rolcreatedb,'createrole',rolcreaterole,'replication',rolreplication,'login',rolcanlogin,'scram',coalesce(rolpassword LIKE 'SCRAM-SHA-256$%',false)) ORDER BY rolname) FROM pg_authid WHERE rolname LIKE 'patchwork_%'),
'users',(SELECT count(*) FROM patchwork.users),'games',(SELECT count(*) FROM patchwork.games),
'temporary_databases',(SELECT count(*) FROM pg_database WHERE datname LIKE 'patchwork_test_deploy_%' OR datname LIKE 'patchwork_test_restore_%'),
'archiver',(SELECT row_to_json(a) FROM (SELECT archived_count,failed_count,last_archived_wal,last_archived_time,last_failed_time FROM pg_stat_archiver) a));
'@)|ConvertFrom-Json
$tasks=@(Get-ScheduledTask|Where-Object {$_.TaskName -like 'Patchwork*'}|ForEach-Object {
    $info=Get-ScheduledTaskInfo -TaskName $_.TaskName
    @{Name=$_.TaskName;State=[string]$_.State;User=$_.Principal.UserId;RunLevel=[string]$_.Principal.RunLevel;RestartCount=$_.Settings.RestartCount;NextRun=$info.NextRunTime;LastResult=$info.LastTaskResult}
})
$services=@(Get-CimInstance Win32_Service | Where-Object {$_.Name -in 'PatchworkPostgres','PatchworkRestore','re-xianyu-postgresql'}|ForEach-Object {@{Name=$_.Name;State=$_.State;StartMode=$_.StartMode;Identity=$_.StartName;ProcessId=$_.ProcessId;Path=$_.PathName}})
$ports=@(Get-NetTCPConnection -State Listen|Where-Object {$_.LocalPort -in 15432,18120,15433,18121}|ForEach-Object {@{Address=$_.LocalAddress;Port=$_.LocalPort;Pid=$_.OwningProcess}})
$report=@{At=[DateTime]::UtcNow.ToString('o');Hostname=$env:COMPUTERNAME;TimeZone=(Get-TimeZone).Id;Root=$DeployRoot;Release=$release;BaseCommit=$manifest.BaseCommit;Dirty=$manifest.Dirty;SourceSha256=$manifest.SourceSha256;Files=$manifest.Files;Database=$database;Tasks=$tasks;Services=$services;Ports=$ports;Ready=(Invoke-RestMethod http://127.0.0.1:18120/readyz);BackupHealth=(Get-Content "$DeployRoot\logs\backup-health.json" -Raw|ConvertFrom-Json);IdentityCheck=(Get-Content "$DeployRoot\control\identity-check.json" -Raw|ConvertFrom-Json)}
Write-Utf8 "$DeployRoot\control\deployment-audit.json" ($report|ConvertTo-Json -Depth 10)
$report|ConvertTo-Json -Depth 10
