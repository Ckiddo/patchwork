. "$PSScriptRoot\Common.ps1"
$issues=@()
try {
    Use-Pg 'backup'
    $archive=Invoke-Sql "SELECT row_to_json(s) FROM (SELECT archived_count,failed_count,last_archived_time,last_failed_time FROM pg_stat_archiver) s" | ConvertFrom-Json
    if($archive.last_failed_time -and (!$archive.last_archived_time -or [DateTime]$archive.last_failed_time -gt [DateTime]$archive.last_archived_time)){$issues+='wal_archive_failed'}
    $ready=@(Get-ChildItem "$DeployRoot\data\postgresql\pg_wal\archive_status" -Filter '*.ready' -File)
    if(@($ready|Where-Object {$_.LastWriteTimeUtc -lt [DateTime]::UtcNow.AddMinutes(-2)}).Count){$issues+='wal_archive_delayed'}
    $base=@(Get-ChildItem "$DeployRoot\backups\base" -Directory | Where-Object {Test-Path "$($_.FullName)\manifest.json"} | Sort-Object Name -Descending)
    $logical=@(Get-ChildItem "$DeployRoot\backups\logical" -Directory | Where-Object {Test-Path "$($_.FullName)\manifest.json"} | Sort-Object Name -Descending)
    if(!$base.Count -or $base[0].CreationTimeUtc -lt [DateTime]::UtcNow.AddDays(-8)){$issues+='base_backup_stale'}
    if(!$logical.Count -or $logical[0].CreationTimeUtc -lt [DateTime]::UtcNow.AddHours(-26)){$issues+='logical_backup_stale'}
    if((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'").FreeSpace -lt 20GB){$issues+='disk_space_low'}
    if((Get-ChildItem "$DeployRoot\data\postgresql\pg_wal" -File | Measure-Object Length -Sum).Sum -gt 2GB){$issues+='pg_wal_growth'}
} catch {$issues+='backup_monitor_failed'}
$previous=$null
if(Test-Path "$DeployRoot\logs\backup-health.json"){$previous=Get-Content "$DeployRoot\logs\backup-health.json" -Raw|ConvertFrom-Json}
$health=@{At=[DateTime]::UtcNow.ToString('o');Ok=($issues.Count -eq 0);Issues=$issues;BackupScope='same-host-only';Retention='all complete bases and continuous WAL retained; no automatic pruning'}
Write-Utf8 "$DeployRoot\logs\backup-health.json" ($health|ConvertTo-Json)
if($issues.Count -and (($previous.Issues -join ',') -ne ($issues -join ','))){Write-EventLog -LogName Application -Source PatchworkBackup -EntryType Error -EventId 4101 -Message ('Patchwork backup needs attention: '+($issues -join ', '))}
$health|ConvertTo-Json
if($issues.Count){exit 1}
