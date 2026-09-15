param([Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$PostgresZipSha256)
. "$PSScriptRoot\Common.ps1"
if($env:COMPUTERNAME -ne 'DESKTOP-NCGG1I7'){throw 'Unexpected target host'}
if(Test-Path -LiteralPath "$DeployRoot\config\deployment.json"){throw 'Deployment already initialized; use release scripts'}
if(Get-Service -Name PatchworkPostgres -ErrorAction SilentlyContinue){throw 'PostgreSQL service already exists'}
if(@(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object {$_.LocalPort -in 15432,18120}).Count){throw 'Deployment ports already in use'}
$zip="$DeployRoot\staging\postgresql-18.6.zip"
if((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne $PostgresZipSha256){throw 'PostgreSQL package hash mismatch'}
$runtime='D:\Tools\PostgreSQL\18.6'
if(Test-Path -LiteralPath $runtime){throw 'PostgreSQL runtime destination already exists; inspect before reuse'}
New-Item -ItemType Directory -Path $runtime -Force | Out-Null
Expand-Archive -LiteralPath $zip -DestinationPath $runtime
$pg="$runtime\pgsql\bin"
if((& "$pg\postgres.exe" --version) -ne 'postgres (PostgreSQL) 18.6'){throw 'Unexpected PostgreSQL binary version'}
Set-PrivateAcl $DeployRoot
foreach($dir in @('releases','config','config\postgresql','secrets','data','data\postgresql','logs','logs\postgresql','logs\backend','logs\backup','backups','backups\logical','backups\base','backups\wal','backups\metadata','tools','staging','tmp','tmp\backend','control','restore-tests')){
    New-Item -ItemType Directory -Path "$DeployRoot\$dir" -Force | Out-Null
}
if(@(Get-ChildItem -LiteralPath "$DeployRoot\data\postgresql" -Force).Count){throw 'Refusing to initialize nonempty PGDATA'}
function New-Secret {
    $bytes=New-Object byte[] 32
    $rng=[Security.Cryptography.RandomNumberGenerator]::Create()
    try {$rng.GetBytes($bytes)} finally {$rng.Dispose()}
    return ([BitConverter]::ToString($bytes)).Replace('-','').ToLowerInvariant()
}
if(Get-LocalUser -Name PatchworkBackend -ErrorAction SilentlyContinue){throw 'Backend account already exists'}
$backendPassword='Pw!'+(New-Secret)
$user=New-LocalUser -Name PatchworkBackend -Password (ConvertTo-SecureString $backendPassword -AsPlainText -Force) -AccountNeverExpires -PasswordNeverExpires -UserMayNotChangePassword -Description 'Patchwork scheduled backend only'
$backendSid=$user.SID.Value
& "$PSScriptRoot\Grant-BatchLogon.ps1" -Sid $backendSid
Write-Utf8 "$DeployRoot\secrets\backend-task.key" $backendPassword
foreach($role in @('admin','migrate','app','backup')){
    $password=New-Secret
    Write-Utf8 "$DeployRoot\secrets\postgres-$role.key" $password
    # Wildcard database also covers replication; these files are never passed in argv.
    Write-Utf8 "$DeployRoot\secrets\postgres-$role.pgpass" "127.0.0.1:*:*:patchwork_${role}:$password`n"
}
Write-Utf8 "$DeployRoot\secrets\jwt.key" (New-Secret)
$config=[ordered]@{Project='patchwork';Hostname=$env:COMPUTERNAME;Root=$DeployRoot;PgBin=$pg;PostgresVersion='18.6';PostgresPackageSha256=$PostgresZipSha256;BackendSid=$backendSid;DatabasePort=15432;BackendPort=18120;BackupScope='same-host-only';InitializedUtc=[DateTime]::UtcNow.ToString('o')}
Write-Utf8 "$DeployRoot\config\deployment.json" ($config|ConvertTo-Json)
# initdb deliberately disables the Administrators SID in its restricted token.
# Give the initializing identity explicit access, then remove it below.
$initializeAccess=@{}
$initializeAccess[[Security.Principal.WindowsIdentity]::GetCurrent().User.Value]='FullControl'
Set-PrivateAcl $DeployRoot $initializeAccess
$env:TEMP="$DeployRoot\tmp";$env:TMP=$env:TEMP
$null=Invoke-Native "$pg\initdb.exe" @('-D',"$DeployRoot\data\postgresql",'-U','patchwork_admin',"--pwfile=$DeployRoot\secrets\postgres-admin.key",'--encoding=UTF8','--locale=C','--auth=scram-sha-256','--data-checksums') "$DeployRoot\logs\initialize.log"
$postgresConf=@'
data_directory = 'D:/deploy_patchwork/data/postgresql'
hba_file = 'D:/deploy_patchwork/config/postgresql/pg_hba.conf'
ident_file = 'D:/deploy_patchwork/config/postgresql/pg_ident.conf'
listen_addresses = '127.0.0.1'
port = 15432
max_connections = 32
shared_buffers = '128MB'
work_mem = '4MB'
maintenance_work_mem = '64MB'
timezone = 'UTC'
log_timezone = 'UTC'
password_encryption = 'scram-sha-256'
fsync = on
full_page_writes = on
synchronous_commit = on
wal_level = replica
max_wal_senders = 3
archive_mode = on
archive_timeout = '60s'
archive_command = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:/deploy_patchwork/tools/Archive-Wal.ps1 -Source "%p" -Name "%f"'
logging_collector = on
log_destination = 'stderr'
log_directory = 'D:/deploy_patchwork/logs/postgresql'
log_filename = 'postgresql-%Y-%m-%d.log'
log_rotation_age = '1d'
log_statement = 'none'
log_min_error_statement = 'panic'
log_error_verbosity = terse
'@
Write-Utf8 "$DeployRoot\config\postgresql\postgresql.conf" $postgresConf
Write-Utf8 "$DeployRoot\config\postgresql\pg_hba.conf" @'
host all patchwork_admin 127.0.0.1/32 scram-sha-256
host patchwork patchwork_migrate,patchwork_app,patchwork_backup 127.0.0.1/32 scram-sha-256
host replication patchwork_backup 127.0.0.1/32 scram-sha-256
'@
Write-Utf8 "$DeployRoot\config\postgresql\pg_ident.conf" "# No identity maps.`n"
$null=Invoke-Native "$pg\pg_ctl.exe" @('register','-D',"$DeployRoot\data\postgresql",'-N','PatchworkPostgres','-U','NT SERVICE\PatchworkPostgres','-S','auto','-o','-c config_file=D:/deploy_patchwork/config/postgresql/postgresql.conf') "$DeployRoot\logs\initialize.log"
$pgSid=([Security.Principal.NTAccount]'NT SERVICE\PatchworkPostgres').Translate([Security.Principal.SecurityIdentifier]).Value
$read=@{};$read[$backendSid]='ReadAndExecute';$read[$pgSid]='ReadAndExecute'
Set-PrivateAcl $DeployRoot $read
Set-PrivateAcl "$DeployRoot\secrets" $read
foreach($file in Get-ChildItem -LiteralPath "$DeployRoot\secrets" -File){Set-PrivateAcl $file.FullName}
$backendRead=@{};$backendRead[$backendSid]='Read'
foreach($name in @('jwt.key','postgres-app.key','postgres-app.pgpass')){Set-PrivateAcl "$DeployRoot\secrets\$name" $backendRead}
$pgModify=@{};$pgModify[$pgSid]='Modify'
foreach($dir in @('data','config\postgresql','logs\postgresql','backups\wal')){Set-PrivateAcl "$DeployRoot\$dir" $pgModify}
$backendModify=@{};$backendModify[$backendSid]='Modify'
foreach($dir in @('logs\backend','tmp\backend','control')){Set-PrivateAcl "$DeployRoot\$dir" $backendModify}
foreach($dir in @('backups\logical','backups\base','backups\metadata','logs\backup','restore-tests')){Set-PrivateAcl "$DeployRoot\$dir"}
Write-Utf8 "$DeployRoot\logs\archive-health.json" '{"Ok":true,"At":null}'
Set-PrivateAcl "$DeployRoot\logs\archive-health.json" $pgModify
$null=Invoke-Native sc.exe @('failure','PatchworkPostgres','reset=','86400','actions=','restart/5000/restart/15000/restart/60000/none/0') "$DeployRoot\logs\initialize.log"
Start-Service -Name PatchworkPostgres
Use-Pg 'admin' 'postgres'
$env:PATCHWORK_MIGRATE_PASSWORD=Get-Content "$DeployRoot\secrets\postgres-migrate.key" -Raw
$env:PATCHWORK_APP_PASSWORD=Get-Content "$DeployRoot\secrets\postgres-app.key" -Raw
try {$null=Invoke-Native "$pg\psql.exe" @('-X','-v','ON_ERROR_STOP=1','-v','db_name=patchwork','-f',"$DeployRoot\tools\bootstrap.sql") "$DeployRoot\logs\initialize.log"} finally {Remove-Item Env:PATCHWORK_MIGRATE_PASSWORD,Env:PATCHWORK_APP_PASSWORD -ErrorAction SilentlyContinue}
$backupPassword=Get-Content "$DeployRoot\secrets\postgres-backup.key" -Raw
$null=Invoke-Sql "CREATE ROLE patchwork_backup LOGIN REPLICATION NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD '$backupPassword'; GRANT pg_read_all_data,pg_monitor TO patchwork_backup; GRANT CONNECT ON DATABASE patchwork TO patchwork_backup; GRANT EXECUTE ON FUNCTION pg_switch_wal() TO patchwork_backup;"
foreach($role in @('app','migrate')){
    $stop=if($role -eq 'app'){"shutdown_signal_file = 'D:/deploy_patchwork/control/backend.stop'"}else{''}
    $app=@"
listen = '127.0.0.1:18120'
jwt_secret_file = '../secrets/jwt.key'
allowed_origins = ['https://ckiddo.github.io','http://127.0.0.1:8082','http://localhost:8082']
workers = 2
shutdown_timeout_secs = 10
$stop
log_level = 'info'
[database]
host = '127.0.0.1'
port = 15432
name = 'patchwork'
user = 'patchwork_$role'
password_file = '../secrets/postgres-$role.key'
max_connections = 8
"@
    $name=if($role -eq 'app'){'backend'}else{'migration'}
    Write-Utf8 "$DeployRoot\config\$name.toml" $app
}
# Only this task can read its application credentials; schema checks happen in the binary.
$action=New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Run-Backend.ps1' -WorkingDirectory $DeployRoot
$trigger=New-ScheduledTaskTrigger -AtStartup
$settings=New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$null=Register-ScheduledTask -TaskName PatchworkBackend -Action $action -Trigger $trigger -Settings $settings -User "$env:COMPUTERNAME\PatchworkBackend" -Password $backendPassword -RunLevel Limited -Description 'Patchwork backend: bounded recovery, loopback PostgreSQL dependency'
$backendPassword=$null;$backupPassword=$null
Use-Pg 'admin' 'patchwork'
Write-Output (Invoke-Sql "SELECT json_build_object('version',version(),'data_directory',current_setting('data_directory'),'port',current_setting('port'),'listen',current_setting('listen_addresses'),'checksums',current_setting('data_checksums'),'encoding',current_setting('server_encoding'),'archive_mode',current_setting('archive_mode'))")
Write-Output 'Patchwork host initialized; publish a verified release before starting the backend task.'
