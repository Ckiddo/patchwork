param([ValidateSet('Logical','Base','All')][string]$Kind='Logical')
. "$PSScriptRoot\Common.ps1"
$c=Get-Deployment
$mutex=New-Object Threading.Mutex($false,'Global\PatchworkBackup')
$locked=$false
try {
    try {$locked=$mutex.WaitOne(0)} catch [Threading.AbandonedMutexException] {$locked=$true}
    if(-not $locked){throw 'Another Patchwork backup is active'}
    $stamp=[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    $log="$DeployRoot\logs\backup\$stamp.log"
    Use-Pg 'backup'
    $metadata="$DeployRoot\backups\metadata\$stamp"
    New-Item -ItemType Directory -Path $metadata | Out-Null
    # Includes protected secrets required to recover the original browser identities.
    Copy-Item -LiteralPath "$DeployRoot\config","$DeployRoot\secrets","$DeployRoot\tools" -Destination $metadata -Recurse
    Set-PrivateAcl $metadata
    $release=(Get-Content "$DeployRoot\config\current-release.txt" -Raw).Trim()
    Copy-Item -LiteralPath "$DeployRoot\releases\$release\manifest.json" -Destination "$metadata\release-manifest.json"
    $null=Invoke-Native "$PgBin\pg_dumpall.exe" @('--roles-only','--no-role-passwords','--no-password','-l','patchwork','--file',"$metadata\roles.sql") $log
    $metadataFiles=@(Get-ChildItem -LiteralPath $metadata -Recurse -File | ForEach-Object {@{Path=$_.FullName.Substring($metadata.Length+1);Bytes=$_.Length;Sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}})
    Write-Utf8 "$metadata\manifest.json" ((@{Files=$metadataFiles;Complete=$true})|ConvertTo-Json -Depth 5)
    $state=Invoke-Sql "SELECT json_build_object('server_version',current_setting('server_version'),'system_identifier',(pg_control_system()).system_identifier::text,'schema',(SELECT json_agg(json_build_object('version',version,'checksum',encode(checksum,'hex'),'success',success) ORDER BY version) FROM patchwork._sqlx_migrations),'lsn',pg_current_wal_lsn()::text,'at',clock_timestamp())"
    $outputs=@()
    if($Kind -in @('Logical','All')){
        $dir="$DeployRoot\backups\logical\$stamp"
        New-Item -ItemType Directory -Path $dir | Out-Null
        $file="$dir\patchwork.dump"
        $null=Invoke-Native "$PgBin\pg_dump.exe" @('--format=custom','--no-password','--file',$file,'patchwork') $log
        $null=Invoke-Native "$PgBin\pg_restore.exe" @('--list',$file) $log
        $manifest=@{Kind='Logical';At=$stamp;Release=$release;Metadata=$metadata;State=($state|ConvertFrom-Json);Files=@(@{Path='patchwork.dump';Bytes=(Get-Item $file).Length;Sha256=(Get-FileHash $file -Algorithm SHA256).Hash});Complete=$true}
        Write-Utf8 "$dir\manifest.json" ($manifest|ConvertTo-Json -Depth 8)
        $outputs+=@{Kind='Logical';Path=$dir;Bytes=(Get-Item $file).Length}
    }
    if($Kind -in @('Base','All')){
        $dir="$DeployRoot\backups\base\$stamp"
        New-Item -ItemType Directory -Path $dir | Out-Null
        $null=Invoke-Native "$PgBin\pg_basebackup.exe" @('--no-password','--pgdata',"$dir\data",'--format=plain','--wal-method=stream','--checkpoint=fast','--manifest-checksums=SHA256') $log
        $null=Invoke-Native "$PgBin\pg_verifybackup.exe" @("$dir\data") $log
        $pgManifest=Get-Content "$dir\data\backup_manifest" -Raw | ConvertFrom-Json
        $manifest=@{Kind='Base';At=$stamp;Release=$release;Metadata=$metadata;State=($state|ConvertFrom-Json);Complete=$true;Verified=$true;WalRanges=$pgManifest.'WAL-Ranges';ManifestSha256=(Get-FileHash "$dir\data\backup_manifest" -Algorithm SHA256).Hash}
        Write-Utf8 "$dir\manifest.json" ($manifest|ConvertTo-Json -Depth 8)
        $outputs+=@{Kind='Base';Path=$dir;Verified=$true}
    }
    # Keep every base and WAL until an explicitly verified chain can be retired.
    # No timestamp-only WAL deletion is performed.
    Write-Utf8 "$DeployRoot\backups\last-success.json" ((@{At=[DateTime]::UtcNow.ToString('o');Kind=$Kind;Outputs=$outputs})|ConvertTo-Json -Depth 5)
    $outputs|ConvertTo-Json -Depth 3
} catch {
    Write-Utf8 "$DeployRoot\backups\last-failure.json" ((@{At=[DateTime]::UtcNow.ToString('o');Kind=$Kind;Reason='backup job failed; inspect protected backup log'})|ConvertTo-Json)
    if([Diagnostics.EventLog]::SourceExists('PatchworkBackup')){Write-EventLog -LogName Application -Source PatchworkBackup -EntryType Error -EventId 4102 -Message "Patchwork $Kind backup failed; inspect protected backup log"}
    throw
} finally {if($locked){$mutex.ReleaseMutex()};$mutex.Dispose()}
